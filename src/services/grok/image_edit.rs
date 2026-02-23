use serde_json::Value as JsonValue;

use crate::core::exceptions::ApiError;
use crate::services::grok::assets::UploadService;
use crate::services::grok::chat::GrokChatService;
use crate::services::grok::media::VideoService;
use crate::services::grok::model::ModelInfo;
use crate::services::grok::processor::ImageCollectProcessor;

use super::chat::LineStream;

pub struct ImageEditService;

impl ImageEditService {
    /// 上传图片并返回 asset URL 列表
    async fn upload_images(images: &[String], token: &str) -> Result<Vec<String>, ApiError> {
        let uploader = UploadService::new().await;
        let mut image_urls = Vec::new();
        for image_data in images {
            let (_file_id, file_uri) = uploader.upload(image_data, token).await?;
            if file_uri.starts_with("http") {
                image_urls.push(file_uri);
            } else {
                image_urls.push(format!(
                    "https://assets.grok.com/{}",
                    file_uri.trim_start_matches('/')
                ));
            }
        }
        if image_urls.is_empty() {
            return Err(ApiError::server("Image upload failed"));
        }
        Ok(image_urls)
    }

    /// 获取 parent_post_id
    async fn get_parent_post_id(token: &str, image_urls: &[String]) -> String {
        if image_urls.is_empty() {
            return String::new();
        }

        // 尝试通过 VideoService::create_image_post 获取
        let service = VideoService::new().await;
        match service.create_image_post(token, &image_urls[0]).await {
            Ok(post_id) if !post_id.is_empty() => {
                tracing::debug!("Parent post ID: {post_id}");
                return post_id;
            }
            Err(e) => {
                tracing::warn!("Create image post failed: {e}");
            }
            _ => {}
        }

        // 回退：从 URL 中提取 ID
        for url in image_urls {
            // 匹配 /generated/UUID/ 模式
            if let Some(id) = extract_id_from_url(url, "generated") {
                tracing::debug!("Parent post ID from URL: {id}");
                return id;
            }
            // 匹配 /users/xxx/UUID/content 模式
            if let Some(id) = extract_id_from_url(url, "users") {
                tracing::debug!("Parent post ID from URL: {id}");
                return id;
            }
        }

        String::new()
    }

    /// 流式编辑图片
    pub async fn edit_stream(
        token: &str,
        model_info: &ModelInfo,
        prompt: &str,
        images: &[String],
        _n: usize,
        _return_base64: bool,
    ) -> Result<LineStream, ApiError> {
        let image_urls = Self::upload_images(images, token).await?;
        let parent_post_id = Self::get_parent_post_id(token, &image_urls).await;

        let mut model_config = serde_json::json!({
            "modelMap": {
                "imageEditModel": "imagine",
                "imageEditModelConfig": {
                    "imageReferences": image_urls,
                }
            }
        });
        if !parent_post_id.is_empty() {
            model_config["modelMap"]["imageEditModelConfig"]["parentPostId"] =
                serde_json::json!(parent_post_id);
        }

        let tool_overrides = serde_json::json!({"imageGen": true});

        let chat_service = GrokChatService::new().await;
        chat_service
            .chat(
                token,
                prompt,
                &model_info.grok_model,
                &model_info.model_mode,
                Some(false),
                true,
                &[],
                &[],
                None,
                None,
                None,
                Some(&tool_overrides),
                Some(&model_config),
            )
            .await
    }

    /// 非流式编辑图片
    pub async fn edit_collect(
        token: &str,
        model_info: &ModelInfo,
        prompt: &str,
        images: &[String],
        n: usize,
        return_base64: bool,
    ) -> Result<Vec<JsonValue>, ApiError> {
        let response =
            Self::edit_stream(token, model_info, prompt, images, n, return_base64).await?;
        let processor =
            ImageCollectProcessor::new(&model_info.model_id, token, return_base64).await;
        Ok(processor.process(response).await)
    }
}

/// 从 URL 路径中提取 UUID
fn extract_id_from_url(url: &str, prefix_segment: &str) -> Option<String> {
    let parts: Vec<&str> = url.split('/').collect();
    for (i, part) in parts.iter().enumerate() {
        if *part == prefix_segment && i + 1 < parts.len() {
            let candidate = parts[i + 1];
            // UUID 通常 32-36 字符（含连字符）
            if candidate.len() >= 32 && candidate.len() <= 36 {
                return Some(candidate.to_string());
            }
        }
    }
    None
}
