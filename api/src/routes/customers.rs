use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AppError;
use crate::models::customer::Customer;
use crate::models::mix::{Mix, MixItem};
use crate::models::order::Order;
use crate::routes::mixes::MixDetail;
use crate::AppState;

#[derive(Deserialize)]
pub struct ListCustomersQuery {
    pub email: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    Query(q): Query<ListCustomersQuery>,
) -> Result<Json<Vec<Customer>>, AppError> {
    let customers = match q.email {
        Some(email) => {
            sqlx::query_as::<_, Customer>(
                "select * from customers where email ilike $1 order by created_at desc limit 50",
            )
            .bind(format!("%{email}%"))
            .fetch_all(&state.db)
            .await?
        }
        None => {
            sqlx::query_as::<_, Customer>(
                "select * from customers order by created_at desc limit 50",
            )
            .fetch_all(&state.db)
            .await?
        }
    };
    Ok(Json(customers))
}

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Customer>, AppError> {
    let customer = sqlx::query_as::<_, Customer>("select * from customers where id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("customer not found".into()))?;
    Ok(Json(customer))
}

#[derive(Deserialize)]
pub struct UpdateCustomerRequest {
    pub name: Option<String>,
    pub marketing_consent: Option<bool>,
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateCustomerRequest>,
) -> Result<Json<Customer>, AppError> {
    let customer = sqlx::query_as::<_, Customer>(
        r#"
        update customers
        set
          name = coalesce($2, name),
          marketing_consent = coalesce($3, marketing_consent),
          marketing_consent_at = case
            when $3 = true and marketing_consent_at is null then now()
            else marketing_consent_at
          end
        where id = $1
        returning *
        "#,
    )
    .bind(id)
    .bind(body.name)
    .bind(body.marketing_consent)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("customer not found".into()))?;

    Ok(Json(customer))
}

/// Everything the lookup view needs to show a customer and offer a one-tap
/// reorder, in a single round trip: the customer, their mixes with items
/// already attached, and their orders. Replaces the old
/// customer → list-mixes → get-each-mix fan-out (an N+1 over the stand's
/// connection) with four indexed queries and one response.
#[derive(Serialize)]
pub struct ReorderResponse {
    pub customer: Customer,
    pub mixes: Vec<MixDetail>,
    pub orders: Vec<Order>,
}

pub async fn reorder(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<ReorderResponse>, AppError> {
    let customer = sqlx::query_as::<_, Customer>("select * from customers where id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("customer not found".into()))?;

    let mixes = sqlx::query_as::<_, Mix>(
        "select * from mixes where customer_id = $1 order by created_at desc",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;

    let mix_ids: Vec<Uuid> = mixes.iter().map(|m| m.id).collect();
    let items = sqlx::query_as::<_, MixItem>("select * from mix_items where mix_id = any($1)")
        .bind(&mix_ids)
        .fetch_all(&state.db)
        .await?;

    let orders = sqlx::query_as::<_, Order>(
        "select * from orders where customer_id = $1 order by created_at desc",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;

    // Bucket the items by mix, then reattach in the same order the mixes were
    // fetched so newest-first is preserved.
    let mut by_mix: HashMap<Uuid, Vec<MixItem>> = HashMap::new();
    for item in items {
        by_mix.entry(item.mix_id).or_default().push(item);
    }
    let mixes = mixes
        .into_iter()
        .map(|mix| {
            let items = by_mix.remove(&mix.id).unwrap_or_default();
            MixDetail { mix, items }
        })
        .collect();

    Ok(Json(ReorderResponse {
        customer,
        mixes,
        orders,
    }))
}
