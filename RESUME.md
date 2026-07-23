# Blend Bar — Resume Notes

Last updated: 2026-07-23, after Milestone 6 (Squarespace webhook receiver, mock).

Read this first in a new session, then README.md for deploy mechanics.

## Git: committed and pushed to GitHub

`master` is committed and now pushed to **`git@github.com:netwalker0099/appblendbarokc.git`**
(remote `origin`, tracking set up). Auth is an **SSH deploy key** generated on this
VPS: private key `~/.ssh/blendbar_deploy`, pinned for github.com in `~/.ssh/config`
(`IdentitiesOnly yes`), public half registered as a write-enabled deploy key on the
repo. So `git push` from this box just works; there is no token stored anywhere.
`git log origin/master` should match local. This is no longer VPS-disk-only.

## Where this lives

This repo lives directly on the target VPS at `/opt/app` (hostname `app`, Ubuntu
26.04). Docker, the Compose stack, and all validation in Milestones 1–3 have been
run for real on this box, not in a separate sandbox.

## Status: Milestones 1–7 done and validated live on this VPS

(Milestones 5 and 6 — Squarespace push sync and the inbound webhook receiver — are
built and validated **against the mock**; their live Squarespace HTTP paths are
untested because there's still no API key or real webhook secret. See their entries
below. All planned milestones are now complete.)

