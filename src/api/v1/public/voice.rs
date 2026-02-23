use axum::extract::Query;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::core::auth::verify_public_key;
use crate::core::exceptions::ApiError;
use crate::services::grok::voice::VoiceService;
use crate::services::token::get_token_manager;

#[derive(Deserialize)]
struct VoiceTokenQuery {
    voice: Option<String>,
    personality: Option<String>,
    speed: Option<f64>,
}

pub fn router() -> Router {
    Router::new().route("/v1/public/voice/token", get(voice_token))
}

async fn voice_token(
    headers: HeaderMap,
    Query(q): Query<VoiceTokenQuery>,
) -> Result<Response, ApiError> {
    verify_public_key(&headers).await?;

    let voice = q.voice.as_deref().unwrap_or("ara");
    let personality = q.personality.as_deref().unwrap_or("assistant");
    let speed = q.speed.unwrap_or(1.0).clamp(0.5, 2.0);

    // 获取 SSO token（优先 ssoBasic，回退 ssoSuper）
    let mgr = get_token_manager().await;
    let mgr = mgr.lock().await;
    let sso_token = mgr
        .get_token("ssoBasic")
        .or_else(|| mgr.get_token("ssoSuper"));
    drop(mgr);

    let sso_token =
        sso_token.ok_or_else(|| ApiError::rate_limit("No available tokens for voice mode"))?;

    let data = VoiceService::get_token(&sso_token, voice, personality, speed).await?;

    let token = data.get("token").and_then(|v| v.as_str()).unwrap_or("");
    if token.is_empty() {
        return Err(ApiError::server("Upstream returned no voice token"));
    }

    let resp = json!({
        "token": token,
        "url": "wss://livekit.grok.com",
        "participant_name": "",
        "room_name": ""
    });

    Ok(Json(resp).into_response())
}
