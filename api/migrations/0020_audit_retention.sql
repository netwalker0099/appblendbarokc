-- Retention for the audit log, without making it deletable.
--
-- The log only grows, so something has to give eventually. But "prune old audit
-- entries" as normally implemented — a DELETE on a schedule — throws away
-- exactly the records an investigation needs and quietly destroys the hash
-- chain's continuity while it does so. An append-only log with a delete job
-- attached is just a log.
--
-- So: **archive, deliver, then prune, in that order.** Entries past the
-- retention window are serialised, encrypted with the same pipeline the database
-- backups use, sent to every enabled backup destination, and only removed from
-- the hot table once at least one destination has confirmed the delivery. If
-- nothing can be delivered to, nothing is pruned — the table grows instead,
-- which is the safe failure.
--
-- What stays behind forever is a **segment record**: the id and time range of
-- what was archived, how many entries, the chain hash on each side of it, and a
-- checksum of the exported bytes. Those rows are tiny and are never pruned. They
-- are what lets a pruned log still verify, and what makes a *silent* deletion
-- still detectable — remove entries without recording a segment and the chain
-- reports a break, exactly as before.
--
-- ## The one sanctioned delete
--
-- The append-only trigger now permits DELETE while `blendbar.audit_archiving` is
-- set, which only the archiver sets, and only after delivery. This is a real
-- (small) weakening: anyone who can run SQL can set that flag. But anyone who can
-- run SQL could already drop the trigger outright, so the flag adds no capability
-- that was not there — and the hash chain, which is the actual defence, is
-- unaffected. Deleting without writing a segment record is still caught.

-- --- Retention setting ------------------------------------------------------
-- 0 means keep everything, and it is the default. Turning retention on should be
-- a decision somebody makes, not something that starts eating history because an
-- upgrade shipped with a default.
alter table settings
    add column audit_retention_days integer not null default 0
    check (audit_retention_days >= 0);

-- --- Archived segments ------------------------------------------------------
create table audit_archive_segments (
    id            bigserial primary key,

    -- Which entries went. Inclusive.
    from_id       bigint not null,
    to_id         bigint not null,
    entry_count   bigint not null,
    from_at       timestamptz not null,
    to_at         timestamptz not null,

    -- The chain on either side of the removed span. `anchor_prev_hash` is the
    -- prev_hash of the first archived entry — i.e. what the chain looked like
    -- immediately before this segment — and `last_entry_hash` is the entry_hash
    -- of the last one, which is what the first surviving row still points at.
    -- Together they let the verifier step across the gap without treating it as
    -- tampering, while still refusing to step across a gap nobody recorded.
    anchor_prev_hash text not null,
    last_entry_hash  text not null,

    -- SHA-256 of the exported (plaintext, pre-compression) bytes. This is what
    -- proves a recovered archive file is the one that was taken, and it is the
    -- reason the segment record is worth keeping even when the file is gone.
    content_sha256 text not null,

    filename      text not null,
    -- Where copies went: [{"destination": "...", "kind": "...", "remote_id": "..."}].
    -- Deliberately NOT recorded in `backup_runs`: database backups rotate under a
    -- retention count, and an archive segment that got rotated away would take
    -- the only copy of that history with it. Archives are permanent; backups are
    -- a rolling window.
    destinations  jsonb not null default '[]'::jsonb,

    created_at    timestamptz not null default now()
);

create index audit_archive_segments_range_idx on audit_archive_segments (from_id);
create unique index audit_archive_segments_head_idx on audit_archive_segments (last_entry_hash);

-- Segment records are themselves append-only. A retention system whose own
-- bookkeeping can be edited proves nothing.
create trigger audit_archive_segments_no_update
    before update on audit_archive_segments
    for each row execute function admin_audit_log_append_only();

create trigger audit_archive_segments_no_delete
    before delete on audit_archive_segments
    for each row execute function admin_audit_log_append_only();