- **Milestone 1 (scaffold):** Compose (`db`/`api`/`caddy`), multi-stage Dockerfiles,
  Caddyfile, `.env.example`, README. `docker compose up --build` brings up all three
  healthy. **DNS is now pointed at this VPS and TLS is live** — as of 2026-07-22,
  `https://app.theblendbarokc.com` serves a real production Let's Encrypt cert
  (verified via `curl`, `/api/health` returns 200 through Caddy). Caddy had
  auto-fallen back to the LE *staging* CA during the earlier NXDOMAIN period;
  a `docker compose restart caddy` after DNS propagated pulled a production cert
  immediately. If TLS ever looks untrusted, check which CA the logs name.
  - **Gotcha when testing from this box:** `/etc/hosts` maps
    `app.theblendbarokc.com` to `127.0.1.1` (Ubuntu's FQDN line), so local `curl`
    hits loopback and does *not* prove external reachability. For a real
    external-path test use
    `curl --resolve app.theblendbarokc.com:443:64.177.120.80 ...`.
- **Milestone 2 (schema):** `api/migrations/0001_init.sql` — customers, ingredients,
  scents, customer_scent_preferences, mixes, mix_items, orders. Rust models in
  `api/src/models/`. Migrations run automatically at API boot via `sqlx::migrate!`
  and are confirmed idempotent across restarts.
- **Milestone 3 (auth + CRUD + intake):** `api/migrations/0002_operator_auth_and_idempotency.sql`
  adds `operator_devices` + `orders.idempotency_key`. Full CRUD for
  customers/ingredients/scents/mixes/orders, all behind bearer-token auth
  (`/api/health` stays open). `POST /api/intake` is the one-shot stand submission,
  idempotent on a required `Idempotency-Key` header. All of this was exercised live
  against the running containers — see the validation list below.
- **Milestone 4 (operator UI):** Vue 3 + vue-router SPA in `web/`. Device pairing
  screen (token in `localStorage`, 401 forces re-pair), intake form with the mix
  builder, and a lookup view with customer history and one-tap reorder. Backend
  needed **no changes** — everything runs on the Milestone 3 endpoints. Validated
  by driving a real headless browser through pair → intake → submit → lookup →
  reorder against the live site; see `web/smoke.js`.
- **Milestone 7 (reorder endpoint):** `GET /api/customers/:id/reorder`
  (`api/src/routes/customers.rs::reorder`) returns `{ customer, mixes, orders }`
  in one round trip — mixes come with their `items` already attached
  (`MixDetail`, items bucketed in Rust from one `mix_id = any($1)` query, no N+1).
  This replaced the lookup view's old customer → list-mixes → get-each-mix
  fan-out; `LookupView.select()` now makes a single `api.getReorder()` call.
  Validated live: 200 with items matching `GET /api/mixes/:id`, empty-mix
  customer returns `[]` (not an error), 404 on unknown id, 401 unauthenticated.
- **Milestone 5 (Squarespace sync — mock-validated):** a transactional-outbox
  push layer behind a mockable trait. Postgres stays source of truth; Squarespace
  is a downstream sink.
  - `api/src/squarespace/` — the `Squarespace` trait (`upsert_contact`,
    `create_order`), a `MockSquarespace` (deterministic `mock_contact_<uuid>` /
    `mock_order_<uuid>` ids, never fails), and `HttpSquarespace` (reqwest+rustls).
    `from_env()` picks the HTTP client when `SQUARESPACE_API_KEY` is set, else the
    mock — the box runs the **mock** today (no key). Selected once at startup into
    `AppState.squarespace: Arc<dyn Squarespace>`.
  - `api/migrations/0003_squarespace_sync_outbox.sql` — `sync_outbox` table.
    Partial unique index `(entity_type, entity_id) where status='pending'` means a
    repeat intake/patch bumps the existing pending row instead of duplicating; the
    enqueue uses `on conflict … do update set next_attempt_at=now()`.
  - `api/src/sync.rs` — `enqueue()` (transactional) + `run_worker()`: polls every
    5s, drains due pending jobs, calls the backend, writes the id back onto the
    customer/order, marks succeeded; on retryable error backs off exponentially
    (10s,20s,40s…) up to `MAX_ATTEMPTS=6` then marks `failed`. Delivery is
    at-least-once — `sync_order` skips create when `squarespace_order_id` is
    already set, so a write-back crash can't double-create an order.
  - Enqueue points: intake enqueues contact+order **inside the intake tx**;
    `customers.rs::update` (PATCH) re-enqueues the contact so consent/name changes
    propagate.
  - `GET /api/sync/status` (backend + counts + recent failures) and
    `POST /api/sync/retry` (requeue all failed) — `api/src/routes/sync.rs`.
  - **Validated live against the mock:** intake → both jobs drained → `mock_*` ids
    written back to customer & order; `/sync/status` showed `succeeded`; 3 rapid
    PATCHes produced exactly 1 pending contact job (dedup); idempotent intake
    replay added no second order outbox row; `/sync/retry` returned 0 with no
    failures. The failure/backoff/`failed`-status path is code-only (the mock
    can't fail) — exercise it once a real key exists, or by pointing at a bad key.
  - **Untested & to check when a key lands:** `HttpSquarespace` endpoint paths
    (`/profiles`, `/commerce/orders`), request bodies, and which response field
    holds the created id — all marked with a warning comment in `http.rs`. Also
    note rustls has two versions in the tree now (sqlx + reqwest); the reqwest
    client is never even constructed under the mock, so no crypto-provider issue
    shows up until the live path is used — verify it there.
- **Milestone 6 (Squarespace webhook receiver — mock-validated):** inbound
  payment/fulfilment reconciliation. Payment is taken via the Squarespace POS, so
  Squarespace webhooks tell us when an order is paid/fulfilled.
  - `POST /api/webhooks/squarespace` (`api/src/routes/webhooks.rs`) is **public but
    HMAC-verified** — it's in the *open* router (Squarespace can't send an operator
    token), not behind the bearer middleware. Verifies HMAC-SHA256 of the raw body
    against `SQUARESPACE_WEBHOOK_SECRET` (constant-time via `mac.verify_slice`).
    **Unset secret ⇒ receiver disabled, returns 503** (`AppError::Unavailable`);
    `AppState.webhook_secret: Option<Arc<str>>` loaded in main.rs.
  - Flow: verify sig → dedup/audit in `webhook_events` (migration 0004; unique
    notification id, statuses received/processed/unmatched/ignored/failed) →
    `order.*` topics fetch authoritative state via the new
    `Squarespace::get_order` (mock returns paid+PENDING⇒maps to 'paid'; real client
    GETs `/commerce/orders/{id}`, untested) → `update orders … where
    squarespace_order_id = $1` (the id M5 stored) → settle the event. No local
    match ⇒ 'unmatched' (order taken directly in POS), not an error. Transient
    `get_order` failure ⇒ 500 so Squarespace redelivers; a redelivered
    already-terminal notification is acked 200 without reprocessing.
  - `GET /api/webhooks/recent` (authed) lists recent events for debugging.
  - **Validated live against the mock:** signed `order.update` flipped a 'lead'
    order to 'paid' (event 'processed', matched); redelivery stayed at 1 row / one
    process; bad signature ⇒ 401 with nothing recorded; unknown order id ⇒
    'unmatched'; non-order topic ⇒ 'ignored'.
  - **Untested & to check when a real webhook secret/key land:** the signature
    header name (`Squarespace-Signature`) and encoding (hex) — a documented guess
    in `verify_signature`; and `HttpSquarespace::get_order`'s response field
    mapping. The dev secret `dev_webhook_secret_change_me` is set in `.env` (git-
    ignored) purely so the receiver is enabled for testing — replace it.
- **Admin section (`/admin`, operator-authed):** catalog + integration UI.
  `web/src/views/AdminView.vue` + reusable `components/CatalogManager.vue`. Adds
  and activates/deactivates ingredients **and** scents from the UI (this is where
  "add ingredient" now lives — the mix builder just consumes the active catalog).
  Plus a Squarespace integration panel: push backend (mock/live), webhook-receiver
  enabled state, sync job counts + "retry failed" (`POST /api/sync/retry`), and
  recent inbound webhooks (`GET /api/webhooks/recent`). Uses existing CRUD; the
  only API change was adding `webhook_receiver_enabled` to `/api/sync/status`.
  Server-side secrets are deliberately not settable from the UI. No role model yet
  — any paired device can reach `/admin` (user-level auth is still deferred).
- **Mix-builder editing (earlier session):** each mix row is now an editable
  `<select class="name">` so an operator can swap an ingredient in place without
  losing its amount (`MixBuilder.vue::setIngredient` / `optionsFor`). A row's
  options are every active ingredient minus the ones other rows already use, with
  its own current ingredient always folded back in (even if since deactivated —
  shown "(inactive)"). The row `:key` moved from `item.ingredient_id` to `index`
  because the ingredient id can now change mid-edit. Amounts are labelled in **ml**
  (a visible `.unit` span per row + a header note), which is what the API has
  always stored. `smoke.js` was updated to read the row's selected option instead
  of a static span's text.

## Decisions locked in — don't re-litigate these

- **Rust stack:** Axum + sqlx 0.8 (rustls, not native-tls/OpenSSL). Queries use
  runtime `sqlx::query_as`, not the compile-time-checked `sqlx::query!` macros —
  deliberate, to avoid needing a live DB or an offline query cache during
  `docker build`.
- **Payment:** both Squarespace Tap to Pay and a Square Reader are in use at the
  stand. Both route through the same Squarespace POS `order.create` webhook, so
  reconciliation logic (Milestone 6) doesn't need to branch on which was used.
- **Operator auth:** per-device bearer token now (table `operator_devices`,
  SHA-256-hashed token, `label` field). Not user-level auth yet — that's an
  explicitly deferred future step, `label` is the only seed for it today.
  Issue a token with:
  ```
  docker compose exec api blendbar-api issue-device-token "<device label>"
  ```
  The raw token is printed once and never stored in retrievable form — if it's
  lost, issue a new one and deactivate the old row (`active = false`) manually.
- **Domain:** `app.theblendbarokc.com`. DNS is **not** pointed at this VPS yet.
- **Marketing consent:** single opt-in (no confirmation email flow).
- **Mix ratios:** milliliters, not percentages. Max 8 ingredients per mix. The
  formula is defined at the 3.4oz bottle size; the 1.7oz bottle is half those
  amounts and the roller is a tenth — both derived at read/order time, never
  stored per-size. The 8-ingredient cap and all mix validation live in Rust
  (`api/src/routes/ingredients.rs::assert_active_ingredients`), not as a DB
  constraint — deliberate, single write path, not worth a trigger.
- **Ingredient catalog:** fully editable via CRUD, no fixed seed list — add
  ingredients as you stock them.
  - **Scent "preferences":** catalog-only model. The `scents` table (editable,
  you have 18 today) is used both for `customer_scent_preferences` (what a
  customer likes) and as the picker for `set_perfume` orders (`orders.scent_id`).
  There are no separate typed preference fields (no intensity/family/allergies) —
  that was an explicit choice, not an oversight.

## What's actually running on the VPS right now

- Docker Engine 29.6.2 + Compose v5.3.1, installed via Docker's official apt repo,
  systemd-enabled (`docker.service` starts on boot).
- `docker compose up -d` stack is up: `db`, `api`, `caddy` — check with
  `docker compose ps`; restart policy is `unless-stopped` so a VPS reboot should
  self-heal, but verify after any long gap.
- `.env` exists on disk, copied from `.env.example` with placeholder/dev values
  (`POSTGRES_PASSWORD=changeme`, etc.) — **not production-ready secrets.**
- `OPERATOR_AUTH_SECRET` is still wired into `docker-compose.yml` and
  `.env.example` but is **dead** — grep confirms no Rust code reads it. It predates
  the per-device-token design landing. Safe to delete from both files whenever
  convenient; harmless to leave.
- **Fixture/test data is in the live DB** from Milestone 3 validation:
  - customers: `visitor@example.com` (Jamie Visitor), `edge5@example.com`
  - ingredients: Bergamot, Sandalwood
  - scents: Golden Hour
  - one `custom_mix` order and one `set_perfume` order, both status `paid`
  - one operator device token issued, labeled "Stand iPad" (the raw token from
    that session is gone — issue a fresh one if you need to authenticate a client)

  Milestone 4 added more throwaway rows on top of that: `m4probe@example.com`,
  `m4flow@example.com`, and one `smoke…@example.com` customer + order per smoke-test
  run (each with a one-ingredient mix). Device tokens labelled `dev-session-*` and
  `smoke` were also issued and are still active.

  **Still unanswered, and now more urgent:** wipe all of this before launch, or keep
  it as fixtures? The smoke test adds a row every run, so this only grows. Deactivate
  the stray device tokens (`update operator_devices set active = false where label
  like 'dev-session-%' or label = 'smoke'`) whenever you clean up.

## Frontend decisions locked in (Milestone 4)

- **Operator-driven, not a customer kiosk.** Staff hold the tablet, so the UI shows
  order status and amount. A customer-facing self-serve mode was considered and
  explicitly not built.
- **vue-router in history mode.** Caddy's `try_files {path} /index.html` already
  serves deep links; a guard bounces every route but `/pair` when no token is stored.
- **Base-formula entry.** The builder takes 3.4oz amounts and displays the 1.7oz /
  roller amounts derived (`web/src/lib/bottle.js`), matching how the API stores them.
- **`step="any"` on the mix amount input** — do not "tidy" this to a fixed step.
  With `step="0.1"` and `min="0.01"` the browser silently refuses to submit round
  numbers like `1`, which is exactly what the builder defaults to. This bug was
  found by the smoke test and it fails invisibly.
- **Style hooks are class-based** (`.primary` / `.ghost` / `.icon`, not
  `button.primary`) so `RouterLink` anchors pick up the same styling as buttons.

## Not started

All seven planned milestones are complete. What remains is going live for real:
- Obtain a Squarespace **API key** → set `SQUARESPACE_API_KEY`, restart; the sync
  layer switches from mock to `HttpSquarespace`. Verify its untested request
  shapes (see M5 entry).
- Obtain the real webhook **signing secret** for the subscription → replace the
  dev value in `.env`; verify the signature header/encoding and `get_order`
  mapping (see M6 entry). Register the webhook subscription in Squarespace.

## Open items nobody has answered yet

- **Squarespace API key still not obtained.** `SQUARESPACE_API_KEY` in `.env` is
  blank, so the app runs the sync mock. Once set + `docker compose up -d`, the
  live `HttpSquarespace` path takes over — but its request shapes are unverified
  (see the M5 entry) and there are stale `mock_*` ids already written on existing
  rows that a real sync won't overwrite for orders (contacts re-upsert fine).
- **Webhook signing secret is a dev placeholder.** `SQUARESPACE_WEBHOOK_SECRET`
  in `.env` is `dev_webhook_secret_change_me` so the receiver is enabled for
  testing. Replace with the real subscription secret before going live, and
  register the subscription on the Squarespace side.
- Whether to wipe or keep the fixture data described above (now also includes M5/M6
  sync-test customers/orders carrying `mock_*` ids and rows in `webhook_events`).

## How to pick this back up

1. `cd /opt/app && git status` — see whether anything's changed since this was
   written; commit first if not already done.
2. `docker compose ps` — confirm the stack is still healthy.
3. Skim this file and `README.md`. All seven milestones are done; the remaining
   work is going live (see "Not started" above): get the Squarespace API key +
   real webhook secret, swap them in, and verify the two untested live HTTP paths
   (`HttpSquarespace` push/`get_order` and the webhook signature wire format).

Note: validation across sessions left several deactivated device tokens
(`m5-validate`, `m6-validate`, `verify`, `m7-validate`, `smoke`) and a deactivated
`Vetiver (swap-test)` ingredient in the DB, plus mock-synced test customers/orders
and `webhook_events` rows — all part of the fixture-cruft-vs-wipe question above.
