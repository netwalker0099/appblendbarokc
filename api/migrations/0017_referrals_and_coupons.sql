-- Referral discounts: buy through someone's shared link and you save; the person
-- whose link you used earns a coupon.
--
-- Both amounts are configurable rather than hard-coded at $5, because the right
-- number is a business decision that will change and should not need a deploy.

-- --- Who gets credited -------------------------------------------------------
-- A short, shareable code per customer. Generated on demand (the first time
-- someone shares), not for every customer ever created.
alter table customers add column referral_code text unique;

-- --- Settings ---------------------------------------------------------------
alter table settings
    add column referral_enabled          boolean not null default false,
    -- What the buyer saves at checkout.
    add column referral_discount_cents   bigint  not null default 500 check (referral_discount_cents >= 0),
    -- What the sharer earns, issued only once their referral actually pays.
    add column referral_reward_cents     bigint  not null default 500 check (referral_reward_cents >= 0),
    -- How long an earned coupon stays usable. 0 = never expires.
    add column coupon_expiry_days        integer not null default 365 check (coupon_expiry_days >= 0);

-- --- Coupons ----------------------------------------------------------------
-- A fixed amount off a future purchase, belonging to one customer.
--
-- `amount_cents` is copied from settings at issue time on purpose: changing the
-- reward next month must not silently re-value coupons already in people's
-- hands.
create table coupons (
    id             uuid primary key default gen_random_uuid(),
    customer_id    uuid not null references customers (id) on delete cascade,
    code           text not null unique,
    amount_cents   bigint not null check (amount_cents > 0),
    source         text not null check (source in ('referral_reward', 'manual')),
    status         text not null default 'active'
        check (status in ('active', 'redeemed', 'void')),
    -- Null means it never expires.
    expires_at     timestamptz,
    redeemed_at    timestamptz,
    redeemed_cart_id uuid references carts (id),
    note           text,
    created_at     timestamptz not null default now()
);

create index coupons_customer_idx on coupons (customer_id);
-- The lookup every redemption does.
create index coupons_active_code_idx on coupons (code) where status = 'active';

-- --- Referrals --------------------------------------------------------------
-- The audit of who introduced whom, and the guard against farming rewards.
create table referrals (
    id                   uuid primary key default gen_random_uuid(),
    referrer_customer_id uuid not null references customers (id) on delete cascade,
    referred_customer_id uuid not null references customers (id) on delete cascade,
    -- The purchase that triggered it.
    cart_id              uuid references carts (id) on delete set null,
    -- The coupon issued to the referrer, once the purchase actually paid.
    reward_coupon_id     uuid references coupons (id) on delete set null,
    created_at           timestamptz not null default now(),

    -- You cannot refer yourself.
    constraint referrals_not_self check (referrer_customer_id <> referred_customer_id),
    -- One reward per person introduced. Without this, a pair could ping-pong
    -- purchases and mint a coupon every time; the business would be paying a
    -- referral bounty on what is really one relationship.
    unique (referrer_customer_id, referred_customer_id)
);

create index referrals_referrer_idx on referrals (referrer_customer_id);

-- --- Carts carry what was applied -------------------------------------------
-- `total_cents` stays the amount actually charged. These record why it is lower
-- than the sum of the lines, so a cart can be explained months later and so
-- reconciliation against Square still adds up.
alter table carts
    add column discount_cents bigint not null default 0 check (discount_cents >= 0),
    add column coupon_id      uuid references coupons (id),
    -- The referral code the buyer arrived with, kept even if the referral row is
    -- later removed.
    add column referral_code  text;
