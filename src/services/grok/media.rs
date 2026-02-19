use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures::Stream;
use serde_json::Value as JsonValue;
use tokio::sync::Semaphore;

use crate::core::config::get_config;
use crate::core::exceptions::ApiError;
use crate::services::grok::assets::UploadService;
use crate::services::grok::chat::MessageExtractor;
use crate::services::grok::model::ModelService;
use crate::services::grok::statsig::StatsigService;
use crate::services::grok::wreq_client::{
    apply_headers, body_preview, build_client, line_stream_from_response,
};
use crate::services::token::TokenService;

const CREATE_POST_API: &str = "https://grok.com/rest/media/post/create";
const CHAT_API: &str = "https://grok.com/rest/app-chat/conversations/new";
const VIDEO_UPSCALE_API: &str = "https://grok.com/rest/media/video/upscale";

static MEDIA_SEM: once_cell::sync::Lazy<Arc<Semaphore>> =
    once_cell::sync::Lazy::new(|| Arc::new(Semaphore::new(50)));

pub type LineStream = Pin<Box<dyn Stream<Item = String> + Send>>;

pub struct VideoService;

impl VideoService {
    pub async fn new() -> Self {
        Self
    }

    async fn build_headers(&self, token: &str, referer: &str) -> reqwest::header::HeaderMap {
        crate::services::grok::headers::build_grok_headers(token, None, Some(referer)).await
    }

