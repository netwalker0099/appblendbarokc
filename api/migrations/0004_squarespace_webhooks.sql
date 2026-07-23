-- Inbound Squarespace webhooks (payment/fulfilment coming back from the POS).
-- Every verified notification is recorded here for dedup and audit: Squarespace
-- may redeliver, so the notification id is unique and a redelivery of an already
-- terminally-handled event is acked without reprocessing.
create table webhook_events (
    id                          uuid primary key default gen_random_uuid(),
    squarespace_notification_id text not null unique,
    topic                       text not null,
    squarespace_order_id        text,
    status                      text not null default 'received'
        check (status in ('received', 'processed', 'unmatched', 'ignored', 'failed')),
    matched_order_id            uuid references orders (id),
    error                       text,
    payload                     jsonb not null,
    received_at                 timestamptz not null default now(),
    processed_at                timestamptz
);

create index webhook_events_order_id_idx on webhook_events (squarespace_order_id);
create index webhook_events_received_at_idx on webhook_events (received_at desc);
