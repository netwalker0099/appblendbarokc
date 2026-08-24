-- An append-only, tamper-evident record of what staff did.
--
-- The app has had no answer to "who changed that price", "who deleted that
-- customer", or "who downloaded the database". Every table records its current
-- state and nothing records the act. That is a gap for an ordinary mistake —
-- somebody sets a price wrong and nobody can tell when or who — and a much
-- bigger one for anything deliberate.
--
-- ## What "immutable" means here, precisely
--
-- Three layers, because the app connects to Postgres as the `blendbar` owner and
-- an owner can do anything. No arrangement of grants makes this untouchable; the
-- goal is that history cannot be altered *silently*.
--
--   1. **Triggers refuse UPDATE, DELETE and TRUNCATE.** This stops the ordinary
--      accident and the casual edit. It does not stop someone who drops the
--      trigger first — which is the point of layer 2.
--
--   2. **A hash chain.** Each row stores the hash of the row before it, and its
--      own hash covers that link plus its own contents. Changing or removing any
--      historical row breaks every hash after it, and the break is detectable by
--      recomputing the chain (`verify_admin_audit_chain()`, exposed in the admin
--      UI). You cannot rewrite one entry without rewriting all of its
--      descendants.
--
--   3. **The chain is computed by the database, not the application.** A BEFORE
--      INSERT trigger fills in the hashes, so a row inserted by hand in psql is
--      chained exactly like one written by the API. If it were done in Rust,
--      anyone with a database connection could append an unchained row and the
--      verifier would have no idea.
--
-- What this deliberately does NOT claim: it is not proof against someone with
-- full database access who recomputes the whole chain from the point of their
-- edit onwards. Defeating that needs the hashes anchored somewhere outside this
-- box — periodically emailing the latest chain head, for example. That is a
-- reasonable later step; the honest description today is *tamper-evident*, not
-- *tamper-proof*.

create table admin_audit_log (
    -- bigserial, not uuid: the chain is an ordering, and it needs a total one
    -- that is obvious to read and cannot be reordered by a random id.
    id           bigserial primary key,
    at           timestamptz not null default now(),

    -- Who. The id can go null if an employee is later deleted, so the email and
    -- role are denormalised copies — an audit entry that becomes anonymous when
    -- someone is offboarded is useless exactly when it matters most.
    actor_id     uuid references employees (id) on delete set null,
    actor_email  text not null,
    actor_role   text not null,

    -- What.
    method       text not null,
    path         text not null,
    status       integer not null,

    -- Where from. Behind Caddy, so this is the forwarded client address.
    ip           text,
    user_agent   text,

    -- Redacted request body, plus a short human summary. Secrets never reach
    -- this column — see `audit::redact` in the API. A password or passphrase
    -- copied into the audit log would turn the log into the thing it protects
    -- against.
    summary      text,
    detail       jsonb,

    -- The chain. Filled in by the trigger below; never by the application.
    prev_hash    text not null,
    entry_hash   text not null unique
);

create index admin_audit_log_at_idx on admin_audit_log (at desc);
create index admin_audit_log_actor_idx on admin_audit_log (actor_email, at desc);

-- One definition of the hash, used by both the writer and the verifier. Two
-- copies of this formula would drift, and a verifier that disagrees with the
-- writer reports tampering that never happened — which is worse than no
-- verifier, because nobody believes the third false alarm.
create function admin_audit_entry_hash(
    prev text, at timestamptz, actor_email text, actor_role text,
    method text, path text, status integer, summary text, detail jsonb
) returns text as $$
    select encode(sha256(convert_to(
        prev
        || '|' || to_char(at at time zone 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US')
        || '|' || actor_email
        || '|' || actor_role
        || '|' || method
        || '|' || path
        || '|' || status::text
        || '|' || coalesce(summary, '')
        || '|' || coalesce(detail::text, ''),
        'UTF8')), 'hex');
$$ language sql immutable;

create function admin_audit_chain() returns trigger as $$
declare
    prev text;
begin
    -- Serialises appends so two concurrent inserts cannot read the same
    -- predecessor and fork the chain. Transaction-scoped, released on commit.
    perform pg_advisory_xact_lock(hashtext('admin_audit_log'));

    select entry_hash into prev from admin_audit_log order by id desc limit 1;
    -- The genesis link. A fixed, recognisable value so the start of the chain is
    -- unambiguous rather than null.
    new.prev_hash := coalesce(prev, repeat('0', 64));
    new.entry_hash := admin_audit_entry_hash(
        new.prev_hash, new.at, new.actor_email, new.actor_role,
        new.method, new.path, new.status, new.summary, new.detail
    );
    return new;
end;
$$ language plpgsql;

create trigger admin_audit_log_chain
    before insert on admin_audit_log
    for each row execute function admin_audit_chain();

-- Append-only enforcement.
create function admin_audit_log_append_only() returns trigger as $$
begin
    raise exception 'admin_audit_log is append-only (attempted %)', tg_op
        using hint = 'Audit history cannot be changed. Entries are hash-chained; '
                     'altering one invalidates every entry after it.';
end;
$$ language plpgsql;

create trigger admin_audit_log_no_update
    before update on admin_audit_log
    for each row execute function admin_audit_log_append_only();

create trigger admin_audit_log_no_delete
    before delete on admin_audit_log
    for each row execute function admin_audit_log_append_only();

-- Statement-level: TRUNCATE has no rows to fire a row-level trigger on, and is
-- the obvious way to erase the lot in one go.
create trigger admin_audit_log_no_truncate
    before truncate on admin_audit_log
    execute function admin_audit_log_append_only();

-- Recompute the chain and report the first entry that does not match.
--
-- Returns no rows when the log is intact. Each row it does return is an entry
-- whose stored hash disagrees with its recomputed one, or whose prev_hash does
-- not match the actual predecessor — a gap left by a deleted row shows up as the
-- latter.
create function verify_admin_audit_chain()
returns table (broken_id bigint, reason text) as $$
declare
    r record;
    expected_prev text := repeat('0', 64);
    recomputed text;
begin
    for r in select * from admin_audit_log order by id loop
        if r.prev_hash <> expected_prev then
            return query select r.id, 'previous-hash mismatch: an earlier entry was changed or removed'::text;
            -- Re-anchor on what this row claims, so one break does not report
            -- every subsequent row as broken too. The first result is the one
            -- that matters; the rest would be noise.
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
