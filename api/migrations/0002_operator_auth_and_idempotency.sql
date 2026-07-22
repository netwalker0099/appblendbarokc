create table operator_devices (
    id             uuid primary key default gen_random_uuid(),
    label          text not null,
    token_hash     text not null unique,
    active         boolean not null default true,
    created_at     timestamptz not null default now(),
    last_used_at   timestamptz
);

alter table orders add column idempotency_key text unique;
