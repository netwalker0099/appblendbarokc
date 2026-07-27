//! Stand intake: record a customer and everything they are taking away.
//!
//! One submission, several items. A customer buying a 3.4oz *and* a roller is one
//! intake with two lines, not two visits to the form; quantity lives on the order
//! because two bottles of the same blend were mixed once.
//!
//! **Intake records intent, never money.** Every order is created as `lead`.
//! Nothing is owed until the order goes into a cart and that cart is checked out,
//! which is a separate, deliberate action — so there is no status to choose here.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Extension;
use axum::Json;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::employee_auth::AuthedEmployee;
use crate::error::AppError;
use crate::models::customer::Customer;
use crate::models::mix::{Mix, MixItem};
use crate::models::order::{BottleSize, Order, OrderStatus, OrderType};
use crate::models::sync::SyncEntity;
use crate::routes::ingredients::assert_active_ingredients;
use crate::routes::mixes::{fetch_mix_detail, MixDetail, MixItemInput};
use crate::routes::scents::assert_active_scent;
use crate::AppState;

/// Guard against a runaway client; a stand order is a handful of bottles.
const MAX_ITEMS: usize = 20;
const MAX_QUANTITY: i32 = 50;

#[derive(Deserialize)]
pub struct IntakeMixRequest {
    /// Required. A blend nobody named is one nobody can find again — every mix
    /// ends up in a customer's history and in the reorder list.
    pub name: String,
    pub items: Vec<MixItemInput>,
}

#[derive(Deserialize)]
pub struct IntakeItemRequest {
    #[serde(rename = "type")]
    pub order_type: OrderType,
    pub size: BottleSize,
    #[serde(default = "one")]
    pub quantity: i32,
    pub scent_id: Option<Uuid>,
    pub mix: Option<IntakeMixRequest>,
    /// Overrides the catalogue price for this line. Omitted is the normal case.
    pub amount: Option<Decimal>,
}

fn one() -> i32 {
    1
}

#[derive(Deserialize)]
pub struct IntakeRequest {
    pub email: String,
    pub name: Option<String>,
    pub marketing_consent: bool,
    pub scent_preference_ids: Option<Vec<Uuid>>,
    /// What the customer is taking away. At least one.
    pub items: Vec<IntakeItemRequest>,
    /// Optional package deal applied to this intake; its components are added on
    /// top of `items` and priced to sum to the bundle price.
    pub bundle_id: Option<Uuid>,
}

#[derive(Serialize)]
pub struct IntakeOrderResult {
    #[serde(flatten)]
    pub order: Order,
    pub mix: Option<MixDetail>,
}

#[derive(Serialize)]
pub struct IntakeResponse {
    pub intake_id: Uuid,
    pub customer: Customer,
    pub orders: Vec<IntakeOrderResult>,
}

/// A line resolved and validated, ready to write.
struct ResolvedLine {
    order_type: OrderType,
    size: BottleSize,
    quantity: i32,
    scent_id: Option<Uuid>,
    mix: Option<IntakeMixRequest>,
    amount: Option<Decimal>,
    bundle_id: Option<Uuid>,
}

