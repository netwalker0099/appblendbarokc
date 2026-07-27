//! Package deals — a named set of bottles sold for one headline price.
//!
//! The price lives on the bundle rather than being derived from its parts,
//! because the whole point of a package is that it costs less than the sum. What
//! each component is *worth* still matters though: it decides how the package
//! price is split across the orders, so the history and the cart add up.
//!
//! Admins define them; any employee can read them, because intake needs the list.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::employee_auth::AdminEmployee;
use crate::error::AppError;
use crate::models::order::{BottleSize, OrderType};
use crate::AppState;

const MAX_COMPONENTS: usize = 12;

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Bundle {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub price: Decimal,
    pub active: bool,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct BundleItem {
    pub id: Uuid,
    pub bundle_id: Uuid,
    pub position: i32,
    #[sqlx(rename = "type")]
    pub item_type: OrderType,
    pub size: BottleSize,
    pub scent_id: Option<Uuid>,
    pub quantity: i32,
}

#[derive(Debug, Serialize)]
pub struct BundleDetail {
    #[serde(flatten)]
    pub bundle: Bundle,
    pub items: Vec<BundleItem>,
}

pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<BundleDetail>>, AppError> {
    let bundles = sqlx::query_as::<_, Bundle>(
        "select id, name, description, price, active from bundles order by name",
    )
    .fetch_all(&state.db)
    .await?;

    let mut out = Vec::with_capacity(bundles.len());
    for bundle in bundles {
        let items = sqlx::query_as::<_, BundleItem>(
            "select * from bundle_items where bundle_id = $1 order by position",
        )
        .bind(bundle.id)
        .fetch_all(&state.db)
        .await?;
        out.push(BundleDetail { bundle, items });
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct BundleItemInput {
    #[serde(rename = "type")]
    pub item_type: OrderType,
    pub size: BottleSize,
    pub scent_id: Option<Uuid>,
    #[serde(default = "one")]
    pub quantity: i32,
}

fn one() -> i32 {
    1
}

#[derive(Deserialize)]
pub struct UpsertBundle {
    pub name: String,
    pub description: Option<String>,
    pub price: Decimal,
    #[serde(default)]
    pub items: Vec<BundleItemInput>,
    pub active: Option<bool>,
}

async fn validate(body: &UpsertBundle) -> Result<(), AppError> {
    if body.name.trim().is_empty() {
        return Err(AppError::BadRequest("a package needs a name".into()));
    }
    if body.price < Decimal::ZERO {
        return Err(AppError::BadRequest("price can't be negative".into()));
    }
    if body.items.is_empty() {
        return Err(AppError::BadRequest(
            "a package needs at least one item in it".into(),
        ));
    }
    if body.items.len() > MAX_COMPONENTS {
        return Err(AppError::BadRequest(format!(
            "a package is limited to {MAX_COMPONENTS} items"
        )));
    }
    for item in &body.items {
        if item.quantity < 1 {
            return Err(AppError::BadRequest("quantity must be at least 1".into()));
        }
        match item.item_type {
            // A set perfume needs a named scent, or intake has nothing to sell.
            OrderType::SetPerfume if item.scent_id.is_none() => {
                return Err(AppError::BadRequest(
                    "choose a scent for each set-perfume item".into(),
                ))
            }
            OrderType::CustomMix if item.scent_id.is_some() => {
                return Err(AppError::BadRequest(
                    "a custom-blend item can't name a scent".into(),
                ))
            }
            _ => {}
        }
    }
    Ok(())
}

async fn write_items(
    tx: &mut sqlx::PgConnection,
    bundle_id: Uuid,
    items: &[BundleItemInput],
) -> Result<(), AppError> {
    sqlx::query("delete from bundle_items where bundle_id = $1")
        .bind(bundle_id)
        .execute(&mut *tx)
        .await?;
    for (i, item) in items.iter().enumerate() {
        sqlx::query(
            "insert into bundle_items (bundle_id, position, type, size, scent_id, quantity) \
             values ($1, $2, $3, $4, $5, $6)",
        )
        .bind(bundle_id)
        .bind(i as i32)
        .bind(item.item_type)
        .bind(item.size)
        .bind(item.scent_id)
        .bind(item.quantity)
        .execute(&mut *tx)
        .await?;
    }
    Ok(())
}

pub async fn create(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Json(body): Json<UpsertBundle>,
) -> Result<(StatusCode, Json<BundleDetail>), AppError> {
    validate(&body).await?;

    let mut tx = state.db.begin().await?;
    let bundle = sqlx::query_as::<_, Bundle>(
        "insert into bundles (name, description, price) values ($1, $2, $3) \
         returning id, name, description, price, active",
    )
    .bind(body.name.trim())
    .bind(body.description.as_deref().map(str::trim))
    .bind(body.price)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            AppError::Conflict("a package with that name already exists".into())
        }
        _ => AppError::from(e),
    })?;

    write_items(&mut tx, bundle.id, &body.items).await?;
    tx.commit().await?;

    let items = sqlx::query_as::<_, BundleItem>(
        "select * from bundle_items where bundle_id = $1 order by position",
    )
    .bind(bundle.id)
    .fetch_all(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(BundleDetail { bundle, items })))
}

pub async fn update(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpsertBundle>,
) -> Result<Json<BundleDetail>, AppError> {
    validate(&body).await?;

    let mut tx = state.db.begin().await?;
    let bundle = sqlx::query_as::<_, Bundle>(
        "update bundles set name = $2, description = $3, price = $4, \
         active = coalesce($5, active), updated_at = now() \
         where id = $1 returning id, name, description, price, active",
    )
    .bind(id)
    .bind(body.name.trim())
    .bind(body.description.as_deref().map(str::trim))
    .bind(body.price)
    .bind(body.active)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::NotFound("package not found".into()))?;

    write_items(&mut tx, bundle.id, &body.items).await?;
    tx.commit().await?;

    let items = sqlx::query_as::<_, BundleItem>(
        "select * from bundle_items where bundle_id = $1 order by position",
    )
    .bind(bundle.id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(BundleDetail { bundle, items }))
}

/// Remove a package.
///
/// Orders already sold under it keep their `bundle_id`, so deleting would orphan
/// that reference and erase what a customer actually bought. Once a package has
/// been sold it can only be deactivated — which hides it from intake and is what
/// "stop offering this" actually means.
pub async fn delete(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let sold: i64 = sqlx::query_scalar("select count(*) from orders where bundle_id = $1")
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    if sold > 0 {
        return Err(AppError::Conflict(format!(
            "this package has been sold {sold} time(s) — deactivate it instead so \
             those orders keep their history"
        )));
    }

    let n = sqlx::query("delete from bundles where id = $1")
        .bind(id)
        .execute(&state.db)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(AppError::NotFound("package not found".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}
