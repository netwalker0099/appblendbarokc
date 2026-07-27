-- Outbound email via the Google Workspace SMTP relay.
--
-- The app has never sent an email. The customer portal's magic-link sign-in
-- logs the link to the server log instead of mailing it (so the only way in has
-- been a dev bypass env var), and the online-order thank-you page promises "we'll
-- email you when it's ready" with nothing behind it.
--
-- Connection details are secrets and live in the environment. What lives here is
-- the part an admin should be able to change without a redeploy: who the mail
-- appears to come from, and which optional messages are switched on.

create table email_settings (
    id                  boolean primary key default true,
    -- Must be a mailbox on the Workspace domain; the relay refuses to send as a
    -- domain it does not own.
    from_address        text,
    from_name           text not null default 'The Blend Bar',
    -- Where replies go, if that should differ from the sender.
    reply_to            text,
    -- Sign-in links are not optional — they are the only way into the portal —
    -- so there is deliberately no toggle for them. This one is.
    order_ready_enabled boolean not null default true,
    updated_at          timestamptz not null default now(),
    constraint email_settings_singleton check (id)
);

insert into email_settings (id) values (true);

-- Delivery log: what was sent, to whom, and whether it landed.
--
-- Metadata only — **no message bodies**. A magic-link email contains a token that
-- grants a customer session; persisting the rendered body would leave a working
-- credential at rest in a second place, when the token is already stored hashed
-- in customer_login_tokens. Queued mail is re-rendered from ids at send time.
create table email_deliveries (
    id              uuid primary key default gen_random_uuid(),
    kind            text not null check (kind in ('magic_link', 'order_ready', 'test')),
    to_address      text not null,
    subject         text not null,
    status          text not null default 'pending'
        check (status in ('pending', 'sent', 'failed')),
    attempts        integer not null default 0,
    last_error      text,
    next_attempt_at timestamptz not null default now(),
    -- What to re-render from, for the queued kinds.
    customer_id     uuid references customers (id) on delete set null,
    order_id        uuid references orders (id) on delete set null,
    created_at      timestamptz not null default now(),
    updated_at      timestamptz not null default now(),
    sent_at         timestamptz
);

create index email_deliveries_due_idx
    on email_deliveries (next_attempt_at)
    where status = 'pending';

create index email_deliveries_created_idx on email_deliveries (created_at desc);

-- One "your blend is ready" per order, however many times staff press the button.
create unique index email_deliveries_order_ready_idx
    on email_deliveries (order_id)
    where kind = 'order_ready' and order_id is not null;