pub async fn intake(
    Extension(employee): Extension<AuthedEmployee>,
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

    // Already submitted: replay the same result rather than taking it twice.
    if let Some(existing) = load_existing(&state, &idempotency_key).await? {
        return Ok((StatusCode::OK, Json(existing)));
    }

    if !body.email.contains('@') || body.email.trim().is_empty() {
        return Err(AppError::BadRequest("a valid email is required".into()));
    }
    if body.items.is_empty() && body.bundle_id.is_none() {
        return Err(AppError::BadRequest(
            "an intake needs at least one item".into(),
        ));
    }
    if body.items.len() > MAX_ITEMS {
        return Err(AppError::BadRequest(format!(
            "an intake is limited to {MAX_ITEMS} lines"
        )));
    }

    // Resolve every line before writing anything, so a bad request fails cleanly
    // rather than half-recording an intake.
    let mut lines: Vec<ResolvedLine> = Vec::new();
    for item in body.items {
        lines.push(resolve_line(&state, item, None).await?);
    }
    if let Some(bundle_id) = body.bundle_id {
        lines.extend(resolve_bundle(&state, bundle_id).await?);
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

    // The unique key here is what actually stops a double-tap creating a second
    // set of orders — the check above alone would still race.
    let intake_id: Uuid = sqlx::query_scalar(
        "insert into intakes (customer_id, idempotency_key, created_by) values ($1, $2, $3) returning id",
    )
    .bind(customer.id)
    .bind(&idempotency_key)
    .bind(employee.id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            AppError::Conflict("this intake was already submitted".into())
        }
        _ => AppError::from(e),
    })?;

    let mut results = Vec::with_capacity(lines.len());
    for line in lines {
        let mix_detail = match &line.mix {
            Some(req) => {
                let mix = sqlx::query_as::<_, Mix>(
                    "insert into mixes (customer_id, name) values ($1, $2) returning *",
                )
                .bind(customer.id)
                .bind(req.name.trim())
                .fetch_one(&mut *tx)
                .await?;

                let mut items = Vec::with_capacity(req.items.len());
                for item in &req.items {
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
            }
            None => None,
        };

        // Price from the catalogue unless this line carries an explicit override.
        let amount = match line.amount {
            Some(a) => Some(a),
            None => {
                crate::pricing::catalog_price(&mut *tx, line.order_type, line.size, line.scent_id)
                    .await?
            }
        };

        let order = sqlx::query_as::<_, Order>(
            r#"
            insert into orders
                (customer_id, type, size, quantity, mix_id, scent_id, status, amount,
                 intake_id, bundle_id)
            values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            returning *
            "#,
        )
        .bind(customer.id)
        .bind(line.order_type)
        .bind(line.size)
        .bind(line.quantity)
        .bind(mix_detail.as_ref().map(|m| m.mix.id))
        .bind(line.scent_id)
        // Always a lead. Intake records intent; checkout moves money.
        .bind(OrderStatus::Lead)
        .bind(amount)
        .bind(intake_id)
        .bind(line.bundle_id)
        .fetch_one(&mut *tx)
        .await?;

        results.push(IntakeOrderResult {
            order,
            mix: mix_detail,
        });
    }

    crate::sync::enqueue(&mut *tx, SyncEntity::Contact, customer.id).await?;
    tx.commit().await?;

    Ok((
        StatusCode::CREATED,
        Json(IntakeResponse {
            intake_id,
            customer,
            orders: results,
        }),
    ))
}

/// Validate one requested line and resolve it into something writable.
async fn resolve_line(
    state: &AppState,
    item: IntakeItemRequest,
    bundle_id: Option<Uuid>,
) -> Result<ResolvedLine, AppError> {
    if item.quantity < 1 || item.quantity > MAX_QUANTITY {
        return Err(AppError::BadRequest(format!(
            "quantity must be between 1 and {MAX_QUANTITY}"
        )));
    }

    match item.order_type {
        OrderType::CustomMix => {
            if item.scent_id.is_some() {
                return Err(AppError::BadRequest(
                    "scent_id must not be set for a custom_mix line".into(),
                ));
            }
            let mix = item.mix.as_ref().ok_or_else(|| {
                AppError::BadRequest("a custom_mix line needs its blend".into())
            })?;
            if mix.name.trim().is_empty() {
                return Err(AppError::BadRequest(
                    "every custom blend needs a name".into(),
                ));
            }
            if mix.items.is_empty() {
                return Err(AppError::BadRequest(
                    "a custom blend needs at least one ingredient".into(),
                ));
            }
            let ids: Vec<Uuid> = mix.items.iter().map(|i| i.ingredient_id).collect();
            assert_active_ingredients(&state.db, &ids).await?;
            for i in &mix.items {
                if i.amount_ml <= Decimal::ZERO {
                    return Err(AppError::BadRequest("amount_ml must be positive".into()));
                }
            }
        }
        OrderType::SetPerfume => {
            if item.mix.is_some() {
                return Err(AppError::BadRequest(
                    "mix must not be set for a set_perfume line".into(),
                ));
            }
            let scent_id = item.scent_id.ok_or_else(|| {
                AppError::BadRequest("a set_perfume line needs a scent".into())
            })?;
            assert_active_scent(&state.db, scent_id).await?;
        }
    }

    Ok(ResolvedLine {
        order_type: item.order_type,
        size: item.size,
        quantity: item.quantity,
        scent_id: item.scent_id,
        mix: item.mix,
        amount: item.amount,
        bundle_id,
    })
}

