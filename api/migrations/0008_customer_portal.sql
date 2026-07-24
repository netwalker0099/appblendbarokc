-- Customer portal: passwordless magic-link login. A short-lived, single-use login
-- token is emailed; consuming it mints a longer-lived session (separate from
-- employee sessions, cookie scoped to the customer site). Customers are matched to
-- existing `customers` rows by email (created during in-store intake).
create table customer_login_tokens (
    id           uuid primary key default gen_random_uuid(),
    token_hash   text not null unique,
    customer_id  uuid not null references customers (id) on delete cascade,
    expires_at   timestamptz not null,
    used_at      timestamptz,
    created_at   timestamptz not null default now()
);

create index customer_login_tokens_customer_idx on customer_login_tokens (customer_id);

create table customer_sessions (
    id           uuid primary key default gen_random_uuid(),
    token_hash   text not null unique,
    customer_id  uuid not null references customers (id) on delete cascade,
    expires_at   timestamptz not null,
    created_at   timestamptz not null default now()
);

create index customer_sessions_customer_idx on customer_sessions (customer_id);
