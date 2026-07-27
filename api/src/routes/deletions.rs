//! Admin deletion of customers, mixes and orders.
//!
//! One rule runs through all of it: **nothing money has touched can be deleted.**
//! A paid cart is a financial record that has to reconcile against Square, and
//! Square keeps its side forever. Deleting our half would turn a matched sale into
//! a permanent "only in Square" discrepancy that nobody can ever explain.
//!
//! So these endpoints refuse rather than cascade, and say what to do instead.
//! Everything they *do* remove is genuinely disposable: an unsold order, a blend
//! nobody bought, a customer who never got as far as paying.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Serialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::employee_auth::AdminEmployee;
use crate::error::AppError;
use crate::AppState;

/// What deleting a customer would take with it — shown before the fact so an
/// admin is never guessing at the size of what they are about to do.
#[derive(Debug, Serialize)]
pub struct DeletionImpact {
    pub customer_id: Uuid,
    pub email: String,
    pub orders: i64,
    pub mixes: i64,
    pub carts: i64,
    /// Carts that were actually paid. Non-zero means deletion is refused.
    pub paid_carts: i64,
    pub can_delete: bool,
    pub reason: Option<String>,
}

async fn impact(state: &AppState, id: Uuid) -> Result<DeletionImpact, AppError> {
    let email: String = sqlx::query_scalar("select email from customers where id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("customer not found".into()))?;

    let counts: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        select
          (select count(*) from orders where customer_id = $1),
          (select count(*) from mixes  where customer_id = $1),
          (select count(*) from carts  where customer_id = $1),
          (select count(*) from carts  where customer_id = $1 and status in ('paid', 'refunded'))
        "#,
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;

    let (orders, mixes, carts, paid_carts) = counts;
    let reason = if paid_carts > 0 {
        Some(format!(
            "this customer has {paid_carts} paid or refunded cart(s). Deleting them \
             would break reconciliation against Square, which keeps its own record. \
             Their history has to stay."
        ))
    } else {
        None
    };

    Ok(DeletionImpact {
        customer_id: id,
        email,
        orders,
        mixes,
        carts,
        paid_carts,
        can_delete: reason.is_none(),
        reason,
    })
}

/// Dry run: what would be removed, and whether it is allowed.
pub async fn customer_impact(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<DeletionImpact>, AppError> {
    Ok(Json(impact(&state, id).await?))
}

pub async fn delete_customer(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let assessment = impact(&state, id).await?;
    if !assessment.can_delete {
        return Err(AppError::Conflict(
            assessment.reason.unwrap_or_else(|| "cannot delete".into()),
        ));
    }

    let mut tx = state.db.begin().await?;

    // Order matters: children before parents, or the foreign keys refuse.
    sqlx::query(
        "delete from cart_items where cart_id in (select id from carts where customer_id = $1)",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("delete from notification_deliveries where cart_id in (select id from carts where customer_id = $1)")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("delete from carts where customer_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("delete from orders where customer_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("delete from mix_items where mix_id in (select id from mixes where customer_id = $1)")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("delete from mixes where customer_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("delete from customer_scent_preferences where customer_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("delete from customer_login_tokens where customer_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("delete from customer_sessions where customer_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("delete from intakes where customer_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("delete from sync_outbox where entity_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("delete from customers where id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    tracing::warn!(customer_id = %id, email = %assessment.email, "customer deleted by admin");
    Ok(Json(json!({
        "deleted": true,
        "email": assessment.email,
        "orders_removed": assessment.orders,
        "mixes_removed": assessment.mixes,
    })))
}

/// Delete a saved blend. Refused while an order still points at it.
pub async fn delete_mix(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let used: i64 = sqlx::query_scalar("select count(*) from orders where mix_id = $1")
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    if used > 0 {
        return Err(AppError::Conflict(format!(
            "this blend is on {used} order(s) — delete those first, or keep it for \
             the customer's history"
        )));
    }

    let mut tx = state.db.begin().await?;
    sqlx::query("delete from mix_items where mix_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let n = sqlx::query("delete from mixes where id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    tx.commit().await?;

    if n == 0 {
        return Err(AppError::NotFound("blend not found".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Delete an order. Refused once it has been sold.
pub async fn delete_order(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let row: Option<(String, i64)> = sqlx::query_as(
        r#"
        select o.status::text,
               (select count(*) from cart_items ci
                  join carts c on c.id = ci.cart_id
                 where ci.order_id = o.id and c.status in ('paid', 'refunded'))
        from orders o where o.id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;

    let (status, paid_carts) = row.ok_or_else(|| AppError::NotFound("order not found".into()))?;
    if paid_carts > 0 || status != "lead" {
        return Err(AppError::Conflict(
            "this order has been paid for — it stays, so the books still reconcile".into(),
        ));
    }

    let mut tx = state.db.begin().await?;
    // Release it from any open cart first; the unique index would otherwise
    // leave a dangling claim.
    sqlx::query("delete from cart_items where order_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let n = sqlx::query("delete from orders where id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    tx.commit().await?;

    if n == 0 {
        return Err(AppError::NotFound("order not found".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Delete a catalogue entry (ingredient or scent) that nothing has ever used.
///
/// Deactivating is the normal move — it keeps history readable while removing the
/// thing from pickers. Deletion is only for genuine mistakes, e.g. a typo added
/// and never used.
pub async fn delete_ingredient(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let used: (i64, i64) = sqlx::query_as(
        "select (select count(*) from mix_items where ingredient_id = $1),
                (select count(*) from scent_items where ingredient_id = $1)",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    if used.0 > 0 || used.1 > 0 {
        return Err(AppError::Conflict(format!(
            "this ingredient is used in {} blend(s) and {} house formula(s) — \
             deactivate it instead",
            used.0, used.1
        )));
    }

    let n = sqlx::query("delete from ingredients where id = $1")
        .bind(id)
        .execute(&state.db)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(AppError::NotFound("ingredient not found".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_scent(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let used: (i64, i64, i64) = sqlx::query_as(
        "select (select count(*) from orders where scent_id = $1),
                (select count(*) from customer_scent_preferences where scent_id = $1),
                (select count(*) from bundle_items where scent_id = $1)",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    if used.0 > 0 || used.2 > 0 {
        return Err(AppError::Conflict(format!(
            "this scent is on {} order(s) and {} package(s) — deactivate it instead",
            used.0, used.2
        )));
    }

    let mut tx = state.db.begin().await?;
    sqlx::query("delete from customer_scent_preferences where scent_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("delete from scent_items where scent_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let n = sqlx::query("delete from scents where id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    tx.commit().await?;

    if n == 0 {
        return Err(AppError::NotFound("scent not found".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}
