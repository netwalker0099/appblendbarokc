-- Multi-item intake, quantities, and package deals.
--
-- Until now one intake produced exactly one order, so a customer buying a 3.4oz
-- *and* a roller had to be entered twice — and there was no way to say "two of
-- these". Orders themselves were already the right grain (one blend, one size),
-- and carts already hold several of them; the limitation was purely in intake.

-- --- Quantity ---------------------------------------------------------------
-- Two bottles of the same blend in the same size is one order of quantity 2,
-- not two rows: the blend was mixed once. Cart lines carry it straight through.
alter table orders
    add column quantity integer not null default 1 check (quantity > 0);

-- --- Intakes ----------------------------------------------------------------
-- One submission, now several orders. Idempotency has to move up a level: it
-- used to sit on `orders.idempotency_key` with a unique constraint, which cannot
-- work once a single key legitimately produces multiple order rows.
--
-- The unique key on this table is what actually prevents a double-tap or a
-- retried request from charging a customer twice; the check-then-insert in the
-- handler alone would still race.
create table intakes (
    id              uuid primary key default gen_random_uuid(),
    customer_id     uuid not null references customers (id),
    idempotency_key text not null unique,
    created_by      uuid references employees (id),
    created_at      timestamptz not null default now()
);

alter table orders add column intake_id uuid references intakes (id);
create index orders_intake_id_idx on orders (intake_id);

-- Superseded by intakes.idempotency_key. Dedup state is transient — nothing
-- reads historical keys — so the column goes rather than lingering as a trap.
alter table orders drop column idempotency_key;

-- --- Package deals ----------------------------------------------------------
-- A bundle is a named set of bottles sold for one headline price: "Date Night —
-- two 3.4oz for $150". The price is on the bundle, not derived from its parts,
-- because the whole point is that it differs from the sum.
create table bundles (
    id          uuid primary key default gen_random_uuid(),
    name        text not null unique,
    description text,
    price       numeric(10, 2) not null check (price >= 0),
    active      boolean not null default true,
    created_at  timestamptz not null default now(),
    updated_at  timestamptz not null default now()
);

-- One component of a bundle. `scent_id` null means the customer chooses at
-- intake — which is how a bundle can contain "a custom blend" or "any house
-- scent" rather than naming one up front.
create table bundle_items (
    id         uuid primary key default gen_random_uuid(),
    bundle_id  uuid not null references bundles (id) on delete cascade,
    position   integer not null,
    type       text not null check (type in ('set_perfume', 'custom_mix')),
    size       text not null check (size in ('oz3_4', 'oz1_7', 'roller', 'spray')),
    scent_id   uuid references scents (id),
    quantity   integer not null default 1 check (quantity > 0),
    unique (bundle_id, position)
);

create index bundle_items_bundle_id_idx on bundle_items (bundle_id);

-- Which bundle an order came from, so a package sold as one thing can still be
-- recognised as one thing afterwards.
alter table orders add column bundle_id uuid references bundles (id);
create index orders_bundle_id_idx on orders (bundle_id);
