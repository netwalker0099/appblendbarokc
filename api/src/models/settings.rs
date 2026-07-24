use rust_decimal::Decimal;
use serde::Serialize;

/// Global settings (single row). Per-size price for custom bespoke blends.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Settings {
    pub custom_price_oz3_4: Option<Decimal>,
    pub custom_price_oz1_7: Option<Decimal>,
    pub custom_price_roller: Option<Decimal>,
}
