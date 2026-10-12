//! Global settings (admin-only): custom-blend prices, the referral programme,
//! audit retention, and the MFA trust window.

use axum::extract::State;
use axum::Json;
use rust_decimal::Decimal;
use serde::Deserialize;

use crate::employee_auth::AdminEmployee;
use crate::error::AppError;
use crate::models::settings::Settings;
use crate::AppState;

const COLS: &str = "custom_price_vintage, custom_price_oz3_4, custom_price_oz1_7, \
     custom_price_oz1_2, custom_price_roller, \
     custom_price_spray, referral_enabled, referral_discount_cents, \
     referral_reward_cents, coupon_expiry_days, audit_retention_days, \
     mfa_trust_hours";

pub async fn get(
    _admin: AdminEmployee,
    State(state): State<AppState>,
) -> Result<Json<Settings>, AppError> {
    let s = sqlx::query_as::<_, Settings>(&format!("select {COLS} from settings where id = true"))
        .fetch_one(&state.db)
        .await?;
    Ok(Json(s))
}

/// Tells "field absent" apart from "field explicitly null" in a PATCH body.
///
/// The custom-blend prices are the only nullable settings — a null clears a
/// price, which takes that size off sale. With a plain `Option` both cases
/// deserialize to `None`, so *any* partial save of this endpoint (referrals,
/// retention, the MFA window) silently wiped all four prices. `Option<Option<T>>`
/// keeps them apart: the outer layer is "was it sent", the inner is "what to".
fn present<'de, D, T>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    Option::<T>::deserialize(de).map(Some)
}

#[derive(Deserialize)]
pub struct UpdateSettings {
    #[serde(default, deserialize_with = "present")]
    pub custom_price_vintage: Option<Option<Decimal>>,
    #[serde(default, deserialize_with = "present")]
    pub custom_price_oz3_4: Option<Option<Decimal>>,
    #[serde(default, deserialize_with = "present")]
    pub custom_price_oz1_7: Option<Option<Decimal>>,
    #[serde(default, deserialize_with = "present")]
    pub custom_price_oz1_2: Option<Option<Decimal>>,
    #[serde(default, deserialize_with = "present")]
    pub custom_price_roller: Option<Option<Decimal>>,
    #[serde(default, deserialize_with = "present")]
    pub custom_price_spray: Option<Option<Decimal>>,
    pub referral_enabled: Option<bool>,
    /// Cents, so the wire format matches how money is stored on carts.
    pub referral_discount_cents: Option<i64>,
    pub referral_reward_cents: Option<i64>,
    pub coupon_expiry_days: Option<i32>,
    /// Days of audit history to keep in the table. 0 keeps everything.
    /// Anything older is archived off-box and only then pruned — see
    /// `audit::archive`.
    pub audit_retention_days: Option<i32>,
    /// Hours a device stays trusted after a successful MFA check, during which
    /// sign-in needs only the password. 0 asks for MFA every time.
    pub mfa_trust_hours: Option<i32>,
}

pub async fn update(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Json(body): Json<UpdateSettings>,
) -> Result<Json<Settings>, AppError> {
    for p in [
        body.custom_price_vintage,
        body.custom_price_oz3_4,
        body.custom_price_oz1_7,
        body.custom_price_oz1_2,
        body.custom_price_roller,
        body.custom_price_spray,
    ]
    .into_iter()
    .flatten()
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

    // Mirrors the column's own check constraint so the UI gets a sentence back
    // instead of a raw 23514. The ceiling is a week: beyond that a second factor
    // has stopped being one, and "trusted indefinitely" should not be reachable
    // by typing a large number into the box.
    if body.mfa_trust_hours.is_some_and(|h| !(0..=168).contains(&h)) {
        return Err(AppError::BadRequest(
            "MFA trust window must be between 0 (always ask) and 168 hours".into(),
        ));
    }

    // The prices need `case when <sent> then <value> ...` rather than coalesce,
    // because for them null is a meaningful value (clear the price) and coalesce
    // cannot tell that from "not sent". Everything else is non-nullable, so
    // coalesce says exactly the right thing.
    let s = sqlx::query_as::<_, Settings>(&format!(
        "update settings set \
         custom_price_vintage = case when $1 then $2 else custom_price_vintage end, \
         custom_price_oz3_4 = case when $3 then $4 else custom_price_oz3_4 end, \
         custom_price_oz1_7 = case when $5 then $6 else custom_price_oz1_7 end, \
         custom_price_oz1_2 = case when $7 then $8 else custom_price_oz1_2 end, \
         custom_price_roller = case when $9 then $10 else custom_price_roller end, \
         custom_price_spray = case when $11 then $12 else custom_price_spray end, \
         referral_enabled = coalesce($13, referral_enabled), \
         referral_discount_cents = coalesce($14, referral_discount_cents), \
         referral_reward_cents = coalesce($15, referral_reward_cents), \
         coupon_expiry_days = coalesce($16, coupon_expiry_days), \
         audit_retention_days = coalesce($17, audit_retention_days), \
         mfa_trust_hours = coalesce($18, mfa_trust_hours) \
         where id = true returning {COLS}"
    ))
    .bind(body.custom_price_vintage.is_some())
    .bind(body.custom_price_vintage.flatten())
    .bind(body.custom_price_oz3_4.is_some())
    .bind(body.custom_price_oz3_4.flatten())
    .bind(body.custom_price_oz1_7.is_some())
    .bind(body.custom_price_oz1_7.flatten())
    .bind(body.custom_price_oz1_2.is_some())
    .bind(body.custom_price_oz1_2.flatten())
    .bind(body.custom_price_roller.is_some())
    .bind(body.custom_price_roller.flatten())
    .bind(body.custom_price_spray.is_some())
    .bind(body.custom_price_spray.flatten())
    .bind(body.referral_enabled)
    .bind(body.referral_discount_cents)
    .bind(body.referral_reward_cents)
    .bind(body.coupon_expiry_days)
    .bind(body.audit_retention_days)
    .bind(body.mfa_trust_hours)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(s))
}
