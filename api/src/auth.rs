use axum::extract::{Request, State};
use axum::http::header::AUTHORIZATION;
use axum::middleware::Next;
use axum::response::Response;
use rand::RngCore;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AppError;
use crate::AppState;

#[derive(Debug, Clone)]
pub struct Device {
    pub id: Uuid,
    #[allow(dead_code)] // available to handlers for future audit logging
    pub label: String,
}

pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("bb_{}", hex::encode(bytes))
}

pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

/// Generates a new device token, stores its hash, and returns the raw token.
/// The raw token is never persisted or logged — this is the only time it's visible.
pub async fn issue_device_token(pool: &PgPool, label: &str) -> Result<String, sqlx::Error> {
    let token = generate_token();
    let token_hash = hash_token(&token);

    sqlx::query("insert into operator_devices (label, token_hash) values ($1, $2)")
        .bind(label)
        .bind(&token_hash)
        .execute(pool)
        .await?;

    Ok(token)
}

pub async fn require_operator_token(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, AppError> {
    let token = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(AppError::Unauthorized)?
        .to_string();

    let token_hash = hash_token(&token);

    let device = sqlx::query_as::<_, (Uuid, String)>(
        "select id, label from operator_devices where token_hash = $1 and active = true",
    )
    .bind(&token_hash)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::Unauthorized)?;

    sqlx::query("update operator_devices set last_used_at = now() where id = $1")
        .bind(device.0)
        .execute(&state.db)
        .await?;

    req.extensions_mut().insert(Device {
        id: device.0,
        label: device.1,
    });

    Ok(next.run(req).await)
}
