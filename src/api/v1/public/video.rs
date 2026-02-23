use std::convert::Infallible;
use std::pin::Pin;

use async_stream::stream;
use axum::extract::Query;
use axum::http::HeaderMap;
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::StreamExt;
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::json;

use crate::core::auth::verify_public_key;
use crate::core::auth::verify_stream_public_key;
use crate::core::exceptions::ApiError;
use crate::core::public_session::SessionStore;
use crate::services::grok::media::{VideoResult, VideoService};
use crate::services::grok::processor::VideoStreamProcessor;

#[derive(Clone)]
struct VideoSession {
    prompt: String,
    aspect_ratio: String,
    video_length: i32,
    resolution_name: String,
    preset: String,
    image_url: Option<String>,
    reasoning_effort: Option<String>,
}

static SESSIONS: Lazy<SessionStore<VideoSession>> = Lazy::new(SessionStore::new);

#[derive(Deserialize)]
struct VideoStartRequest {
    prompt: String,
    aspect_ratio: Option<String>,
    video_length: Option<i32>,
    resolution_name: Option<String>,
    preset: Option<String>,
    image_url: Option<String>,
    reasoning_effort: Option<String>,
}

#[derive(Deserialize)]
struct VideoStopRequest {
    task_ids: Vec<String>,
}

#[derive(Deserialize)]
struct StreamQuery {
    task_id: Option<String>,
    public_key: Option<String>,
}

pub fn router() -> Router {
    Router::new()
        .route("/v1/public/video/start", post(video_start))
        .route("/v1/public/video/stop", post(video_stop))
        .route("/v1/public/video/sse", get(video_sse))
}

fn normalize_aspect_ratio(input: &str) -> String {
    match input {
        "1280x720" | "720x1280" => {
            if input == "1280x720" {
                "16:9".to_string()
            } else {
                "9:16".to_string()
            }
        }
        r @ ("3:2" | "2:3" | "16:9" | "9:16" | "1:1") => r.to_string(),
        _ => "16:9".to_string(),
    }
}

fn normalize_resolution(name: &str) -> &'static str {
    match name.to_lowercase().as_str() {
        "480p" | "sd" => "SD",
        "720p" | "hd" => "HD",
        _ => "HD",
    }
}

async fn video_start(
    headers: HeaderMap,
    Json(req): Json<VideoStartRequest>,
) -> Result<Response, ApiError> {
    verify_public_key(&headers).await?;

    let aspect_ratio = normalize_aspect_ratio(req.aspect_ratio.as_deref().unwrap_or("16:9"));
    let video_length = match req.video_length.unwrap_or(6) {
        6 | 10 | 15 => req.video_length.unwrap_or(6),
        _ => 6,
    };
    let resolution_name = req.resolution_name.as_deref().unwrap_or("720p").to_string();
    let preset = req.preset.as_deref().unwrap_or("normal").to_string();

    let session = VideoSession {
        prompt: req.prompt,
        aspect_ratio: aspect_ratio.clone(),
        video_length,
        resolution_name,
        preset,
        image_url: req.image_url,
        reasoning_effort: req.reasoning_effort,
    };

    let task_id = SESSIONS.create(session).await;
    Ok(Json(json!({ "task_id": task_id, "aspect_ratio": aspect_ratio })).into_response())
}

async fn video_stop(
    headers: HeaderMap,
    Json(req): Json<VideoStopRequest>,
) -> Result<Response, ApiError> {
    verify_public_key(&headers).await?;
    let removed = SESSIONS.remove_many(&req.task_ids).await;
    Ok(Json(json!({ "status": "ok", "removed": removed })).into_response())
}

async fn video_sse(
    Query(q): Query<StreamQuery>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let session = if let Some(ref tid) = q.task_id {
        SESSIONS.get(tid).await
    } else {
        None
    };

    let session = if let Some(s) = session {
        s
    } else {
        verify_stream_public_key(q.public_key).await?;
        return Err(ApiError::invalid_request("task_id is required"));
    };

    // 构建消息
    let mut messages = Vec::new();
    let user_content = session.prompt.clone();

    if let Some(ref img_url) = session.image_url {
        // 含参考图
        let content = json!([
            { "type": "text", "text": user_content },
            { "type": "image_url", "image_url": { "url": img_url } }
        ]);
        messages.push(json!({ "role": "user", "content": content }));
    } else {
        messages.push(json!({ "role": "user", "content": user_content }));
    }

    let resolution = normalize_resolution(&session.resolution_name);
    let thinking = session.reasoning_effort.clone();

    let result = VideoService::completions(
        "grok-imagine-1.0-video",
        messages,
        Some(true),
        thinking,
        &session.aspect_ratio,
        session.video_length,
        resolution,
        &session.preset,
    )
    .await;

    match result {
        Ok(VideoResult::Stream {
            stream: line_stream,
            token,
            model,
            think,
            is_stream: _,
            upscale_on_finish,
        }) => {
            let processor =
                VideoStreamProcessor::new(&model, &token, think, upscale_on_finish).await;
            let event_stream = processor.process(line_stream);

            let s = stream! {
                tokio::pin!(event_stream);
                while let Some(chunk) = event_stream.next().await {
                    let bytes = chunk.unwrap_or_default();
                    let text = String::from_utf8_lossy(&bytes);
                    if text.contains("data: ") {
                        for line in text.lines() {
                            if let Some(data) = line.strip_prefix("data: ") {
                                yield Ok::<_, Infallible>(Event::default().data(data));
                            }
                        }
                    } else if !text.trim().is_empty() {
                        yield Ok::<_, Infallible>(Event::default().data(&*text));
                    }
                }
                yield Ok::<_, Infallible>(Event::default().data("[DONE]"));
            };

            let boxed: Pin<Box<dyn futures::Stream<Item = Result<Event, Infallible>> + Send>> =
                Box::pin(s);
            Ok(Sse::new(boxed))
        }
        Ok(VideoResult::Json(value)) => {
            let s = stream! {
                yield Ok::<_, Infallible>(Event::default().data(value.to_string()));
                yield Ok::<_, Infallible>(Event::default().data("[DONE]"));
            };
            let boxed: Pin<Box<dyn futures::Stream<Item = Result<Event, Infallible>> + Send>> =
                Box::pin(s);
            Ok(Sse::new(boxed))
        }
        Err(e) => Err(e),
    }
}
