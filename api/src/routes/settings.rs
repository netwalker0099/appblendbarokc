//! Global settings (admin-only). Currently the per-size price for custom blends.

use axum::extract::State;
use axum::Json;
use rust_decimal::Decimal;
use serde::Deserialize;

use crate::employee_auth::AdminEmployee;
use crate::error::AppError;
use crate::models::settings::Settings;
use crate::AppState;

const COLS: &str =
    "custom_price_oz3_4, custom_price_oz1_7, custom_price_roller, custom_price_spray";

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

    let s = sqlx::query_as::<_, Settings>(&format!(
        "update settings set custom_price_oz3_4 = $1, custom_price_oz1_7 = $2, \
         custom_price_roller = $3, custom_price_spray = $4 \
         where id = true returning {COLS}"
    ))
    .bind(body.custom_price_oz3_4)
    .bind(body.custom_price_oz1_7)
    .bind(body.custom_price_roller)
    .bind(body.custom_price_spray)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(s))
}
