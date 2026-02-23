use axum::http::HeaderMap;

use crate::core::config::get_config;
use crate::core::exceptions::ApiError;

fn extract_bearer(headers: &HeaderMap) -> Option<String> {
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    if let Some(rest) = auth.strip_prefix("Bearer ") {
        return Some(rest.trim().to_string());
    }
    None
}

/// key 为空时的策略
enum EmptyKeyPolicy {
    /// key 为空则放行（如 api_key 未配置时不鉴权）
    Allow,
    /// key 为空则拒绝（如 app_key 必须配置）
    Deny,
    /// key 为空时根据额外开关决定（如 public_key + public_enabled）
    Switch(bool),
}

/// 通用 Bearer Header 鉴权
fn verify_bearer(headers: &HeaderMap, key: &str, policy: &EmptyKeyPolicy) -> Result<(), ApiError> {
    if key.is_empty() {
        return match policy {
            EmptyKeyPolicy::Allow => Ok(()),
            EmptyKeyPolicy::Deny => Err(ApiError::authentication("App key is not configured")),
            EmptyKeyPolicy::Switch(enabled) => {
                if *enabled {
                    Ok(())
                } else {
                    Err(ApiError::authentication("Public access is disabled"))
                }
            }
        };
    }
    match extract_bearer(headers) {
        Some(token) if token == key => Ok(()),
        Some(_) => Err(ApiError::authentication("Invalid authentication token")),
        None => Err(ApiError::authentication("Missing authentication token")),
    }
}

/// 通用 Query 参数鉴权
fn verify_query(query_key: Option<String>, key: &str, policy: &EmptyKeyPolicy) -> Result<(), ApiError> {
    if key.is_empty() {
        return match policy {
            EmptyKeyPolicy::Allow => Ok(()),
            EmptyKeyPolicy::Deny => Err(ApiError::authentication("App key is not configured")),
            EmptyKeyPolicy::Switch(enabled) => {
                if *enabled {
                    Ok(())
                } else {
                    Err(ApiError::authentication("Public access is disabled"))
                }
            }
        };
    }
    if let Some(q) = query_key {
        let raw = q
            .strip_prefix("Bearer ")
            .map(|v| v.trim())
            .unwrap_or(q.trim());
        if raw == key {
            return Ok(());
        }
    }
    Err(ApiError::authentication("Invalid authentication token"))
}

pub async fn verify_api_key(headers: &HeaderMap) -> Result<(), ApiError> {
    let api_key: String = get_config("app.api_key", String::new()).await;
    verify_bearer(headers, &api_key, &EmptyKeyPolicy::Allow)
}

pub async fn verify_app_key(headers: &HeaderMap) -> Result<(), ApiError> {
    let app_key: String = get_config("app.app_key", String::new()).await;
    verify_bearer(headers, &app_key, &EmptyKeyPolicy::Deny)
}

pub async fn verify_stream_api_key(query_key: Option<String>) -> Result<(), ApiError> {
    let api_key: String = get_config("app.api_key", String::new()).await;
    verify_query(query_key, &api_key, &EmptyKeyPolicy::Allow)
}

pub async fn verify_public_key(headers: &HeaderMap) -> Result<(), ApiError> {
    let public_enabled: bool = get_config("app.public_enabled", false).await;
    let public_key: String = get_config("app.public_key", String::new()).await;
    verify_bearer(headers, &public_key, &EmptyKeyPolicy::Switch(public_enabled))
}

pub async fn verify_stream_public_key(query_key: Option<String>) -> Result<(), ApiError> {
    let public_enabled: bool = get_config("app.public_enabled", false).await;
    let public_key: String = get_config("app.public_key", String::new()).await;
    verify_query(query_key, &public_key, &EmptyKeyPolicy::Switch(public_enabled))
}

pub async fn verify_any_key(headers: &HeaderMap) -> Result<(), ApiError> {
    if verify_api_key(headers).await.is_ok() {
        return Ok(());
    }
    if verify_public_key(headers).await.is_ok() {
        return Ok(());
    }
    Err(ApiError::authentication("Invalid authentication token"))
}
