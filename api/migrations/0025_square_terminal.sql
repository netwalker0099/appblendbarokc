-- In-person payment on a Square Terminal, driven from this app through Square's
-- cloud Terminal API.
--
-- The flow: Save intake builds a cart; "Charge on Terminal" creates a Square
-- order for it and sends a checkout to a paired Terminal over the internet. The
-- customer taps or inserts on the Terminal; Square reports the result by
-- webhook, and the screen also polls, so it flips to Paid on its own either way.
-- No card data and no Bluetooth device ever touches this app or the browser.

-- --- Paired Terminals ---------------------------------------------------------
-- A Terminal is paired by signing it in with a one-time device code that this
-- app asks Square for. Until the code is used the row is `pending`; once Square
-- reports it paired, `device_id` is what checkouts are sent to.
create table square_terminals (
    id             uuid primary key default gen_random_uuid(),
    name           text not null,
    -- Null until paired.
    device_id      text unique,
    -- The pairing code, its Square id, and when it stops working. Null for a
    -- Terminal entered by device id directly (the Sandbox test devices).
    device_code_id text unique,
    code           text,
    pair_by        timestamptz,
    status         text not null default 'pending'
        check (status in ('pending', 'paired', 'expired')),
    -- The Terminal "Charge on Terminal" uses. At most one.
    is_default     boolean not null default false,
    created_at     timestamptz not null default now(),
    paired_at      timestamptz,
    check (status <> 'paired' or device_id is not null)
);

create unique index square_terminals_one_default
    on square_terminals (is_default)
    where is_default;

-- --- Carts paid on a Terminal ---------------------------------------------------
-- How the cart is being paid: `link` (hosted page / QR, the existing path) or
-- `terminal`. A cart is one or the other, never both, because each creates its
-- own Square order and a cart has exactly one `square_order_id`.
alter table carts
    add column payment_method text
        check (payment_method in ('link', 'terminal')),
    add column terminal_checkout_id     text unique,
    add column terminal_device_id       text,
    -- Square's checkout status: PENDING, IN_PROGRESS, CANCEL_REQUESTED,
    -- CANCELED, COMPLETED. Kept so the screen can say "waiting for the customer"
    -- vs "customer canceled on the Terminal" without asking Square every time.
    add column terminal_status          text,
    add column terminal_cancel_reason   text;

-- Every cart sent to Square so far went by payment link.
update carts set payment_method = 'link' where square_order_id is not null;

-- --- Reconciliation retired -----------------------------------------------------
-- The report and its screen were removed on 2026-10-08 at the owner's request.
-- The saved runs table is left in place, not dropped: it is history, it costs
-- nothing, and dropping it is the one step here that can't be undone.
comment on table reconciliation_runs is
    'Retired 2026-10-08 with the reconciliation report. Read-only history; nothing writes here.';
