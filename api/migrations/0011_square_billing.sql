-- Replace the Squarespace integration with Square as the billing system.
--
-- Squarespace was a downstream sink: we pushed contacts/orders at it and read
-- fulfilment status back, but money was never actually collected by this app —
-- an operator typed 'paid' at intake. Square inverts that. A *cart* (one or more
-- orders, plus any ad-hoc lines like an event deposit) is pushed to Square as an
-- Order + hosted payment link; the customer pays on Square's page; a webhook
-- brings the settled payment back. Square is the system of record for money,
-- this database stays the system of record for blends, and the two are
-- reconciled on `carts.square_order_id`.
--
-- Money note: Square speaks integer minor units (cents) + a currency code, never
-- decimals. Cart money is stored that way (`*_cents bigint`) so no rounding is
-- introduced on the way out. `orders.amount numeric(10,2)` is left alone — it is
-- the human-entered price on the intake screen, and the cart derives cents from
-- it at build time.

-- --- Drop the Squarespace surface -------------------------------------------
-- Only ever held mock ids ("mock_order_…") written by the in-process stub; no
-- real Squarespace record was ever created, so nothing of value is lost.
alter table orders drop column squarespace_order_id;

drop table webhook_events;

-- Contacts still sync outward, now to the Square Customers API, so the column is
-- renamed rather than dropped. Existing values are null (the mock never wrote a
-- contact id back), so no stale ids survive the rename.
alter table customers rename column squarespace_contact_id to square_customer_id;

-- The outbox now only pushes contacts. Orders reach Square through the cart
-- checkout path, which is synchronous (the operator needs the URL immediately),
-- not through the background worker.
delete from sync_outbox where entity_type = 'order';
alter table sync_outbox drop constraint sync_outbox_entity_type_check;
alter table sync_outbox add constraint sync_outbox_entity_type_check
    check (entity_type in ('contact'));

-- --- Carts ------------------------------------------------------------------
-- A cart is one checkout: the unit that becomes a single Square Order and a
-- single Square payment.
--
-- Lifecycle: open -> pending_payment -> paid
--                          |             |
--                          +-> canceled  +-> refunded
--
--   open             built locally, not yet sent to Square, still editable
--   pending_payment  pushed to Square, hosted link issued, awaiting the customer
--   paid             Square reported a COMPLETED payment for it
--   canceled         abandoned before payment (link voided or manually dropped)
--   refunded         was paid, then refunded in Square
create table carts (
    id                     uuid primary key default gen_random_uuid(),
    customer_id            uuid not null references customers (id),
    status                 text not null default 'open'
        check (status in ('open', 'pending_payment', 'paid', 'canceled', 'refunded')),
    currency               text not null default 'USD',

    -- What we asked Square to charge, summed from cart_items at checkout time.
    total_cents            bigint not null default 0 check (total_cents >= 0),
    -- What Square actually reported collecting. Divergence from total_cents is
    -- the headline reconciliation signal (partial payment, tip, price edited in
    -- the Square dashboard between link creation and payment).
    paid_cents             bigint,

    -- Square's ids. square_order_id is the join key for reconciliation; it is
    -- unique so a webhook can never fan out across two local carts.
    square_order_id        text unique,
    square_payment_link_id text,
    square_payment_id      text,
    checkout_url           text,

    -- Reused on every Square create call for this cart so a retry after a
    -- timeout returns the original payment link instead of minting a second one.
    idempotency_key        text not null unique,

    note                   text,
    created_by             uuid references employees (id),
    created_at             timestamptz not null default now(),
    updated_at             timestamptz not null default now(),
    checkout_at            timestamptz,
    paid_at                timestamptz
);

create index carts_customer_id_idx on carts (customer_id);
create index carts_status_idx on carts (status);
-- Drives the reconciliation window scan.
create index carts_paid_at_idx on carts (paid_at desc) where paid_at is not null;

-- A cart line. `order_id` links the line back to the blend it is selling; it is
-- nullable so a cart can also carry money that is not a bottle — an event
-- deposit, a rush fee, the multi-day hotel line from the booking terms.
--
-- name/unit_amount_cents are snapshotted at build time on purpose: repricing a
-- scent later must not retroactively change what a customer was charged.
create table cart_items (
    id                uuid primary key default gen_random_uuid(),
    cart_id           uuid not null references carts (id) on delete cascade,
    order_id          uuid references orders (id),
    name              text not null,
    quantity          integer not null default 1 check (quantity > 0),
    unit_amount_cents bigint not null check (unit_amount_cents >= 0),
    created_at        timestamptz not null default now()
);

create index cart_items_cart_id_idx on cart_items (cart_id);
-- An order can be claimed by at most one cart at a time — the double-billing
-- guard. A partial index cannot reach carts.status, so the claim is released
-- explicitly instead: cancelling a cart nulls its lines' order_id (keeping their
-- name and price for the record) and frees those blends to be sold again. See
-- `billing::cancel_cart`.
create unique index cart_items_order_idx
    on cart_items (order_id)
    where order_id is not null;

-- --- Inbound Square webhooks ------------------------------------------------
-- Same dedup/audit contract as the old Squarespace table: Square retries a
-- delivery until it gets a 2xx, so event ids are unique and an already-handled
-- redelivery is acked without reprocessing.
create table square_webhook_events (
    id               uuid primary key default gen_random_uuid(),
    square_event_id  text not null unique,
    event_type       text not null,
    square_order_id  text,
    square_payment_id text,
    status           text not null default 'received'
        check (status in ('received', 'processed', 'unmatched', 'ignored', 'failed')),
    matched_cart_id  uuid references carts (id),
    error            text,
    payload          jsonb not null,
    received_at      timestamptz not null default now(),
    processed_at     timestamptz
);

create index square_webhook_events_order_idx on square_webhook_events (square_order_id);
create index square_webhook_events_received_at_idx on square_webhook_events (received_at desc);

-- --- Reconciliation runs ----------------------------------------------------
-- A stored snapshot of each "do our books match Square's" comparison, so a
-- discrepancy found on the 5th is still inspectable on the 20th even though the
-- underlying rows have since been fixed up.
create table reconciliation_runs (
    id                uuid primary key default gen_random_uuid(),
    period_start      timestamptz not null,
    period_end        timestamptz not null,
    -- Totals in cents across the window.
    local_total_cents bigint not null default 0,
    square_total_cents bigint not null default 0,
    matched_count     integer not null default 0,
    mismatched_count  integer not null default 0,
    missing_in_square_count integer not null default 0,
    missing_locally_count   integer not null default 0,
    -- The full bucketed report, exactly as the endpoint returned it.
    report            jsonb not null,
    run_by            uuid references employees (id),
    created_at        timestamptz not null default now()
);

create index reconciliation_runs_created_at_idx on reconciliation_runs (created_at desc);
