use std::convert::Infallible;
use std::sync::Arc;

use async_stream::stream;
use axum::extract::Query;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::SinkExt;
use futures::stream::{SplitSink, StreamExt};
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::{Value as JsonValue, json};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::core::auth::{verify_public_key, verify_stream_public_key};
use crate::core::config::get_config;
use crate::core::exceptions::ApiError;
use crate::core::public_session::SessionStore;
use crate::services::grok::model::ModelService;
use crate::services::token::TokenService;

#[derive(Clone)]
#[allow(dead_code)]
struct ImagineSession {
    prompt: String,
    aspect_ratio: String,
    nsfw: Option<bool>,
}

static SESSIONS: Lazy<SessionStore<ImagineSession>> = Lazy::new(SessionStore::new);

#[derive(Deserialize)]
struct ImagineStartRequest {
    prompt: String,
    aspect_ratio: Option<String>,
    nsfw: Option<bool>,
}

#[derive(Deserialize)]
struct ImagineStopRequest {
    task_ids: Vec<String>,
}

#[derive(Deserialize)]
struct StreamQuery {
    task_id: Option<String>,
    public_key: Option<String>,
}

pub fn router() -> Router {
    Router::new()
        .route("/v1/public/imagine/start", post(imagine_start))
        .route("/v1/public/imagine/stop", post(imagine_stop))
        .route("/v1/public/imagine/ws", get(imagine_ws))
        .route("/v1/public/imagine/sse", get(imagine_sse))
        .route("/v1/public/imagine/config", get(imagine_config))
}

async fn imagine_start(
    headers: HeaderMap,
    Json(req): Json<ImagineStartRequest>,
) -> Result<Response, ApiError> {
    verify_public_key(&headers).await?;
    let aspect_ratio = req.aspect_ratio.unwrap_or_else(|| "1:1".to_string());
    let session = ImagineSession {
        prompt: req.prompt,
        aspect_ratio: aspect_ratio.clone(),
        nsfw: req.nsfw,
    };
    let task_id = SESSIONS.create(session).await;
    Ok(Json(json!({ "task_id": task_id, "aspect_ratio": aspect_ratio })).into_response())
}

async fn imagine_stop(
    headers: HeaderMap,
    Json(req): Json<ImagineStopRequest>,
) -> Result<Response, ApiError> {
    verify_public_key(&headers).await?;
    let removed = SESSIONS.remove_many(&req.task_ids).await;
    Ok(Json(json!({ "status": "ok", "removed": removed })).into_response())
}

async fn imagine_config() -> Response {
    let nsfw: bool = get_config("downstream.enable_images_nsfw", true).await;
    Json(json!({
        "final_min_bytes": 50000,
        "nsfw": nsfw
    }))
    .into_response()
}

async fn generate_images(session: &ImagineSession) -> Result<Vec<JsonValue>, ApiError> {
    let use_nsfw = session.nsfw.unwrap_or(false);
    if use_nsfw {
        let nsfw_enabled: bool = get_config("downstream.enable_images_nsfw", true).await;
        if nsfw_enabled {
            let result =
                crate::services::grok::imagine_nsfw::generate(&session.prompt, None, Some(2), None)
                    .await;
            if result.success && !result.b64_list.is_empty() {
                return Ok(result
                    .b64_list
                    .into_iter()
                    .map(|b64| json!({ "b64_json": b64 }))
                    .collect());
            }
            if let Some(err) = result.error {
                return Err(ApiError::server(&err));
            }
        }
    }

    let model_id = "grok-imagine-1.0";
    let model_info = ModelService::get(model_id)
        .ok_or_else(|| ApiError::invalid_request("Imagine model not found"))?;
    let token = TokenService::get_token_for_model(model_id).await?;
    let images =
        crate::api::v1::image::call_grok_images_once(&token, &session.prompt, &model_info, true)
            .await?;
    Ok(images)
}

/// 向 WS sender 发送 JSON 消息的辅助函数
async fn ws_send(sender: &Arc<Mutex<SplitSink<WebSocket, Message>>>, msg: JsonValue) -> bool {
    let mut tx = sender.lock().await;
    tx.send(Message::Text(msg.to_string())).await.is_ok()
}

async fn imagine_ws(ws: WebSocketUpgrade, Query(q): Query<StreamQuery>) -> Response {
    ws.on_upgrade(move |socket| handle_ws(socket, q))
}

