use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AppError;
use crate::models::scent::Scent;
use crate::AppState;

pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<Scent>>, AppError> {
    let scents = sqlx::query_as::<_, Scent>("select * from scents order by name")
        .fetch_all(&state.db)
        .await?;
    Ok(Json(scents))
}

#[derive(Deserialize)]
pub struct CreateScentRequest {
    pub name: String,
}

pub async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateScentRequest>,
) -> Result<Json<Scent>, AppError> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("name is required".into()));
    }

    let scent = sqlx::query_as::<_, Scent>("insert into scents (name) values ($1) returning *")
        .bind(name)
        .fetch_one(&state.db)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(db_err) if db_err.is_unique_violation() => {
                AppError::Conflict("a scent with this name already exists".into())
            }
            _ => AppError::from(e),
        })?;

    Ok(Json(scent))
}

#[derive(Deserialize)]
pub struct UpdateScentRequest {
    pub name: Option<String>,
    pub active: Option<bool>,
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateScentRequest>,
) -> Result<Json<Scent>, AppError> {
    let scent = sqlx::query_as::<_, Scent>(
        "update scents set name = coalesce($2, name), active = coalesce($3, active) where id = $1 returning *",
    )
    .bind(id)
    .bind(body.name)
    .bind(body.active)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("scent not found".into()))?;

    Ok(Json(scent))
}

/// Used by the intake handler to validate a `set_perfume` order's `scent_id`.
pub async fn assert_active_scent(pool: &PgPool, id: Uuid) -> Result<(), AppError> {
    let exists: bool =
        sqlx::query_scalar("select exists(select 1 from scents where id = $1 and active = true)")
            .bind(id)
            .fetch_one(pool)
            .await?;

    if !exists {
        return Err(AppError::BadRequest(
            "scent_id is invalid or inactive".into(),
        ));
    }

    Ok(())
}
