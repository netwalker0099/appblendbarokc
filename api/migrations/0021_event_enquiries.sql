-- Event booking enquiries from the public site (/book on the customer site).
--
-- Until now the app captured nothing about an event until a deposit settled:
-- the site pointed people at Instagram DMs, and `event.booked` fired at payment
-- because that was the first moment this system knew an event existed at all.
-- This is the missing front half — the enquiry itself, stored here and
-- announced to the team's chat channel.

create table event_enquiries (
    id          uuid primary key default gen_random_uuid(),

    first_name  text not null,
    last_name   text not null,
    email       text not null,
    phone       text not null,
    -- A date, not a timestamp. Nobody enquires about an instant, and storing a
    -- timestamptz would drag the submitter's timezone into a field that means
    -- "the 14th of March" in Oklahoma regardless of where it was typed.
    event_date  date not null,
    details     text not null,

    -- Workflow state. Deliberately small: this is a list someone works through,
    -- not a CRM. 'new' is what the public endpoint writes; everything else is a
    -- staff decision.
    status      text not null default 'new'
                check (status in ('new', 'contacted', 'booked', 'declined')),
    -- Internal only. Never returned by any public route, never echoed back to
    -- the person who submitted the enquiry.
    staff_notes text,

    handled_by  uuid references employees (id),
    handled_at  timestamptz,

    created_at  timestamptz not null default now(),
    updated_at  timestamptz not null default now()
);

-- The working view is "what came in, newest first", and the common filter is
-- "not dealt with yet".
create index event_enquiries_created_idx on event_enquiries (created_at desc);
create index event_enquiries_open_idx on event_enquiries (created_at desc)
    where status in ('new', 'contacted');

-- --- Let a notification be about something other than a cart -----------------
--
-- notification_deliveries was built when every notifiable event was a payment,
-- so cart_id was `not null` and the dedup key was (target, cart, event). An
-- enquiry has no cart and never will — nobody has paid anything. Rather than
-- invent a placeholder cart, the subject becomes one-of: exactly one of
-- cart_id / enquiry_id is set, enforced below.

alter table notification_deliveries
    alter column cart_id drop not null;

alter table notification_deliveries
    add column enquiry_id uuid references event_enquiries (id) on delete cascade;

-- Exactly one subject. Without this the table would accept a row that is about
-- nothing (both null) or about two different things at once (both set), and
-- the delivery worker would have to guess which one it meant.
alter table notification_deliveries
    add constraint notification_deliveries_one_subject
    check (num_nonnulls(cart_id, enquiry_id) = 1);

alter table notification_deliveries
    drop constraint notification_deliveries_event_type_check;
alter table notification_deliveries
    add constraint notification_deliveries_event_type_check
    check (event_type in ('sale.online', 'event.booked', 'event.enquiry'));

-- The old dedup key was a table constraint over a now-nullable column, and in
-- Postgres `unique (target_id, cart_id, event_type)` stops guarding anything the
-- moment cart_id is null — every null row is distinct from every other. Two
-- partial indexes restore the guarantee on each subject separately.
alter table notification_deliveries
    drop constraint notification_deliveries_target_id_cart_id_event_type_key;

create unique index notification_deliveries_cart_uniq
    on notification_deliveries (target_id, cart_id, event_type)
    where cart_id is not null;

create unique index notification_deliveries_enquiry_uniq
    on notification_deliveries (target_id, enquiry_id, event_type)
    where enquiry_id is not null;

-- --- Per-target opt-in -------------------------------------------------------
-- On by default, matching the other two: a team that has configured a channel
-- for bookings wants to hear about the enquiry that precedes one.
alter table notification_targets
    add column notify_event_enquiry boolean not null default true;
