use std::pin::Pin;

use futures::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::core::config::get_config;
use crate::core::exceptions::ApiError;
use crate::services::grok::assets::UploadService;
use crate::services::grok::headers::build_grok_headers;
use crate::services::grok::model::ModelService;
use crate::services::grok::wreq_client::{
    apply_headers, body_preview, build_client, line_stream_from_response,
};
use crate::services::token::TokenService;

const CHAT_API: &str = "https://grok.com/rest/app-chat/conversations/new";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<JsonValue>,
    pub stream: Option<bool>,
    pub think: Option<bool>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub reasoning_effort: Option<String>,
}

pub struct MessageExtractor;

impl MessageExtractor {
    pub fn extract(
        messages: &[JsonValue],
        is_video: bool,
    ) -> Result<(String, Vec<(String, String)>), ApiError> {
        let mut texts: Vec<String> = Vec::new();
        let mut attachments: Vec<(String, String)> = Vec::new();
        let mut extracted: Vec<(String, String)> = Vec::new();

        for msg in messages {
            let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("user");
            let content = msg.get("content");
            let mut parts: Vec<String> = Vec::new();
            if let Some(content) = content {
                if let Some(text) = content.as_str() {
                    if !text.trim().is_empty() {
                        parts.push(text.to_string());
                    }
                } else if let Some(list) = content.as_array() {
                    for item in list {
                        let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        match item_type {
                            "text" => {
                                if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                                    if !text.trim().is_empty() {
                                        parts.push(text.to_string());
                                    }
                                }
                            }
                            "image_url" => {
                                if let Some(url_obj) = item.get("image_url") {
                                    let url = if let Some(u) =
                                        url_obj.get("url").and_then(|v| v.as_str())
                                    {
                                        u.to_string()
                                    } else if let Some(u) = url_obj.as_str() {
                                        u.to_string()
                                    } else {
                                        String::new()
                                    };
                                    if !url.is_empty() {
                                        attachments.push(("image".to_string(), url));
                                    }
                                }
                            }
                            "input_audio" => {
                                if is_video {
                                    return Err(ApiError::invalid_request(
                                        "视频模型不支持 input_audio 类型",
                                    ));
                                }
                                if let Some(audio_obj) = item.get("input_audio") {
                                    let data = if let Some(d) =
                                        audio_obj.get("data").and_then(|v| v.as_str())
                                    {
                                        d.to_string()
                                    } else if let Some(d) = audio_obj.as_str() {
                                        d.to_string()
                                    } else {
                                        String::new()
                                    };
                                    if !data.is_empty() {
                                        attachments.push(("audio".to_string(), data));
                                    }
                                }
                            }
                            "file" => {
                                if is_video {
                                    return Err(ApiError::invalid_request(
                                        "视频模型不支持 file 类型",
                                    ));
                                }
                                if let Some(file_obj) = item.get("file") {
                                    let url = file_obj
                                        .get("url")
                                        .and_then(|v| v.as_str())
                                        .or_else(|| file_obj.get("data").and_then(|v| v.as_str()))
                                        .or_else(|| file_obj.as_str())
                                        .unwrap_or("");
                                    if !url.is_empty() {
                                        attachments.push(("file".to_string(), url.to_string()));
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            if !parts.is_empty() {
                extracted.push((role.to_string(), parts.join("\n")));
            }
        }

        let mut last_user = None;
        for (i, (role, _)) in extracted.iter().enumerate().rev() {
            if role == "user" {
                last_user = Some(i);
                break;
            }
        }
        for (i, (role, text)) in extracted.iter().enumerate() {
            if Some(i) == last_user {
                texts.push(text.clone());
            } else {
                texts.push(format!(
                    "{}: {}",
                    if role.is_empty() { "user" } else { role },
                    text
                ));
            }
        }

        Ok((texts.join("\n\n"), attachments))
    }
}

pub struct ChatRequestBuilder;

impl ChatRequestBuilder {
    pub async fn build_headers(token: &str) -> reqwest::header::HeaderMap {
        build_grok_headers(token, None, None).await
    }

    pub async fn build_payload(
        message: &str,
        model: &str,
        mode: &str,
        think: Option<bool>,
        file_attachments: &[String],
        image_attachments: &[String],
        temperature: Option<f64>,
        top_p: Option<f64>,
        reasoning_effort: Option<&str>,
        tool_overrides: Option<&JsonValue>,
        model_config_override: Option<&JsonValue>,
    ) -> JsonValue {
        let temporary: bool = get_config("grok.temporary", true).await;
        let _think = think.unwrap_or(get_config("grok.thinking", false).await);

        // 构建 modelConfigOverride（扁平结构，与官网一致）
        let config_override = if let Some(mco) = model_config_override {
            mco.clone()
        } else {
            let mut config = serde_json::json!({});
            if let Some(t) = temperature {
                config["temperature"] = serde_json::json!(t);
            }
            if let Some(p) = top_p {
                config["topP"] = serde_json::json!(p);
            }
            if let Some(re) = reasoning_effort {
                config["reasoningEffort"] = serde_json::json!(re);
            }
            config
        };

        let tools = if let Some(to) = tool_overrides {
            to.clone()
        } else {
            serde_json::json!({})
        };

        serde_json::json!({
            "temporary": temporary,
            "modelName": model,
            "modelMode": mode,
            "message": message,
            "fileAttachments": file_attachments,
            "imageAttachments": image_attachments,
            "disableSearch": false,
            "enableImageGeneration": true,
            "returnImageBytes": false,
            "returnRawGrokInXaiRequest": false,
            "enableImageStreaming": true,
            "imageGenerationCount": 2,
            "forceConcise": false,
            "toolOverrides": tools,
            "enableSideBySide": true,
            "sendFinalMetadata": true,
            "isReasoning": false,
            "disableTextFollowUps": false,
            "responseMetadata": {
                "modelConfigOverride": config_override,
                "requestModelDetails": {"modelId": model},
            },
            "disableMemory": false,
            "forceSideBySide": false,
            "isAsyncChat": false,
            "disableSelfHarmShortCircuit": false,
            "deviceEnvInfo": {
                "darkModeEnabled": false,
                "devicePixelRatio": 2,
                "screenWidth": 2056,
                "screenHeight": 1329,
                "viewportWidth": 2056,
                "viewportHeight": 1083,
            }
        })
    }
}

pub struct GrokChatService;

impl GrokChatService {
    pub async fn new() -> Self {
        Self
    }

    pub async fn chat(
        &self,
        token: &str,
        message: &str,
        model: &str,
        mode: &str,
        think: Option<bool>,
        _stream: bool,
        file_attachments: &[String],
        image_attachments: &[String],
        temperature: Option<f64>,
        top_p: Option<f64>,
        reasoning_effort: Option<&str>,
        tool_overrides: Option<&JsonValue>,
        model_config_override: Option<&JsonValue>,
    ) -> Result<LineStream, ApiError> {
        self.chat_via_wreq(
            token,
            message,
            model,
            mode,
            think,
            file_attachments,
            image_attachments,
            temperature,
            top_p,
            reasoning_effort,
            tool_overrides,
            model_config_override,
        )
        .await
    }

    async fn chat_via_wreq(
        &self,
        token: &str,
        message: &str,
        model: &str,
        mode: &str,
        think: Option<bool>,
        file_attachments: &[String],
        image_attachments: &[String],
        temperature: Option<f64>,
        top_p: Option<f64>,
        reasoning_effort: Option<&str>,
        tool_overrides: Option<&JsonValue>,
        model_config_override: Option<&JsonValue>,
    ) -> Result<LineStream, ApiError> {
        let headers = ChatRequestBuilder::build_headers(token).await;
        let payload = ChatRequestBuilder::build_payload(
            message,
            model,
            mode,
            think,
            file_attachments,
            image_attachments,
            temperature,
            top_p,
            reasoning_effort,
            tool_overrides,
            model_config_override,
        )
        .await;
        let timeout: u64 = get_config("grok.timeout", 120u64).await;
        let proxy: String = get_config("grok.base_proxy_url", String::new()).await;
        let client = build_client(Some(&proxy), timeout).await?;
        let request = apply_headers(client.post(CHAT_API), &headers)
            .body(payload.to_string());

        let response = request
            .send()
            .await
            .map_err(|e| ApiError::upstream(format!("Chat request failed: {e}")))?;

        let status_code = response.status().as_u16();
        if status_code != 200 {
            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("<unknown>")
                .to_string();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| String::new());
            let preview = body_preview(&body, 220);
            if !preview.is_empty() {
                tracing::warn!(
                    "Chat error status={} content_type={} body={}",
                    status_code,
                    content_type,
                    preview
                );
            }
            return Err(ApiError::upstream(format!(
                "Grok API request failed: {status_code}; content-type: {content_type}; body: {preview}"
            )));
        }

        Ok(line_stream_from_response(response))
    }

    pub async fn chat_openai(
        &self,
        token: &str,
        request: &ChatRequest,
    ) -> Result<(LineStream, bool, String), ApiError> {
        let model_info = ModelService::get(&request.model)
            .ok_or_else(|| ApiError::invalid_request("Unknown model"))?;
        let is_video = model_info.is_video;
        let (message, attachments) = MessageExtractor::extract(&request.messages, is_video)?;

        let mut file_ids = Vec::new();
        if !attachments.is_empty() {
            let uploader = UploadService::new().await;
            for (_kind, data) in attachments {
                let (file_id, _) = uploader.upload(&data, token).await?;
                file_ids.push(file_id);
            }
        }
        let image_ids: Vec<String> = Vec::new();

        let stream = request
            .stream
            .unwrap_or(get_config("grok.stream", true).await);
        let think = request
            .think
            .or(Some(get_config("grok.thinking", false).await));

        let re_str = request.reasoning_effort.as_deref();

        let response = self
            .chat(
                token,
                &message,
                &model_info.grok_model,
                &model_info.model_mode,
                think,
                stream,
                &file_ids,
                &image_ids,
                request.temperature,
                request.top_p,
                re_str,
                None,
                None,
            )
            .await?;
        Ok((response, stream, request.model.clone()))
    }
}

pub struct ChatService;

impl ChatService {
    pub async fn completions(
        model: &str,
        messages: Vec<JsonValue>,
        stream: Option<bool>,
        thinking: Option<String>,
        temperature: Option<f64>,
        top_p: Option<f64>,
        reasoning_effort: Option<String>,
    ) -> Result<ChatResult, ApiError> {
        let token = TokenService::get_token_for_model(model).await?;

        // show_think 逻辑：reasoning_effort 优先，其次 thinking 字段
        let think = if let Some(ref re) = reasoning_effort {
            Some(re != "none")
        } else {
            match thinking.as_deref() {
                Some("enabled") => Some(true),
                Some("disabled") => Some(false),
                _ => None,
            }
        };

        let chat_req = ChatRequest {
            model: model.to_string(),
            messages,
            stream,
            think,
            temperature,
            top_p,
            reasoning_effort,
        };
        let service = GrokChatService::new().await;
        let (resp, is_stream, model_name) = service.chat_openai(&token, &chat_req).await?;
        Ok(ChatResult::Stream {
            stream: resp,
            token,
            model: model_name,
            is_stream,
            think,
        })
    }
}

pub type LineStream = Pin<Box<dyn Stream<Item = String> + Send>>;

#[allow(dead_code)]
pub enum ChatResult {
    Stream {
        stream: LineStream,
        token: String,
        model: String,
        is_stream: bool,
        think: Option<bool>,
    },
    Json(JsonValue),
}
