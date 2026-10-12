use rust_decimal::Decimal;
use serde::Serialize;

/// Global settings (single row). Per-size price for custom bespoke blends.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Settings {
    pub custom_price_vintage: Option<Decimal>,
    pub custom_price_oz3_4: Option<Decimal>,
    pub custom_price_oz1_7: Option<Decimal>,
    pub custom_price_oz1_2: Option<Decimal>,
    pub custom_price_roller: Option<Decimal>,
    pub custom_price_spray: Option<Decimal>,
    /// Referral programme. Amounts in cents, matching how carts store money.
    pub referral_enabled: bool,
    pub referral_discount_cents: i64,
    pub referral_reward_cents: i64,
    /// 0 means coupons never expire.
    pub coupon_expiry_days: i32,
    /// Days of audit history kept in the table; 0 keeps everything. Anything
    /// older is archived off-box and pruned only once delivered.
    pub audit_retention_days: i32,
    /// How long a device stays trusted after MFA before the code is asked for
    /// again. 0 means every sign-in needs MFA.
    pub mfa_trust_hours: i32,
}
