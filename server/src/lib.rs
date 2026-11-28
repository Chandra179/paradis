//! Lux server library exposing the Axum router and shared configuration/database helpers.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod config;
pub mod db;
pub mod error;
pub mod routes;

use axum::{
    Router,
    routing::{get, post},
};
use sqlx::SqlitePool;

/// Builds the HTTP router backed by `pool`.
pub fn app(pool: SqlitePool) -> Router {
    Router::new()
        .route("/health", get(routes::health::health))
        .route("/ready", get(routes::health::ready))
        .route("/documents", get(routes::documents::list))
        .route("/documents/extract", post(routes::documents::extract))
        .route("/documents/{id}", get(routes::documents::get))
        .route("/reviews", post(routes::documents::review))
        .with_state(pool)
}
