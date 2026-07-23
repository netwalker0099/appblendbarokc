-- Transactional outbox for pushing customers/orders to Squarespace. Rows are
-- enqueued in the same transaction that writes the customer/order, so a sync is
-- never lost to a crash between the DB commit and the external call. A background
-- worker drains pending rows, writes the returned id back onto the entity, and
-- marks the job succeeded (or failed after exhausting retries).
create table sync_outbox (
    id               uuid primary key default gen_random_uuid(),
    entity_type      text not null check (entity_type in ('contact', 'order')),
    entity_id        uuid not null,
    status           text not null default 'pending' check (status in ('pending', 'succeeded', 'failed')),
    attempts         integer not null default 0,
    last_error       text,
    next_attempt_at  timestamptz not null default now(),
    created_at       timestamptz not null default now(),
    updated_at       timestamptz not null default now()
);

-- At most one outstanding job per entity: a repeat intake for the same customer
-- just bumps the existing pending row instead of piling up. Succeeded/failed rows
-- fall out of this index, so a later change re-enqueues cleanly.
create unique index sync_outbox_pending_entity_idx
    on sync_outbox (entity_type, entity_id)
    where status = 'pending';

-- The worker's claim query: due pending jobs, oldest first.
create index sync_outbox_due_idx
    on sync_outbox (next_attempt_at)
    where status = 'pending';
