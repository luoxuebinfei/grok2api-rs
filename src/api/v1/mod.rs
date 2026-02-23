mod admin;
mod chat;
mod files;
mod image;
mod models;
mod public;
mod responses;

use axum::Router;

pub fn router() -> Router {
    Router::new()
        .merge(chat::router())
        .merge(responses::router())
        .merge(image::router())
        .merge(models::router())
        .merge(files::router())
        .merge(admin::router())
        .merge(public::router())
}
