use rust_decimal::Decimal;
use serde::Serialize;

/// Global settings (single row). Per-size price for custom bespoke blends.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Settings {
    pub custom_price_oz3_4: Option<Decimal>,
    pub custom_price_oz1_7: Option<Decimal>,
    pub custom_price_roller: Option<Decimal>,
    pub custom_price_spray: Option<Decimal>,
    /// Referral programme. Amounts in cents, matching how carts store money.
    pub referral_enabled: bool,
    pub referral_discount_cents: i64,
    pub referral_reward_cents: i64,
    /// 0 means coupons never expire.
    pub coupon_expiry_days: i32,
}
