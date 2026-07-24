//! Public (no-auth) share targets: a customer can share a scent link/QR with a
//! friend. Deliberately exposes ingredient NAMES only (the "notes") and prices —
//! never the ml amounts, which stay employee-only.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header;
use axum::response::Response;
use axum::Json;
use qrcode::render::svg;
use qrcode::QrCode;
use rust_decimal::Decimal;
use serde::Serialize;
use uuid::Uuid;

use crate::error::AppError;
use crate::models::scent::Scent;
use crate::AppState;

fn site_url() -> String {
    std::env::var("CUSTOMER_SITE_URL")
        .unwrap_or_else(|_| "https://sandbox.theblendbarokc.com".to_string())
}

#[derive(Serialize)]
pub struct PublicScent {
    pub id: Uuid,
    pub name: String,
    /// Ingredient names only — no amounts.
    pub notes: Vec<String>,
    pub price_oz3_4: Option<Decimal>,
    pub price_oz1_7: Option<Decimal>,
    pub price_roller: Option<Decimal>,
}

pub async fn scent(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<PublicScent>, AppError> {
    let scent = sqlx::query_as::<_, Scent>("select * from scents where id = $1 and active = true")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("scent not found".into()))?;

    let notes: Vec<String> = sqlx::query_scalar(
        "select i.name from scent_items si join ingredients i on i.id = si.ingredient_id \
         where si.scent_id = $1 order by i.name",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(PublicScent {
        id: scent.id,
        name: scent.name,
        notes,
        price_oz3_4: scent.price_oz3_4,
        price_oz1_7: scent.price_oz1_7,
        price_roller: scent.price_roller,
    }))
}

/// An SVG QR code that points at the scent's public share page.
pub async fn scent_qr(Path(id): Path<Uuid>) -> Result<Response, AppError> {
    let url = format!("{}/s/{}", site_url(), id);
    let code = QrCode::new(url.as_bytes())
        .map_err(|e| AppError::Internal(format!("qr encode failed: {e}")))?;
    let image = code
        .render::<svg::Color>()
        .min_dimensions(220, 220)
        .quiet_zone(true)
        .build();

    Response::builder()
        .header(header::CONTENT_TYPE, "image/svg+xml")
        .header(header::CACHE_CONTROL, "public, max-age=3600")
        .body(Body::from(image))
        .map_err(|e| AppError::Internal(e.to_string()))
}
