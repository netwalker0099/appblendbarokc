use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::Json;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AppError;
use crate::models::scent::{Scent, ScentItem};
use crate::routes::ingredients::assert_active_ingredients;
use crate::routes::mixes::MixItemInput;
use crate::AppState;

#[derive(Serialize)]
pub struct ScentDetail {
    #[serde(flatten)]
    pub scent: Scent,
    pub items: Vec<ScentItem>,
}

pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<ScentDetail>>, AppError> {
    let scents = sqlx::query_as::<_, Scent>("select * from scents order by name")
        .fetch_all(&state.db)
        .await?;

    // One query for every scent's items, then bucket — no per-scent round trip.
    let scent_ids: Vec<Uuid> = scents.iter().map(|s| s.id).collect();
    let items = sqlx::query_as::<_, ScentItem>("select * from scent_items where scent_id = any($1)")
        .bind(&scent_ids)
        .fetch_all(&state.db)
        .await?;

    let mut by_scent: HashMap<Uuid, Vec<ScentItem>> = HashMap::new();
    for item in items {
        by_scent.entry(item.scent_id).or_default().push(item);
    }
    let details = scents
        .into_iter()
        .map(|scent| {
            let items = by_scent.remove(&scent.id).unwrap_or_default();
            ScentDetail { scent, items }
        })
        .collect();

    Ok(Json(details))
}

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<ScentDetail>, AppError> {
    let detail = fetch_scent_detail(&state.db, id)
        .await?
        .ok_or_else(|| AppError::NotFound("scent not found".into()))?;
    Ok(Json(detail))
}

pub async fn fetch_scent_detail(pool: &PgPool, id: Uuid) -> Result<Option<ScentDetail>, AppError> {
    let Some(scent) = sqlx::query_as::<_, Scent>("select * from scents where id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await?
    else {
        return Ok(None);
    };
    let items = sqlx::query_as::<_, ScentItem>("select * from scent_items where scent_id = $1")
        .bind(id)
        .fetch_all(pool)
        .await?;
    Ok(Some(ScentDetail { scent, items }))
}

/// Validates a scent's formula: an empty formula is allowed (recipe not defined
/// yet); a non-empty one must be at most 8 distinct active ingredients with
/// positive amounts — the same rule mixes use.
async fn validate_formula(pool: &PgPool, items: &[MixItemInput]) -> Result<(), AppError> {
    if items.is_empty() {
        return Ok(());
    }
    let ids: Vec<Uuid> = items.iter().map(|i| i.ingredient_id).collect();
    assert_active_ingredients(pool, &ids).await?;
    for item in items {
        if item.amount_ml <= Decimal::ZERO {
            return Err(AppError::BadRequest("amount_ml must be positive".into()));
        }
    }
    Ok(())
}

async fn replace_items(
    tx: &mut sqlx::PgConnection,
    scent_id: Uuid,
    items: &[MixItemInput],
) -> Result<(), AppError> {
    sqlx::query("delete from scent_items where scent_id = $1")
        .bind(scent_id)
        .execute(&mut *tx)
        .await?;
    for item in items {
        sqlx::query(
            "insert into scent_items (scent_id, ingredient_id, amount_ml) values ($1, $2, $3)",
        )
        .bind(scent_id)
        .bind(item.ingredient_id)
        .bind(item.amount_ml)
        .execute(&mut *tx)
        .await?;
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct CreateScentRequest {
    pub name: String,
    pub items: Option<Vec<MixItemInput>>,
}

pub async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateScentRequest>,
) -> Result<Json<ScentDetail>, AppError> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("name is required".into()));
    }
    let items = body.items.unwrap_or_default();
    validate_formula(&state.db, &items).await?;

    let mut tx = state.db.begin().await?;
    let scent = sqlx::query_as::<_, Scent>("insert into scents (name) values ($1) returning *")
        .bind(name)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(db_err) if db_err.is_unique_violation() => {
                AppError::Conflict("a scent with this name already exists".into())
            }
            _ => AppError::from(e),
        })?;
    replace_items(&mut tx, scent.id, &items).await?;
    tx.commit().await?;

    let detail = fetch_scent_detail(&state.db, scent.id)
        .await?
        .ok_or_else(|| AppError::Internal("scent vanished after create".into()))?;
    Ok(Json(detail))
}

#[derive(Deserialize)]
pub struct UpdateScentRequest {
    pub name: Option<String>,
    pub active: Option<bool>,
    pub items: Option<Vec<MixItemInput>>,
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateScentRequest>,
) -> Result<Json<ScentDetail>, AppError> {
    if let Some(items) = &body.items {
        validate_formula(&state.db, items).await?;
    }

    let mut tx = state.db.begin().await?;
    sqlx::query_as::<_, Scent>(
        "update scents set name = coalesce($2, name), active = coalesce($3, active) where id = $1 returning *",
    )
    .bind(id)
    .bind(&body.name)
    .bind(body.active)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::NotFound("scent not found".into()))?;

    if let Some(items) = &body.items {
        replace_items(&mut tx, id, items).await?;
    }
    tx.commit().await?;

    let detail = fetch_scent_detail(&state.db, id)
        .await?
        .ok_or_else(|| AppError::Internal("scent vanished after update".into()))?;
    Ok(Json(detail))
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
