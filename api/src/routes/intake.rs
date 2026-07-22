use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AppError;
use crate::models::customer::Customer;
use crate::models::mix::{Mix, MixItem};
use crate::models::order::{BottleSize, Order, OrderStatus, OrderType};
use crate::routes::ingredients::assert_active_ingredients;
use crate::routes::mixes::{fetch_mix_detail, MixDetail, MixItemInput};
use crate::routes::scents::assert_active_scent;
use crate::AppState;

#[derive(Deserialize)]
pub struct IntakeMixRequest {
    pub name: Option<String>,
    pub items: Vec<MixItemInput>,
}

#[derive(Deserialize)]
pub struct IntakeOrderRequest {
    #[serde(rename = "type")]
    pub order_type: OrderType,
    pub size: BottleSize,
    pub status: OrderStatus,
    pub scent_id: Option<Uuid>,
    pub mix: Option<IntakeMixRequest>,
    pub amount: Option<Decimal>,
}

#[derive(Deserialize)]
pub struct IntakeRequest {
    pub email: String,
    pub name: Option<String>,
    pub marketing_consent: bool,
    pub scent_preference_ids: Option<Vec<Uuid>>,
    pub order: IntakeOrderRequest,
}

#[derive(Serialize)]
pub struct IntakeResponse {
    pub customer: Customer,
    pub mix: Option<MixDetail>,
    pub order: Order,
}

pub async fn intake(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<IntakeRequest>,
) -> Result<(StatusCode, Json<IntakeResponse>), AppError> {
    let idempotency_key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| AppError::BadRequest("Idempotency-Key header is required".into()))?
        .to_string();

    if let Some(existing_order) =
        sqlx::query_as::<_, Order>("select * from orders where idempotency_key = $1")
            .bind(&idempotency_key)
            .fetch_optional(&state.db)
            .await?
    {
        let customer = sqlx::query_as::<_, Customer>("select * from customers where id = $1")
            .bind(existing_order.customer_id)
            .fetch_one(&state.db)
            .await?;
        let mix = match existing_order.mix_id {
            Some(mix_id) => fetch_mix_detail(&state.db, mix_id).await?,
            None => None,
        };
        return Ok((
            StatusCode::OK,
            Json(IntakeResponse {
                customer,
                mix,
                order: existing_order,
            }),
        ));
    }

    if !body.email.contains('@') || body.email.trim().is_empty() {
        return Err(AppError::BadRequest("a valid email is required".into()));
    }

    if body.order.status == OrderStatus::Fulfilled {
        return Err(AppError::BadRequest(
            "orders can only be created as lead or paid at intake".into(),
        ));
    }

    match body.order.order_type {
        OrderType::CustomMix => {
            if body.order.scent_id.is_some() {
                return Err(AppError::BadRequest(
                    "scent_id must not be set for a custom_mix order".into(),
                ));
            }
            let mix = body.order.mix.as_ref().ok_or_else(|| {
                AppError::BadRequest("mix is required for a custom_mix order".into())
            })?;
            let ids: Vec<Uuid> = mix.items.iter().map(|i| i.ingredient_id).collect();
            assert_active_ingredients(&state.db, &ids).await?;
            for item in &mix.items {
                if item.amount_ml <= Decimal::ZERO {
                    return Err(AppError::BadRequest("amount_ml must be positive".into()));
                }
            }
        }
        OrderType::SetPerfume => {
            if body.order.mix.is_some() {
                return Err(AppError::BadRequest(
                    "mix must not be set for a set_perfume order".into(),
                ));
            }
            let scent_id = body.order.scent_id.ok_or_else(|| {
                AppError::BadRequest("scent_id is required for a set_perfume order".into())
            })?;
            assert_active_scent(&state.db, scent_id).await?;
        }
    }

    let mut tx = state.db.begin().await?;

    let customer = sqlx::query_as::<_, Customer>(
        r#"
        insert into customers (email, name, marketing_consent, marketing_consent_at)
        values ($1, $2, $3, case when $3 then now() else null end)
        on conflict (email) do update set
          name = coalesce(excluded.name, customers.name),
          marketing_consent = excluded.marketing_consent,
          marketing_consent_at = case
            when excluded.marketing_consent and customers.marketing_consent_at is null then now()
            else customers.marketing_consent_at
          end
        returning *
        "#,
    )
    .bind(&body.email)
    .bind(&body.name)
    .bind(body.marketing_consent)
    .fetch_one(&mut *tx)
    .await?;

    if let Some(scent_ids) = &body.scent_preference_ids {
        sqlx::query("delete from customer_scent_preferences where customer_id = $1")
            .bind(customer.id)
            .execute(&mut *tx)
            .await?;

        for scent_id in scent_ids {
            sqlx::query(
                "insert into customer_scent_preferences (customer_id, scent_id) values ($1, $2)",
            )
            .bind(customer.id)
            .bind(scent_id)
            .execute(&mut *tx)
            .await?;
        }
    }

    let mix_detail = if let Some(mix_req) = &body.order.mix {
        let mix = sqlx::query_as::<_, Mix>(
            "insert into mixes (customer_id, name) values ($1, $2) returning *",
        )
        .bind(customer.id)
        .bind(&mix_req.name)
        .fetch_one(&mut *tx)
        .await?;

        let mut items = Vec::with_capacity(mix_req.items.len());
        for item in &mix_req.items {
            let row = sqlx::query_as::<_, MixItem>(
                "insert into mix_items (mix_id, ingredient_id, amount_ml) values ($1, $2, $3) returning *",
            )
            .bind(mix.id)
            .bind(item.ingredient_id)
            .bind(item.amount_ml)
            .fetch_one(&mut *tx)
            .await?;
            items.push(row);
        }

        Some(MixDetail { mix, items })
    } else {
        None
    };

    let order = sqlx::query_as::<_, Order>(
        r#"
        insert into orders (customer_id, type, size, mix_id, scent_id, status, amount, idempotency_key)
        values ($1, $2, $3, $4, $5, $6, $7, $8)
        returning *
        "#,
    )
    .bind(customer.id)
    .bind(body.order.order_type)
    .bind(body.order.size)
    .bind(mix_detail.as_ref().map(|m| m.mix.id))
    .bind(body.order.scent_id)
    .bind(body.order.status)
    .bind(body.order.amount)
    .bind(&idempotency_key)
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok((
        StatusCode::CREATED,
        Json(IntakeResponse {
            customer,
            mix: mix_detail,
            order,
        }),
    ))
}