-- --- Allow the archiver's delete -------------------------------------------
create or replace function admin_audit_log_append_only() returns trigger as $$
begin
    -- The single sanctioned removal path. Set only by the archiver, and only
    -- once the entries have been serialised, checksummed and delivered off this
    -- box. `current_setting(..., true)` returns null when unset, so the default
    -- is always refusal.
    if tg_op = 'DELETE'
       and tg_table_name = 'admin_audit_log'
       and coalesce(current_setting('blendbar.audit_archiving', true), '') = 'on'
    then
        return old;
    end if;

    raise exception '% is append-only (attempted %)', tg_table_name, tg_op
        using hint = 'Audit history cannot be changed. Entries are hash-chained; '
                     'altering one invalidates every entry after it. Old entries '
                     'are removed only by the archiver, after being delivered '
                     'off-box.';
end;
$$ language plpgsql;

-- --- Let a restored archive keep its own hashes -----------------------------
-- The chain trigger normally computes prev_hash/entry_hash, which is what stops
-- anyone appending an unchained row. But re-importing an archive must put the
-- *original* hashes back — recomputing them would produce a chain that verifies
-- against itself while no longer matching the segment record or the history it
-- came from, which is precisely the laundering this design exists to prevent.
--
-- So the trigger stands aside while `blendbar.audit_restoring` is set, and only
-- when the row already carries both hashes. A caller who sets the flag but
-- supplies no hashes still gets them computed, so the flag cannot be used to
-- sneak in an unchained entry.
create or replace function admin_audit_chain() returns trigger as $$
declare
    prev text;
begin
    if coalesce(current_setting('blendbar.audit_restoring', true), '') = 'on'
       and new.prev_hash is not null
       and new.entry_hash is not null
    then
        return new;
    end if;

    -- Serialises appends so two concurrent inserts cannot read the same
    -- predecessor and fork the chain. Transaction-scoped, released on commit.
    perform pg_advisory_xact_lock(hashtext('admin_audit_log'));

    select entry_hash into prev from admin_audit_log order by id desc limit 1;
    new.prev_hash := coalesce(prev, repeat('0', 64));
    new.entry_hash := admin_audit_entry_hash(
        new.prev_hash, new.at, new.actor_email, new.actor_role,
        new.method, new.path, new.status, new.summary, new.detail
    );
    return new;
end;
$$ language plpgsql;

-- --- Verification across archived gaps --------------------------------------
create or replace function verify_admin_audit_chain()
returns table (broken_id bigint, reason text) as $$
declare
    r record;
    expected_prev text := repeat('0', 64);
    recomputed text;
begin
    -- Walk the archived segments first. They chain to each other exactly as
    -- entries do, so a segment record that was forged or reordered shows up here
    -- rather than being taken on trust.
    for r in select * from audit_archive_segments order by from_id loop
        if r.anchor_prev_hash <> expected_prev then
            return query select r.from_id,
                format('archive segment %s does not continue the chain', r.id)::text;
        end if;
        expected_prev := r.last_entry_hash;
    end loop;

    -- Then the entries still in the table, continuing from wherever the last
    -- archive left off.
    for r in select * from admin_audit_log order by id loop
        if r.prev_hash <> expected_prev then
            return query select r.id,
                'previous-hash mismatch: an earlier entry was changed or removed'::text;
            -- Re-anchor on what this row claims so a single break does not report
            -- every later row as broken too. The first result is the one that
            -- matters; the rest would be noise.
            expected_prev := r.entry_hash;
            continue;
        end if;

        recomputed := admin_audit_entry_hash(
            r.prev_hash, r.at, r.actor_email, r.actor_role,
            r.method, r.path, r.status, r.summary, r.detail
        );
        if recomputed <> r.entry_hash then
            return query select r.id, 'contents do not match the stored hash'::text;
        end if;

        expected_prev := r.entry_hash;
    end loop;
    return;
end;
$$ language plpgsql stable;
