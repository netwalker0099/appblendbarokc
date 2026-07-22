use axum::extract::{Path, Query, State};
use axum::Json;
use rust_decimal::Decimal;
use serde::Deserialize;
use uuid::Uuid;

use crate::error::AppError;
use crate::models::order::Order;
use crate::AppState;

#[derive(Deserialize)]
pub struct ListOrdersQuery {
    pub status: Option<String>,
    pub customer_id: Option<Uuid>,
}

pub async fn list(
    State(state): State<AppState>,
    Query(q): Query<ListOrdersQuery>,
) -> Result<Json<Vec<Order>>, AppError> {
    let orders = sqlx::query_as::<_, Order>(
        r#"
        select * from orders
        where ($1::text is null or status = $1)
          and ($2::uuid is null or customer_id = $2)
        order by created_at desc
        limit 100
        "#,
    )
    .bind(q.status)
    .bind(q.customer_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(orders))
}

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Order>, AppError> {
    let order = sqlx::query_as::<_, Order>("select * from orders where id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("order not found".into()))?;
    Ok(Json(order))
}

#[derive(Deserialize)]
pub struct UpdateOrderRequest {
    pub status: Option<String>,
    pub squarespace_order_id: Option<String>,
    pub external_ref: Option<String>,
    pub amount: Option<Decimal>,
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateOrderRequest>,
) -> Result<Json<Order>, AppError> {
    let order = sqlx::query_as::<_, Order>(
        r#"
        update orders set
          status = coalesce($2, status),
          squarespace_order_id = coalesce($3, squarespace_order_id),
          external_ref = coalesce($4, external_ref),
          amount = coalesce($5, amount)
        where id = $1
        returning *
        "#,
    )
    .bind(id)
    .bind(body.status)
    .bind(body.squarespace_order_id)
    .bind(body.external_ref)
    .bind(body.amount)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db_err) if db_err.is_check_violation() => {
            AppError::BadRequest("invalid status value".into())
        }
        _ => AppError::from(e),
    })?
    .ok_or_else(|| AppError::NotFound("order not found".into()))?;

    Ok(Json(order))
}