    async fn create_post(&self, token: &str, prompt: &str) -> Result<String, ApiError> {
        let headers = self.build_headers(token, "https://grok.com/imagine").await;
        let payload = serde_json::json!({"mediaType": "MEDIA_POST_TYPE_VIDEO", "prompt": prompt});
        let value = self
            .wreq_json(CREATE_POST_API, headers, &payload, 30)
            .await?;
        Ok(value
            .get("post")
            .and_then(|v| v.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string())
    }

    pub async fn create_image_post(&self, token: &str, image_url: &str) -> Result<String, ApiError> {
        let headers = self.build_headers(token, "https://grok.com/imagine").await;
        let payload =
            serde_json::json!({"mediaType": "MEDIA_POST_TYPE_IMAGE", "mediaUrl": image_url});
        let value = self
            .wreq_json(CREATE_POST_API, headers, &payload, 30)
            .await?;
        Ok(value
            .get("post")
            .and_then(|v| v.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string())
    }

    async fn build_payload(
        &self,
        prompt: &str,
        post_id: &str,
        aspect_ratio: &str,
        video_length: i32,
        resolution: &str,
        preset: &str,
    ) -> JsonValue {
        let mode_flag = match preset {
            "fun" => "--mode=extremely-crazy",
            "normal" => "--mode=normal",
            "spicy" => "--mode=extremely-spicy-or-crazy",
            _ => "--mode=custom",
        };
        let full_prompt = format!("{prompt} {mode_flag}");
        serde_json::json!({
            "temporary": true,
            "modelName": "grok-3",
            "message": full_prompt,
            "toolOverrides": {"videoGen": true},
            "enableSideBySide": true,
            "responseMetadata": {
                "experiments": [],
                "modelConfigOverride": {"modelMap": {"videoGenModelConfig": {
                    "parentPostId": post_id,
                    "aspectRatio": aspect_ratio,
                    "videoLength": video_length,
                    "videoResolution": resolution,
                }}}
            }
        })
    }

    async fn generate(
        &self,
        token: &str,
        prompt: &str,
        aspect_ratio: &str,
        video_length: i32,
        resolution: &str,
        preset: &str,
    ) -> Result<LineStream, ApiError> {
        let _permit = MEDIA_SEM.clone().acquire_owned().await.unwrap();
        let post_id = self.create_post(token, prompt).await?;
        let headers = self.build_headers(token, "https://grok.com/imagine").await;
        let payload = self
            .build_payload(
                prompt,
                &post_id,
                aspect_ratio,
                video_length,
                resolution,
                preset,
            )
            .await;
        let timeout: u64 = get_config("grok.timeout", 300u64).await;
        self.wreq_stream(CHAT_API, headers, &payload, timeout).await
    }

    async fn generate_from_image(
        &self,
        token: &str,
        prompt: &str,
        image_url: &str,
        aspect_ratio: &str,
        video_length: i32,
        resolution: &str,
        preset: &str,
    ) -> Result<LineStream, ApiError> {
        let _permit = MEDIA_SEM.clone().acquire_owned().await.unwrap();
        let post_id = self.create_image_post(token, image_url).await?;
        let headers = self.build_headers(token, "https://grok.com/imagine").await;
        let payload = self
            .build_payload(
                prompt,
                &post_id,
                aspect_ratio,
                video_length,
                resolution,
                preset,
            )
            .await;
        let timeout: u64 = get_config("grok.timeout", 300u64).await;
        self.wreq_stream(CHAT_API, headers, &payload, timeout).await
    }

    async fn wreq_json(
        &self,
        url: &str,
        headers: reqwest::header::HeaderMap,
        payload: &JsonValue,
        timeout: u64,
    ) -> Result<JsonValue, ApiError> {
        let proxy: String = get_config("grok.base_proxy_url", String::new()).await;
        let client = build_client(Some(&proxy), timeout).await?;
        let response = apply_headers(client.post(url), &headers)
            .timeout(Duration::from_secs(timeout.max(1)))
            .body(payload.to_string())
            .send()
            .await
            .map_err(|e| ApiError::upstream(format!("Media request failed: {e}")))?;

        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("<unknown>")
            .to_string();
        let body = response
            .text()
            .await
            .map_err(|e| ApiError::upstream(format!("Media response read failed: {e}")))?;

        if status != 200 {
            let preview = body_preview(&body, 220);
            return Err(ApiError::upstream(format!(
                "Media request failed: {status}; content-type: {content_type}; body: {preview}"
            )));
        }

        serde_json::from_str(&body).map_err(|e| {
            let preview = body_preview(&body, 220);
            ApiError::upstream(format!(
                "Media parse error: {e}; content-type: {content_type}; body: {preview}"
            ))
        })
    }

    async fn wreq_stream(
        &self,
        url: &str,
        headers: reqwest::header::HeaderMap,
        payload: &JsonValue,
        timeout: u64,
    ) -> Result<LineStream, ApiError> {
        let proxy: String = get_config("grok.base_proxy_url", String::new()).await;
        let client = build_client(Some(&proxy), timeout).await?;
        let response = apply_headers(client.post(url), &headers)
            .timeout(Duration::from_secs(timeout.max(1)))
            .body(payload.to_string())
            .send()
            .await
            .map_err(|e| ApiError::upstream(format!("Media request failed: {e}")))?;

        let status_code = response.status().as_u16();
        if status_code != 200 {
            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("<unknown>")
                .to_string();
            let body = response.text().await.unwrap_or_else(|_| String::new());
            let preview = body_preview(&body, 220);
            if !preview.is_empty() {
                tracing::warn!(
                    "Media error status={} content_type={} body={}",
                    status_code,
                    content_type,
                    preview
                );
            }
            return Err(ApiError::upstream(format!(
                "Media request failed: {status_code}; content-type: {content_type}; body: {preview}"
            )));
        }

        Ok(line_stream_from_response(response))
    }

    pub async fn completions(
        model: &str,
        messages: Vec<JsonValue>,
        stream: Option<bool>,
        thinking: Option<String>,
        aspect_ratio: &str,
        video_length: i32,
        resolution: &str,
        preset: &str,
    ) -> Result<VideoResult, ApiError> {
        let token = TokenService::get_token_for_model(model).await?;
        let think = match thinking.as_deref() {
            Some("enabled") => Some(true),
            Some("disabled") => Some(false),
            _ => None,
        };
        let _model_info =
            ModelService::get(model).ok_or_else(|| ApiError::invalid_request("Unknown model"))?;
        let (prompt, attachments) = MessageExtractor::extract(&messages, true)?;

        let mut image_url: Option<String> = None;
        if !attachments.is_empty() {
            let uploader = UploadService::new().await;
            for (kind, data) in attachments {
                if kind == "image" {
                    let (_file_id, file_uri) = uploader.upload(&data, &token).await?;
                    image_url = Some(format!("https://assets.grok.com/{file_uri}"));
                    break;
                }
            }
        }

        let service = VideoService::new().await;
        let is_stream = stream.unwrap_or(get_config("grok.stream", true).await);

        let line_stream = if let Some(url) = image_url {
            service
                .generate_from_image(
                    &token,
                    &prompt,
                    &url,
                    aspect_ratio,
                    video_length,
                    resolution,
                    preset,
                )
                .await?
        } else {
            service
                .generate(
                    &token,
                    &prompt,
                    aspect_ratio,
                    video_length,
                    resolution,
                    preset,
                )
                .await?
        };

        Ok(VideoResult::Stream {
            stream: line_stream,
            token: token.clone(),
            model: model.to_string(),
            think,
            is_stream,
            upscale_on_finish: resolution == "720p",
        })
    }
}

#[allow(dead_code)]
pub enum VideoResult {
    Stream {
        stream: LineStream,
        token: String,
        model: String,
        think: Option<bool>,
        is_stream: bool,
        upscale_on_finish: bool,
    },
    Json(JsonValue),
}

/// 从视频 URL 中提取 video_id (UUID)
pub fn extract_video_id(video_url: &str) -> Option<String> {
    if video_url.is_empty() {
        return None;
    }
    // 匹配 /UUID/generated_video 模式
    let parts: Vec<&str> = video_url.split('/').collect();
    for (i, part) in parts.iter().enumerate() {
        if *part == "generated_video" && i > 0 {
            let candidate = parts[i - 1];
            // UUID 长度 32-36 字符
            if candidate.len() >= 32 && candidate.len() <= 36 {
                return Some(candidate.to_string());
            }
        }
    }
    None
}

pub struct VideoUpscaleService;

impl VideoUpscaleService {
    /// 调用视频 upscale 接口，返回 HD URL
    pub async fn upscale(token: &str, video_url: &str) -> Option<String> {
        let video_id = extract_video_id(video_url)?;
        if video_id.is_empty() {
            tracing::warn!("Video upscale skipped: unable to extract video id");
            return None;
        }

        let proxy: String = get_config("grok.base_proxy_url", String::new()).await;
        let timeout: u64 = get_config("grok.timeout", 60u64).await;

        let client = match build_client(Some(&proxy), timeout).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Video upscale client build failed: {e}");
                return None;
            }
        };

        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("Accept", "*/*".parse().unwrap());
        headers.insert("Content-Type", "application/json".parse().unwrap());
        headers.insert("Origin", "https://grok.com".parse().unwrap());
        headers.insert("Referer", "https://grok.com/".parse().unwrap());
        let raw = token.strip_prefix("sso=").unwrap_or(token);
        let cf: String = get_config("grok.cf_clearance", String::new()).await;
        let cookie = if cf.is_empty() {
            format!("sso={raw}")
        } else {
            format!("sso={raw};cf_clearance={cf}")
        };
        headers.insert("Cookie", cookie.parse().unwrap());
        let statsig = StatsigService::gen_id().await;
        headers.insert("x-statsig-id", statsig.parse().unwrap());
        headers.insert(
            "x-xai-request-id",
            uuid::Uuid::new_v4().to_string().parse().unwrap(),
        );

        let payload = serde_json::json!({"videoId": video_id});

        let response = match apply_headers(client.post(VIDEO_UPSCALE_API), &headers)
            .timeout(Duration::from_secs(timeout))
            .body(payload.to_string())
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("Video upscale request failed: {e}");
                return None;
            }
        };

        if response.status().as_u16() != 200 {
            tracing::warn!("Video upscale failed with status: {}", response.status());
            return None;
        }

        let body: JsonValue = match response.text().await {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(_) => return None,
            },
            Err(_) => return None,
        };

        let hd_url = body.get("hdMediaUrl").and_then(|v| v.as_str());
        if let Some(url) = hd_url {
            tracing::info!("Video upscale completed: {url}");
            Some(url.to_string())
        } else {
            None
        }
    }
}
