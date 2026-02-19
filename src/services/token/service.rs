use crate::core::exceptions::ApiError;
use crate::services::grok::model::ModelService;
use crate::services::token::manager::get_token_manager;
use crate::services::token::models::EffortType;

pub struct TokenService;

impl TokenService {
    pub async fn get_token_for_model(model: &str) -> Result<String, ApiError> {
        let candidates = ModelService::pool_candidates_for_model(model);
        let mgr = get_token_manager().await;
        let mut mgr = mgr.lock().await;
        mgr.reload_if_stale().await;

        for pool in &candidates {
            if let Some(token) = mgr.get_token(pool) {
                return Ok(token);
            }
        }

        Err(ApiError::rate_limit(
            "No available tokens. Please try again later.",
        ))
    }

    pub async fn consume(token: &str, effort: EffortType) -> bool {
        let mgr = get_token_manager().await;
        let mut mgr = mgr.lock().await;
        mgr.consume(token, effort).await
    }
}
