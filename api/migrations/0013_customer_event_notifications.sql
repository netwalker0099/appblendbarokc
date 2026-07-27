-- Outbound chat notifications (Discord / Slack / Teams) for things a CUSTOMER
-- does, not things staff do.
--
-- The distinction matters: staff at the bar already know they just rang up a
-- sale, and a notification for it is noise that trains people to ignore the
-- channel. What nobody is watching for is money arriving when no one is at the
-- till — an online purchase from a shared scent link, or an event deposit
-- landing. Those are the two events worth interrupting someone for.

-- --- Classify cart lines ----------------------------------------------------
-- Ad-hoc lines were free text ("Event deposit (50%)"), which is fine for a
-- receipt and useless for triggering on. Matching a deposit by string would
-- break the first time someone retyped the label. `kind` makes it explicit.
alter table cart_items
    add column kind text not null default 'blend'
    check (kind in ('blend', 'event_deposit', 'fee', 'other'));

-- Existing lines: anything tied to an order is a blend (already the default);
-- best-effort classify the historical free-text lines by their label.
update cart_items set kind = 'event_deposit'
    where order_id is null and name ilike '%deposit%';
update cart_items set kind = 'fee'
    where order_id is null and kind = 'blend' and (name ilike '%fee%' or name ilike '%hotel%');
update cart_items set kind = 'other'
    where order_id is null and kind = 'blend';

-- --- Where to send ----------------------------------------------------------
create table notification_targets (
    id                     uuid primary key default gen_random_uuid(),
    label                  text not null,
    platform               text not null check (platform in ('discord', 'slack', 'teams')),
    -- Held server-side only and never returned to the browser: a webhook URL is
    -- a bearer credential — anyone holding it can post into the channel.
    webhook_url            text not null,
    active                 boolean not null default true,

    notify_online_sale     boolean not null default true,
    notify_event_booked    boolean not null default true,
    -- Off by default. The order details are enough to act on, and a chat channel
    -- is a third party: sending a customer's email there should be a decision
    -- someone makes, not something that happens quietly.
    include_customer_email boolean not null default false,

    created_by             uuid references employees (id),
    created_at             timestamptz not null default now(),
    updated_at             timestamptz not null default now(),
    last_success_at        timestamptz,
    last_error             text
);

-- --- Delivery log -----------------------------------------------------------
-- A queue, an audit trail, and the dedup guard in one table. Sending happens on
-- the background worker, never on the payment path: a chat outage must not be
-- able to fail a customer's checkout.
create table notification_deliveries (
    id              uuid primary key default gen_random_uuid(),
    target_id       uuid not null references notification_targets (id) on delete cascade,
    event_type      text not null check (event_type in ('sale.online', 'event.booked')),
    cart_id         uuid not null references carts (id) on delete cascade,
    status          text not null default 'pending' check (status in ('pending', 'sent', 'failed')),
    attempts        integer not null default 0,
    last_error      text,
    next_attempt_at timestamptz not null default now(),
    created_at      timestamptz not null default now(),
    updated_at      timestamptz not null default now(),

    -- One notification per target per cart per event, forever. Payment
    -- settlement is idempotent and can be re-applied (webhook redelivery, a
    -- staff member pressing "Check Square"), so without this a customer paying
    -- once could ping the channel repeatedly.
    unique (target_id, cart_id, event_type)
);

create index notification_deliveries_due_idx
    on notification_deliveries (next_attempt_at)
    where status = 'pending';
