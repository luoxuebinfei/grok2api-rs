pub mod imagine;
pub mod video;
pub mod voice;

use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;

use crate::core::auth::verify_public_key;
use crate::core::config::get_config;
use crate::core::exceptions::ApiError;
use crate::core::static_assets;

pub fn router() -> Router {
    Router::new()
        .route("/", get(root_redirect))
        .route("/login", get(public_login_page))
        .route("/chat", get(public_chat_page))
        .route("/imagine", get(public_imagine_page))
        .route("/video", get(public_video_page))
        .route("/voice", get(public_voice_page))
        .route("/v1/public/verify", get(verify_endpoint))
        .merge(imagine::router())
        .merge(video::router())
        .merge(voice::router())
}

async fn root_redirect() -> Response {
    let public_enabled: bool = get_config("app.public_enabled", false).await;
    if public_enabled {
        Redirect::temporary("/login").into_response()
    } else {
        Redirect::temporary("/admin").into_response()
    }
}

async fn render_public_template(path: &str) -> Response {
    let public_enabled: bool = get_config("app.public_enabled", false).await;
    if !public_enabled {
        return (StatusCode::NOT_FOUND, "Not Found").into_response();
    }
    if let Some(content) = static_assets::get_text(path) {
        return Html(content).into_response();
    }
    let file_path = crate::core::config::project_root()
        .join("static")
        .join(path);
    if let Ok(content) = tokio::fs::read_to_string(&file_path).await {
        return Html(content).into_response();
    }
    (StatusCode::NOT_FOUND, format!("Template {path} not found.")).into_response()
}

async fn public_login_page() -> Response {
    render_public_template("public/login/login.html").await
}

async fn public_chat_page() -> Response {
    render_public_template("public/chat/chat.html").await
}

async fn public_imagine_page() -> Response {
    render_public_template("public/imagine/imagine.html").await
}

async fn public_video_page() -> Response {
    render_public_template("public/video/video.html").await
}

async fn public_voice_page() -> Response {
    render_public_template("public/voice/voice.html").await
}

async fn verify_endpoint(headers: HeaderMap) -> Result<Response, ApiError> {
    verify_public_key(&headers).await?;
    Ok(Json(json!({"status": "ok"})).into_response())
}
