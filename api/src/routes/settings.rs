//! Global settings (admin-only). Currently the per-size price for custom blends.

use axum::extract::State;
use axum::Json;
use rust_decimal::Decimal;
use serde::Deserialize;

use crate::employee_auth::AdminEmployee;
use crate::error::AppError;
use crate::models::settings::Settings;
use crate::AppState;

const COLS: &str = "custom_price_oz3_4, custom_price_oz1_7, custom_price_roller, \
     custom_price_spray, referral_enabled, referral_discount_cents, \
     referral_reward_cents, coupon_expiry_days, audit_retention_days";

pub async fn get(
    _admin: AdminEmployee,
    State(state): State<AppState>,
) -> Result<Json<Settings>, AppError> {
    let s = sqlx::query_as::<_, Settings>(&format!("select {COLS} from settings where id = true"))
        .fetch_one(&state.db)
        .await?;
    Ok(Json(s))
}

#[derive(Deserialize)]
pub struct UpdateSettings {
    pub custom_price_oz3_4: Option<Decimal>,
    pub custom_price_oz1_7: Option<Decimal>,
    pub custom_price_roller: Option<Decimal>,
    pub custom_price_spray: Option<Decimal>,
    pub referral_enabled: Option<bool>,
    /// Cents, so the wire format matches how money is stored on carts.
    pub referral_discount_cents: Option<i64>,
    pub referral_reward_cents: Option<i64>,
    pub coupon_expiry_days: Option<i32>,
    /// Days of audit history to keep in the table. 0 keeps everything.
    /// Anything older is archived off-box and only then pruned — see
    /// `audit::archive`.
    pub audit_retention_days: Option<i32>,
}

pub async fn update(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Json(body): Json<UpdateSettings>,
) -> Result<Json<Settings>, AppError> {
    for p in [
        body.custom_price_oz3_4,
        body.custom_price_oz1_7,
        body.custom_price_roller,
        body.custom_price_spray,
    ]
    .into_iter()
    .flatten()
    {
        if p < Decimal::ZERO {
            return Err(AppError::BadRequest("prices can't be negative".into()));
        }
    }

    for cents in [body.referral_discount_cents, body.referral_reward_cents]
        .into_iter()
        .flatten()
    {
        if cents < 0 {
            return Err(AppError::BadRequest(
                "referral amounts can't be negative".into(),
            ));
        }
    }
    if body.coupon_expiry_days.is_some_and(|d| d < 0) {
        return Err(AppError::BadRequest(
            "coupon expiry can't be negative — use 0 for never".into(),
        ));
    }
    // A floor rather than just a non-negative check. A one-day retention would
    // start shipping today's audit entries off-box within hours, which is not a
    // setting anyone wants by accident — and the archive only becomes readable
    // again by decrypting a file.
    if body.audit_retention_days.is_some_and(|d| d != 0 && d < 30) {
        return Err(AppError::BadRequest(
            "audit retention must be 0 (keep everything) or at least 30 days".into(),
        ));
    }

    let s = sqlx::query_as::<_, Settings>(&format!(
        "update settings set custom_price_oz3_4 = $1, custom_price_oz1_7 = $2, \
         custom_price_roller = $3, custom_price_spray = $4, \
         referral_enabled = coalesce($5, referral_enabled), \
         referral_discount_cents = coalesce($6, referral_discount_cents), \
         referral_reward_cents = coalesce($7, referral_reward_cents), \
         coupon_expiry_days = coalesce($8, coupon_expiry_days), \
         audit_retention_days = coalesce($9, audit_retention_days) \
         where id = true returning {COLS}"
    ))
    .bind(body.custom_price_oz3_4)
    .bind(body.custom_price_oz1_7)
    .bind(body.custom_price_roller)
    .bind(body.custom_price_spray)
    .bind(body.referral_enabled)
    .bind(body.referral_discount_cents)
    .bind(body.referral_reward_cents)
    .bind(body.coupon_expiry_days)
    .bind(body.audit_retention_days)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(s))
}
