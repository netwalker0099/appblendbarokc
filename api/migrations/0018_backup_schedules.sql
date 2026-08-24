-- Scheduled, encrypted, off-box database backups.
--
-- `GET /api/admin/backup` already hands an admin a pg_dump on demand. That is a
-- manual pull: it protects against "I am about to change something risky", and
-- not at all against "the VPS is gone". Nobody remembers to click a button every
-- morning, and the backup you need is always the one from the day you forgot.
--
-- What lives here is the schedule and where to send it. What deliberately does
-- NOT live here:
--
--   * The encryption passphrase. It sits on the secrets volume, because a
--     passphrase stored in this database would be dumped INTO the very backups
--     it encrypts — anyone holding a backup file would hold its own key, and the
--     encryption would protect nobody.
--   * The Google service-account key, for the same reason. It is already on that
--     volume for email (see 0016) and is reused here.
--
-- Destination config that is NOT secret (a Drive folder id, a recipient address)
-- is fine in `config`, and being able to read it back is what makes the admin
-- page useful.

create table backup_destinations (
    id            uuid primary key default gen_random_uuid(),
    label         text not null,

    -- 'sharepoint' is accepted by the schema but has no backend yet: this
    -- deployment is a Google Workspace shop with no Microsoft tenant to upload
    -- into. The constraint names it now so adding the backend later is a code
    -- change and not a migration against live rows.
    kind          text not null check (kind in ('google_drive', 'email', 'sharepoint')),

    -- Per-kind, non-secret settings. google_drive: {folder_id, impersonate}.
    -- email: {to}.
    config        jsonb not null default '{}'::jsonb,

    -- Standard 5-field cron (minute hour day-of-month month day-of-week).
    -- 5-field because that is what people can paste from anywhere else; the
    -- seconds field the parser wants is prepended in Rust, so a schedule can
    -- never accidentally mean "every second of the matching minute".
    schedule      text not null,

    -- An IANA zone, evaluated per-run rather than stored as UTC. "Daily at 2am"
    -- has to stay 2am across a DST change; converting once at save time would
    -- silently become 1am or 3am for half the year.
    timezone      text not null default 'America/Chicago',

    -- Keep this many of the most recent backups at this destination; older ones
    -- this scheduler uploaded are deleted after a successful run. An hourly
    -- schedule is ~8,760 files a year otherwise, and it would quietly eat the
    -- Drive quota until an upload failed at 3am.
    retain_count  integer not null default 30 check (retain_count between 1 and 3650),

    enabled       boolean not null default true,

    -- Denormalised from backup_runs so the admin list is one query. The runs
    -- table remains the record of what actually happened.
    last_run_at   timestamptz,
    last_status   text check (last_status in ('ok', 'failed')),
    last_error    text,

    -- Null means "not scheduled yet"; the worker computes it at boot and after
    -- every run. Kept in the row rather than recomputed from last_run_at so a
    -- schedule edit takes effect immediately and a box that was powered off does
    -- not fire a burst of catch-up runs on boot.
    next_run_at   timestamptz,

    created_at    timestamptz not null default now(),
    updated_at    timestamptz not null default now()
);

-- The worker's hot query: due, enabled destinations.
create index backup_destinations_due_idx
    on backup_destinations (next_run_at)
    where enabled;

-- What actually happened, kept whether it worked or not. A backup system that
-- only records its successes is how you find out on the worst possible day that
-- it stopped running in March.
create table backup_runs (
    id             uuid primary key default gen_random_uuid(),
    destination_id uuid not null references backup_destinations(id) on delete cascade,

    -- 'manual' when someone pressed Run now, 'scheduled' when the cron fired.
    -- Worth distinguishing: a green history made entirely of manual runs means
    -- the schedule is not working.
    trigger        text not null default 'scheduled' check (trigger in ('scheduled', 'manual')),

    status         text not null check (status in ('running', 'ok', 'failed')),
    started_at     timestamptz not null default now(),
    finished_at    timestamptz,

    filename       text,
    -- Size of the encrypted artefact actually uploaded, not of the raw dump.
    bytes          bigint,

    -- The destination's own id for the uploaded file (a Drive file id). This is
    -- what makes retention possible: pruning deletes files this scheduler
    -- created and can still identify, so it can never delete somebody else's
    -- file that happens to sit in the same folder.
    remote_id      text,

    error          text
);

create index backup_runs_recent_idx on backup_runs (destination_id, started_at desc);