/// Expand a bundle into priced lines.
///
/// The bundle's price is split across its components, weighted by what each
/// would cost on its own, so the parts sum to exactly the advertised price.
async fn resolve_bundle(
    state: &AppState,
    bundle_id: Uuid,
) -> Result<Vec<ResolvedLine>, AppError> {
    let bundle: Option<(String, Decimal, bool)> =
        sqlx::query_as("select name, price, active from bundles where id = $1")
            .bind(bundle_id)
            .fetch_optional(&state.db)
            .await?;
    let (name, price, active) =
        bundle.ok_or_else(|| AppError::NotFound("package not found".into()))?;
    if !active {
        return Err(AppError::BadRequest(format!(
            "the \"{name}\" package is no longer offered"
        )));
    }

    let components: Vec<(OrderType, BottleSize, Option<Uuid>, i32)> = sqlx::query_as(
        "select type, size, scent_id, quantity from bundle_items where bundle_id = $1 order by position",
    )
    .bind(bundle_id)
    .fetch_all(&state.db)
    .await?;

    if components.is_empty() {
        return Err(AppError::BadRequest(format!(
            "the \"{name}\" package has no items in it"
        )));
    }

    // Weight by catalogue price so a 3.4oz carries more of the package than a
    // roller does. Unpriced components weigh nothing and fall back to an even
    // split (handled inside split_bundle_price).
    let mut weights = Vec::with_capacity(components.len());
    for (order_type, size, scent_id, quantity) in &components {
        let unit = crate::pricing::catalog_price(&state.db, *order_type, *size, *scent_id)
            .await?
            .and_then(crate::square::money::to_cents)
            .unwrap_or(0);
        weights.push(unit * *quantity as i64);
    }

    let total_cents = crate::square::money::to_cents(price).ok_or_else(|| {
        AppError::BadRequest(format!("the \"{name}\" package has an invalid price"))
    })?;
    let split = crate::pricing::split_bundle_price(total_cents, &weights);

    let mut lines = Vec::with_capacity(components.len());
    for (i, (order_type, size, scent_id, quantity)) in components.into_iter().enumerate() {
        // A custom blend inside a package still has to be built at the bar; the
        // operator adds it as its own line. Only set perfumes can be expanded
        // automatically, because they need no formula.
        if order_type == OrderType::CustomMix {
            return Err(AppError::BadRequest(format!(
                "the \"{name}\" package contains a custom blend — add that line \
                 yourself so the formula can be recorded"
            )));
        }
        let scent_id = scent_id.ok_or_else(|| {
            AppError::BadRequest(format!(
                "the \"{name}\" package has a set-perfume slot with no scent chosen"
            ))
        })?;
        assert_active_scent(&state.db, scent_id).await?;

        // Per bottle, not per line — orders store a unit amount.
        let per_unit = crate::square::money::from_cents(split[i] / quantity.max(1) as i64);

        lines.push(ResolvedLine {
            order_type,
            size,
            quantity,
            scent_id: Some(scent_id),
            mix: None,
            amount: Some(per_unit),
            bundle_id: Some(bundle_id),
        });
    }
    Ok(lines)
}

/// Replay a previously-submitted intake.
async fn load_existing(
    state: &AppState,
    key: &str,
) -> Result<Option<IntakeResponse>, AppError> {
    let row: Option<(Uuid, Uuid)> =
        sqlx::query_as("select id, customer_id from intakes where idempotency_key = $1")
            .bind(key)
            .fetch_optional(&state.db)
            .await?;
    let Some((intake_id, customer_id)) = row else {
        return Ok(None);
    };

    let customer = sqlx::query_as::<_, Customer>("select * from customers where id = $1")
        .bind(customer_id)
        .fetch_one(&state.db)
        .await?;

    let orders = sqlx::query_as::<_, Order>(
        "select * from orders where intake_id = $1 order by created_at",
    )
    .bind(intake_id)
    .fetch_all(&state.db)
    .await?;

    let mut results = Vec::with_capacity(orders.len());
    for order in orders {
        let mix = match order.mix_id {
            Some(mix_id) => fetch_mix_detail(&state.db, mix_id).await?,
            None => None,
        };
        results.push(IntakeOrderResult { order, mix });
    }

    Ok(Some(IntakeResponse {
        intake_id,
        customer,
        orders: results,
    }))
}
