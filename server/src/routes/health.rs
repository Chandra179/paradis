//! Health and readiness endpoints.

use axum::{Json, extract::State};
use serde::Serialize;
use sqlx::SqlitePool;

use crate::error::AppError;

/// Response for `GET /health`.
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    /// Fixed status string `"ok"`.
    pub status: &'static str,
}

/// Response for `GET /ready`.
#[derive(Debug, Serialize)]
pub struct ReadinessResponse {
    /// Fixed status string `"ready"`.
    pub status: &'static str,
}

/// Liveness probe.
pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

/// Database readiness probe.
pub async fn ready(State(pool): State<SqlitePool>) -> Result<Json<ReadinessResponse>, AppError> {
    sqlx::query("SELECT 1").execute(&pool).await?;
    Ok(Json(ReadinessResponse { status: "ready" }))
}
