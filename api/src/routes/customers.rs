use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use crate::error::AppError;
use crate::models::customer::Customer;
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