async fn handle_ws(socket: WebSocket, q: StreamQuery) {
    // 拆分 socket 为独立的 sender 和 receiver，避免死锁
    let (sender, mut receiver) = socket.split();
    let sender = Arc::new(Mutex::new(sender));

    // 认证：通过 task_id 或 public_key
    let session = if let Some(ref tid) = q.task_id {
        SESSIONS.get(tid).await
    } else {
        None
    };

    if session.is_none()
        && verify_stream_public_key(q.public_key.clone())
            .await
            .is_err()
    {
        let _ = ws_send(&sender, json!({"type":"error","message":"Unauthorized"})).await;
        let mut tx = sender.lock().await;
        let _ = tx.close().await;
        return;
    }

    // 当前生成任务的取消令牌
    let mut current_cancel: Option<CancellationToken> = None;

    // 主循环：只从 receiver 读消息，不阻塞 sender
    while let Some(msg) = receiver.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                let Ok(cmd) = serde_json::from_str::<JsonValue>(&text) else {
                    continue;
                };
                let cmd_type = cmd.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match cmd_type {
                    "start" => {
                        let prompt = cmd
                            .get("prompt")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        if prompt.is_empty() {
                            let _ = ws_send(
                                &sender,
                                json!({"type":"error","message":"prompt is required"}),
                            )
                            .await;
                            continue;
                        }

                        // 取消之前的生成任务
                        if let Some(cancel) = current_cancel.take() {
                            cancel.cancel();
                        }

                        let ar = cmd
                            .get("aspect_ratio")
                            .and_then(|v| v.as_str())
                            .unwrap_or("1:1")
                            .to_string();
                        let nsfw = cmd.get("nsfw").and_then(|v| v.as_bool());
                        let session = ImagineSession {
                            prompt,
                            aspect_ratio: ar,
                            nsfw,
                        };

                        let cancel = CancellationToken::new();
                        current_cancel = Some(cancel.clone());
                        let tx = sender.clone();

                        // 启动持续生成循环（独立任务，不阻塞消息接收）
                        tokio::spawn(async move {
                            ws_send(&tx, json!({"type":"status","message":"generating"})).await;
                            const MAX_CONSECUTIVE_ERRORS: u32 = 5;
                            let mut error_count: u32 = 0;

                            loop {
                                if cancel.is_cancelled() {
                                    break;
                                }

                                match generate_images(&session).await {
                                    Ok(images) => {
                                        error_count = 0;
                                        for img in images {
                                            if cancel.is_cancelled() {
                                                break;
                                            }
                                            if let Some(b64) =
                                                img.get("b64_json").and_then(|v| v.as_str())
                                                && !ws_send(
                                                    &tx,
                                                    json!({"type":"image","b64_json":b64}),
                                                )
                                                .await
                                            {
                                                return; // 发送失败，连接已断开
                                            }
                                        }
                                        if !ws_send(
                                            &tx,
                                            json!({"type":"status","message":"batch_done"}),
                                        )
                                        .await
                                        {
                                            return;
                                        }
                                    }
                                    Err(e) => {
                                        error_count += 1;
                                        let _ = ws_send(
                                            &tx,
                                            json!({"type":"error","message":e.to_string()}),
                                        )
                                        .await;
                                        if error_count >= MAX_CONSECUTIVE_ERRORS {
                                            let _ = ws_send(
                                                &tx,
                                                json!({"type":"error","message":"Too many consecutive errors, stopping"}),
                                            )
                                            .await;
                                            break;
                                        }
                                        // 出错后短暂等待再重试
                                        tokio::time::sleep(std::time::Duration::from_millis(1500))
                                            .await;
                                    }
                                }
                            }
                        });
                    }
                    "stop" => {
                        if let Some(cancel) = current_cancel.take() {
                            cancel.cancel();
                        }
                        let _ =
                            ws_send(&sender, json!({"type":"status","message":"stopped"})).await;
                    }
                    _ => {}
                }
            }
            Ok(Message::Close(_)) | Err(_) => break,
            _ => {}
        }
    }

    // 连接关闭，取消生成任务
    if let Some(cancel) = current_cancel.take() {
        cancel.cancel();
    }
}

async fn imagine_sse(
    Query(q): Query<StreamQuery>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    // 认证
    let session = if let Some(ref tid) = q.task_id {
        SESSIONS.get(tid).await
    } else {
        None
    };

    let session = if let Some(s) = session {
        s
    } else {
        verify_stream_public_key(q.public_key).await?;
        return Err(ApiError::invalid_request("task_id is required for SSE"));
    };

    let task_id = q.task_id.clone().unwrap_or_default();

    let s = stream! {
        yield Ok(Event::default().data(json!({"type":"status","message":"generating"}).to_string()));
        const MAX_CONSECUTIVE_ERRORS: u32 = 5;
        let mut error_count: u32 = 0;

        loop {
            // 检查 session 是否仍然存在（被 stop 接口删除则停止）
            if !task_id.is_empty() && SESSIONS.get(&task_id).await.is_none() {
                yield Ok(Event::default().data(json!({"type":"status","message":"stopped"}).to_string()));
                break;
            }

            match generate_images(&session).await {
                Ok(images) => {
                    error_count = 0;
                    for img in images {
                        if let Some(b64) = img.get("b64_json").and_then(|v| v.as_str()) {
                            yield Ok(Event::default().data(json!({"type":"image","b64_json":b64}).to_string()));
                        }
                    }
                    yield Ok(Event::default().data(json!({"type":"status","message":"batch_done"}).to_string()));
                }
                Err(e) => {
                    error_count += 1;
                    yield Ok(Event::default().data(json!({"type":"error","message":e.to_string()}).to_string()));
                    if error_count >= MAX_CONSECUTIVE_ERRORS {
                        yield Ok(Event::default().data(json!({"type":"error","message":"Too many consecutive errors, stopping"}).to_string()));
                        break;
                    }
                    // 出错后短暂等待再重试
                    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                }
            }
        }
    };

    Ok(Sse::new(s))
}
