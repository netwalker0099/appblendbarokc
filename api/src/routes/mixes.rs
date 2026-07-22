use axum::extract::{Path, State};
use axum::Json;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AppError;
use crate::models::mix::{Mix, MixItem};
use crate::routes::ingredients::assert_active_ingredients;
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct MixItemInput {
    pub ingredient_id: Uuid,
    pub amount_ml: Decimal,
}

#[derive(Serialize)]
pub struct MixDetail {
    #[serde(flatten)]
    pub mix: Mix,
    pub items: Vec<MixItem>,
}

pub async fn list_for_customer(
    State(state): State<AppState>,
    Path(customer_id): Path<Uuid>,
) -> Result<Json<Vec<Mix>>, AppError> {
    let mixes = sqlx::query_as::<_, Mix>(
        "select * from mixes where customer_id = $1 order by created_at desc",
    )
    .bind(customer_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(mixes))
}

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<MixDetail>, AppError> {
    let detail = fetch_mix_detail(&state.db, id)
        .await?
        .ok_or_else(|| AppError::NotFound("mix not found".into()))?;
    Ok(Json(detail))
}

pub async fn fetch_mix_detail(pool: &PgPool, id: Uuid) -> Result<Option<MixDetail>, AppError> {
    let Some(mix) = sqlx::query_as::<_, Mix>("select * from mixes where id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await?
    else {
        return Ok(None);
    };

    let items = sqlx::query_as::<_, MixItem>("select * from mix_items where mix_id = $1")
        .bind(id)
        .fetch_all(pool)
        .await?;

    Ok(Some(MixDetail { mix, items }))
}

#[derive(Deserialize)]
pub struct UpdateMixRequest {
    pub name: Option<String>,
    pub items: Option<Vec<MixItemInput>>,
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateMixRequest>,
) -> Result<Json<MixDetail>, AppError> {
    if let Some(items) = &body.items {
        let ids: Vec<Uuid> = items.iter().map(|i| i.ingredient_id).collect();
        assert_active_ingredients(&state.db, &ids).await?;
        for item in items {
            if item.amount_ml <= Decimal::ZERO {
                return Err(AppError::BadRequest("amount_ml must be positive".into()));
            }
        }
    }

    let mut tx = state.db.begin().await?;

    sqlx::query_as::<_, Mix>("update mixes set name = coalesce($2, name) where id = $1 returning *")
        .bind(id)
        .bind(&body.name)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| AppError::NotFound("mix not found".into()))?;

    if let Some(items) = &body.items {
        sqlx::query("delete from mix_items where mix_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;

        for item in items {
            sqlx::query(
                "insert into mix_items (mix_id, ingredient_id, amount_ml) values ($1, $2, $3)",
            )
            .bind(id)
            .bind(item.ingredient_id)
            .bind(item.amount_ml)
            .execute(&mut *tx)
            .await?;
        }
    }

    tx.commit().await?;

    let detail = fetch_mix_detail(&state.db, id)
        .await?
        .ok_or_else(|| AppError::Internal("mix vanished after update".into()))?;
    Ok(Json(detail))
}
