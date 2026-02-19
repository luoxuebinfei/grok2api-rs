use std::time::Duration;

use serde_json::Value as JsonValue;

use crate::core::config::get_config;
use crate::core::exceptions::ApiError;
use crate::services::grok::headers::build_grok_headers;
use crate::services::grok::wreq_client::{apply_headers, build_client_with_emulation};

const LIMITS_API: &str = "https://grok.com/rest/rate-limits";

pub struct UsageService;

impl UsageService {
    pub async fn new() -> Self {
        Self
    }

    async fn build_headers(&self, token: &str) -> reqwest::header::HeaderMap {
        build_grok_headers(token, None, None).await
    }

    async fn get_via_wreq(&self, token: &str, model_name: &str) -> Result<JsonValue, ApiError> {
        let headers = self.build_headers(token).await;
        let payload = serde_json::json!({
            "requestKind": "DEFAULT",
            "modelName": model_name,
        });
        let timeout: u64 = get_config("grok.timeout", 10u64).await;
        let proxy: String = get_config("grok.base_proxy_url", String::new()).await;
        let usage_emulation: String = get_config("grok.wreq_emulation_usage", String::new()).await;
        let emulation_override = if usage_emulation.trim().is_empty() {
            None
        } else {
            Some(usage_emulation.trim())
        };

        let client = build_client_with_emulation(Some(&proxy), timeout, emulation_override).await?;
        let response = apply_headers(client.post(LIMITS_API), &headers)
            .timeout(Duration::from_secs(timeout.max(1)))
            .body(payload.to_string())
            .send()
            .await
            .map_err(|e| ApiError::upstream(format!("Usage request failed: {e}")))?;

        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("<unknown>")
            .to_string();
        let bytes = response
            .bytes()
            .await
            .map_err(|e| ApiError::upstream(format!("Usage response read failed: {e}")))?;

        let body_text = String::from_utf8_lossy(&bytes).to_string();

        if status != 200 {
            let preview = body_preview(&body_text);
            return Err(ApiError::upstream(format!(
                "Failed to get usage stats: {status}; content-type: {content_type}; body: {preview}"
            )));
        }

        let normalized = normalize_json_text(&body_text);
        if normalized.is_empty() {
            return Err(ApiError::upstream(format!(
                "Usage parse error: empty response body; content-type: {content_type}"
            )));
        }

        serde_json::from_str(&normalized).map_err(|e| {
            let preview = body_preview(&normalized);
            let compression_hint = if bytes.starts_with(&[0x1f, 0x8b]) {
                "; hint: upstream body still looks gzip-compressed"
            } else {
                ""
            };
            ApiError::upstream(format!(
                "Usage parse error: {e}; content-type: {content_type}; body: {preview}{compression_hint}"
            ))
        })
    }

    pub async fn get(&self, token: &str, model_name: &str) -> Result<JsonValue, ApiError> {
        self.get_via_wreq(token, model_name).await
    }
}

fn normalize_json_text(raw: &str) -> String {
    let mut text = raw.trim_start_matches('\u{feff}').trim_start().to_string();

    if text.starts_with(")]}'") {
        if let Some((_, rest)) = text.split_once('\n') {
            text = rest.trim_start().to_string();
        }
    }

    if text.starts_with("for (;;);") {
        text = text.trim_start_matches("for (;;);").trim_start().to_string();
    }

    text.trim().to_string()
}

fn body_preview(text: &str) -> String {
    text.chars()
        .take(200)
        .flat_map(|ch| ch.escape_default())
        .collect::<String>()
}
