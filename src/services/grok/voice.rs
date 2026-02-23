use serde_json::{Value as JsonValue, json};

use crate::core::config::get_config;
use crate::core::exceptions::ApiError;
use crate::services::grok::headers::build_grok_headers;
use crate::services::grok::wreq_client::{self, body_preview};

const LIVEKIT_TOKEN_API: &str = "https://grok.com/rest/livekit/tokens";
const LIVEKIT_WS_URL: &str = "wss://livekit.grok.com";

pub struct VoiceService;

impl VoiceService {
    /// 请求 LiveKit Token
    pub async fn get_token(
        token: &str,
        voice: &str,
        personality: &str,
        speed: f64,
    ) -> Result<JsonValue, ApiError> {
        let headers =
            build_grok_headers(token, Some("application/json"), Some("https://grok.com/")).await;

        let session_payload = json!({
            "voice": voice,
            "personality": personality,
            "playback_speed": speed,
            "enable_vision": false,
            "turn_detection": { "type": "server_vad" }
        });

        let payload = json!({
            "sessionPayload": session_payload.to_string(),
            "requestAgentDispatch": false,
            "livekitUrl": LIVEKIT_WS_URL,
            "params": { "enable_markdown_transcript": "true" }
        });

        let timeout: u64 = get_config("grok.timeout", 30u64).await;
        let proxy: String = get_config("grok.base_proxy_url", String::new()).await;
        let proxy_opt = if proxy.is_empty() {
            None
        } else {
            Some(proxy.as_str())
        };

        let client = wreq_client::build_client(proxy_opt, timeout).await?;
        let mut builder = client.post(LIVEKIT_TOKEN_API);
        builder = wreq_client::apply_headers(builder, &headers);
        builder = builder.body(payload.to_string());

        let response = builder.send().await.map_err(|e| {
            tracing::error!("VoiceService: LiveKit token request failed: {e}");
            ApiError::server(format!("Voice token request failed: {e}"))
        })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let preview = body_preview(&body, 200);
            tracing::error!(
                "VoiceService: LiveKit returned {status}, body={preview}"
            );
            return Err(ApiError::server(format!("LiveKit returned {status}")));
        }

        let data: JsonValue = response.json().await.map_err(|e| {
            tracing::error!("VoiceService: failed to parse LiveKit response: {e}");
            ApiError::server("Failed to parse voice token response")
        })?;

        Ok(data)
    }
}
