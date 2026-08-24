# Blend Bar — Resume Notes

Last updated: 2026-07-27 — **billing migrated from Squarespace to Square**
(Milestones 9 and 10 below): carts → Square hosted checkout → webhook →
reconciliation, plus the public buy button on share pages.
Built and tested against the mock; no real Square credentials on this box yet, so
nothing has been verified against the live service. Booking/cancellation terms also
added to the sandbox site's events section.

Previously: 2026-07-24 — DB wiped clean for real-data entry (all 7 milestones plus
ingredient types & scent formulas done; see history below).

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
- **Ingredient types (Base / Top Note / Heart Note):** every ingredient has a
  perfumery `type` (migration 0006: `ingredients.type` text, NOT NULL default
  `heart_note`, checked in `('base','top_note','heart_note')`; seeded Bergamot→
  top_note, Sandalwood→base). `IngredientType` enum in `models/ingredient.rs`
  (field `ingredient_type`, `#[sqlx/serde(rename="type")]`, JSON key `type`).
  Create **requires** `type` (422 if missing/invalid); update takes optional
  `type`. Frontend: `INGREDIENT_TYPES` in `lib/bottle.js`; admin `CatalogManager`
  gained an add-time type picker + per-row type select + type label (sorted by
  type); `MixBuilder` groups both its pickers with `<optgroup>` by type — so the
  grouping applies to custom mixes **and** scent formulas (ScentManager reuses
  MixBuilder). Validated live (API: list/create/missing-422/invalid-422/update;
  browser: add-with-type, edit type, picker optgroups render).
- **Scent formulas (recipes):** set-perfume scents now carry an ingredient
  breakdown like custom mixes. `scent_items` table (migration 0005, same shape/
  rules as `mix_items`; formula may be empty = "not defined yet"). `scents.rs`
  `list`/`get` return `ScentDetail` (scent + items, flattened — backward compatible
  with existing name/active consumers); `create`/`update` accept optional `items`
  (reusing `MixItemInput` and `assert_active_ingredients`, transactional
  delete+reinsert). The formula is referenced **live** from the scent, not
  snapshotted per order — editing a scent changes what past set-perfume orders
  display. Admin edits it via `components/ScentManager.vue` (reuses `MixBuilder`);
  `LookupView` and `IntakeView` show the selected scent's breakdown. Validated
  live (API: create/get/list/update/empty/duplicate-reject/order; browser: admin
  editor saves, intake readout renders).
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
- **Pour chart (all sizes):** `MixBuilder` shows, below the ingredient rows, a
  table of each ingredient + total at all three bottle sizes (3.4oz base, 1.7oz =
  ½, roller = ⅒), derived live from the base amounts (`web/src/lib/bottle.js`
  `scaleMl`). The size selected for the order is highlighted. Because ScentManager
  reuses MixBuilder, scent formulas get the same chart. Styles: `.pour-chart` in
  `styles.css` (horizontally scrollable, tabular-nums).
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
- **Secrets hardened 2026-07-24** (`.env`, git-ignored): `POSTGRES_PASSWORD` is now
  a 24-byte random hex (rotated live via `ALTER USER blendbar` + matching
  `DATABASE_URL`, not the old `changeme`), and `SQUARESPACE_WEBHOOK_SECRET` is a
  32-byte random hex (was the `dev_…` placeholder). The webhook secret still must
  be set to match whatever signing secret Squarespace generates when the
  subscription is actually registered. `SQUARESPACE_API_KEY` remains blank (mock).
- `OPERATOR_AUTH_SECRET` (dead, unread by any code) was **removed** from `.env`,
  `.env.example`, and `docker-compose.yml`.
- **The DB was wiped clean on 2026-07-24** to start entering real data. All
  business tables (`customers`, `orders`, `mixes`, `mix_items`, `scents`,
  `scent_items`, `ingredients`, `customer_scent_preferences`, `webhook_events`,
  `sync_outbox`) were `TRUNCATE … CASCADE`'d to zero rows — the ingredient and
  scent **catalogs are empty**, so real ingredients (with types) and scents must
  be added via `/admin` before intake can build mixes / take set-perfume orders.
  - `operator_devices` was **deliberately kept** (4 active tokens preserved), so
    an already-paired tablet stays paired. Raw values of those tokens are not
    recoverable — issue a fresh one if a device needs to pair.
  - Schema + all migrations are intact (`_sqlx_migrations` untouched).
  - A pre-wipe backup is on the VPS **outside the repo** at
    `/opt/blendbar-preclear-backup-20260724-142713.sql` (`pg_dump`, ~32K) if any
    of the old fixture data is ever needed back.
  - The old smoke test (`web/smoke.js`) still writes a customer+order per run, so
    don't run it against this instance now that it holds real data.

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

## Customer-facing site + portals (new initiative, started 2026-07-24)

Goal: replace the Squarespace site with our own. New public site at
**`sandbox.theblendbarokc.com`**; later promote/redirect `theblendbarokc.com` to it
and keep sandbox as a staging env.

**Decisions (owner-approved):**
- **Stack:** built in our own stack — a static marketing site served by Caddy (NOT
  WordPress; rejected for security/attack-surface). Shares the same API for the
  future portal.
- **Employee MFA:** support **both** TOTP (build first — no email dependency) and
  email codes (added when email is ready). MFA required for all employee logins.
- **Customer "repurchase":** staff-fulfilled reorder (customer submits, staff
  complete/charge) — no online payment for now.

**Phase 1 — public site (DONE, validated):** `site/` (index.html, portal.html,
styles.css, assets/). Warm gold-on-cream luxury design matching the brand; the 3
brand images were pulled from the old Squarespace site into `site/assets/` (served
as `.webp`/`.jpeg` with correct types — needed because our `nosniff` header blocks
mistyped content). New Caddy vhost `sandbox.theblendbarokc.com` in `Caddyfile`
(headers factored into a `(security_headers)` snippet imported by both vhosts);
`web/Dockerfile` copies `site/` to `/usr/share/blendbar-site`. "Customer Portal"
button → `portal.html` (a "launching soon" stub); "Employee Login" → the operator
app at `app.theblendbarokc.com`. Validated: images load, no JS errors, Caddy config
valid, sandbox vhost routes (308→https), app.* unaffected.

**Blockers:** DNS for `sandbox.theblendbarokc.com` not created yet (owner will) —
TLS issues automatically once it resolves; until then Caddy retries ACME (harmless,
app.* fine). Email not ready → customer magic-link + email-based MFA get stubbed
until it is.

**Phase 2 — employee auth (RBAC + MFA). In progress, built in increments:**
- **2a — auth backend + bootstrap (DONE, curl-validated).** Migration 0007
  (`employees`, `employee_sessions`). `employee_auth.rs`: argon2id passwords, TOTP
  (RFC 6238, totp-rs, server QR), 32-byte session tokens hashed in DB, httpOnly +
  Secure + SameSite=Lax cookie `bb_session`, timing-safe login (dummy-hash on
  missing email), and `require_employee`/`require_admin` middleware +
  `AuthedEmployee` extension (defined, wired in 2b). Roles `worker`/`admin`
  (`EmployeeRole`). Auth-flow handlers in `routes/session.rs` on the OPEN router:
  `POST /api/auth/login` (→ pending session, `enroll_required`|`mfa_required`),
  `/api/auth/mfa/enroll` (secret+QR), `/api/auth/mfa/verify` (upgrades the pending
  session to full), `/api/auth/logout`, `GET /api/auth/me`. `mfa_pending` on the
  session gates everything until MFA is done. Bootstrap: CLI
  `blendbar-api create-admin <email>` prints a one-time temp password. **The
  existing device-token auth and all operator routes are untouched** — 2a is purely
  additive; nothing is broken yet.
- **2b — frontend + cutover (DONE, validated). The device-token model is
  retired.** Operator routes now use the `employee_auth::require_employee`
  middleware (replacing `require_operator_token`); admin-only routes take an
  `AdminEmployee` extractor (ingredients/scents create+update, sync status/retry,
  webhooks/recent, admin/backup → 403 for workers). Frontend: API client switched
  to cookie sessions (`credentials: same-origin`, no bearer/localStorage);
  `lib/auth.js` holds `currentUser`/`isAdmin`; `LoginView.vue` (login → enroll-with-
  QR → verify, and login → verify) replaced `PairDevice.vue` (deleted); router
  guard resolves the session once via `/api/auth/me` and gates by auth + `admin`
  meta; `App.vue` shows role-based nav (Admin hidden for workers) + email + Log out.
  **Gotcha for future:** axum 0.7 (axum-core 0.4.x) still needs `#[async_trait]` on
  `FromRequestParts` impls — native `async fn` fails with E0195. **Also: never pipe
  `docker compose build` through `tail` — it hides cargo failures (exit code
  becomes tail's 0); use `set -o pipefail` or no pipe.** Validated: curl RBAC
  matrix (worker 403s on all admin routes, admin 200s, no-session 401) + a browser
  run (unauth→login, admin enroll→verify→full nav→/admin, logout, worker
  login→MFA→no Admin nav→/admin redirects to intake).
- **2c — user management + password change (DONE, validated).** `routes/employees.rs`
  (all admin-only): `GET/POST /api/employees` (create returns a one-time
  `temp_password`), `PATCH /api/employees/:id` (role/active — in a tx that rolls
  back if it would leave zero active admins: the **last-admin guard**),
  `POST …/reset-password` (new temp + kills that employee's sessions),
  `POST …/reset-mfa` (clears TOTP → re-enroll on next login; lost-device recovery).
  `POST /api/auth/change-password` in `session.rs` (self-service: needs current
  password, ≥8 chars, keeps the current session, kills others). Frontend:
  `components/TeamManager.vue` in the admin page (roster, create, role select,
  reset pw/MFA, deactivate, temp-password banner); `views/AccountView.vue` at
  `/account` (change password), linked from the header email. Validated via curl
  (create/list/dupe-409/worker-403/role/active/resets/change-password) + a browser
  run (create employee → temp-password banner; account form). The last-admin guard
  is code-reviewed only — it can't fire in a live DB that always has ≥1 admin
  (`rtaylor@theblendbarokc.com`).
- **Phase 2 is complete.** Not yet done (deferred, noted): login
  rate-limiting/lockout (comes with Cloudflare), email-code MFA (second method,
  when email is ready), forced temp-password change on first login (self-service
  `/account` change covers the need for now).

**Phase 3 — customer portal (in progress).**
- **3a — backend (DONE, curl-validated).** Passwordless magic-link login for
  customers, matched to `customers` rows by email. Migration 0008
  (`customer_login_tokens` single-use+expiring, `customer_sessions`).
  `customer_auth.rs` (own cookie `bb_customer`, 30-day session, reuses
  `employee_auth` token/cookie helpers). `routes/customer_portal.rs` on the OPEN
  router: `POST /api/customer/login` (generic response always — no email
  enumeration; **email is STUBBED: the link is logged, not sent** — replace with a
  real mailer when email is ready, and never return the token to the caller),
  `POST /api/customer/verify` (consumes token → session cookie), `GET
  /api/customer/me`, `GET /api/customer/history` (their mixes-with-items + the
  set-perfume scents they've ordered), `POST /api/customer/reorder` (ownership-
  checked; creates a `lead` order for staff to fulfil + enqueues sync),
  `POST /api/customer/logout`. Magic link points at
  `${CUSTOMER_SITE_URL:-https://sandbox.theblendbarokc.com}/portal/verify?token=`.
  Validated: link generation/anti-enumeration, single-use token, session, history,
  reorder (incl. 404 on someone else's mix, 400 on a never-ordered scent).
- **3b — portal frontend (DONE, validated).** `site/portal/` (index.html + app.js
  + verify.html + verify.js), served on the sandbox site; replaced the old
  `portal.html` stub. Flow: `/portal` → email entry (→ `/api/customer/login`) →
  "check your email"; the magic link → `/portal/verify?token=` → verify.js consumes
  it → `/portal` dashboard (`/me` + `/history`) listing custom blends + signature
  scents (by **name only** — no proprietary formula shown to customers), each with
  a bottle-size select and one-tap **Reorder** (→ a staff-fulfilled `lead` order).
  Vanilla JS under the strict CSP (external scripts only). **Gotcha:** portal pages
  must use **absolute** asset/script paths (`/portal/app.js`, `/styles.css`) — with
  the `/portal` no-trailing-slash URL, relative `app.js` resolved to `/app.js` and
  fell through to the marketing HTML (wrong MIME → refused). Caddy sandbox
  `try_files` gained `{path}/index.html` to serve the portal directory. Validated
  with a full browser run (login form → check-email → magic-link verify →
  dashboard → reorder creates a lead order).
- **Only blocker left:** real email send (owner setting up). Until then the login
  link is logged, not emailed — a real customer can't complete login end-to-end,
  but everything else is built and works (validated by pulling the link from logs).
  When email lands: swap the `tracing::info!` stub in `customer_portal::request_link`
  for a real send. **Phase 3 is otherwise complete.**
- **⚠️ REMINDER — WHEN WIRING EMAIL, ALSO UNDO THE DEV PORTAL BYPASS** (added
  2026-07-24 at the owner's request so he could preview the customer page without
  email). To remove:
  1. Delete the `PORTAL_BYPASS_EMAIL` branch at the top of
     `customer_portal::request_link` (marked with a ⚠️ comment).
  2. Remove `PORTAL_BYPASS_EMAIL` from `.env` and `docker-compose.yml` (leave the
     blank line in `.env.example` or drop it), rebuild/redeploy the api.
  3. Delete the seeded **example data**: customer `rtaylor@theblendbarokc.com`
     (name "Ryan Taylor (example)") + its example mix/orders, and the
     `Golden Hour (example)` scent. (While the bypass is on, ANYONE who types
     `rtaylor@theblendbarokc.com` on the portal is logged into that example
     customer — it's demo data, but that's why this must come out.)

**Phase 4 — promote to production:** cut `theblendbarokc.com` over to our site,
redirect from Squarespace, keep sandbox for testing.

## Milestone 8: Shareable scents + Square checkout (in progress, 2026-07-24)

Owner wants shareable scent links + QR so a customer can send a scent to a friend
who can buy it online. **Owner-approved decisions:**
- **Payments via Square, PCI-minimal:** use **Square Hosted Checkout** (create a
  payment link server-side → redirect the buyer to Square's hosted page → payment
  webhook back). Card data never touches our server or page ⇒ **PCI SAQ-A**. (Not
  Xero — it invoices/integrates Stripe, doesn't process cards.) Web Payments SDK is
  the in-page alternative if a seamless field is wanted later (SAQ-A-EP).
- **Per-size prices per scent** (3.4oz / 1.7oz / roller), set in Admin. ✔ done.
- **Public visibility:** share/public pages show a scent's **ingredient NAMES only**
  (the notes) + prices — **never the ml amounts** (formula stays employee-only).
- **Shared custom blends:** view-and-buy only, **not editable**, **crafted by staff
  at an event** (a purchased blend is a fulfilment task, not shipped).

**Step 1 — per-size pricing (DONE, validated).** Migration 0009 adds
`price_oz3_4/oz1_7/roller` (numeric, nullable) to `scents`; `Scent` model + the
`Scent` serialization carry them. `PATCH /api/scents/:id` takes `prices:{oz3_4,
oz1_7,roller}` (present ⇒ set all three, a null clears one; omitted ⇒ unchanged;
negatives ⇒ 400). Admin `ScentManager` gained 3 price inputs in the scent editor
and a price summary. Validated (curl + browser: set/clear/negative/omit; UI saves
72/40/22).

**Step 2 — share links + QR + public pages (DONE, validated).** `routes/public.rs`
(open router): `GET /api/public/scent/:id` → `{name, notes:[ingredient names],
price_*}` for **active** scents only (404 otherwise) — **names only, never
amounts/items**; `GET /api/public/scent/:id/qr` → SVG QR of the share URL (`qrcode`
crate). Sandbox site: `site/share.html` + `share.js` served for `/s/*` (Caddy
`handle /s/* { rewrite * /share.html }`; share.js reads the id from the path) —
public scent page with monogram, notes chips, per-size price radios, and a Buy CTA
(placeholder → "checkout launching soon" until Step 3). Portal scent cards gained a
**Share** button → panel with the `/s/<id>` link (copy) + the QR `<img>`. Validated
(curl: names-only/no-amount-leak/404/QR-svg + `/s/<id>` serves the page; browser:
public page renders name/notes/prices, QR loads, Buy shows placeholder).

**Step 2.5 — global custom-blend pricing (DONE).** Migration 0010 adds a singleton
`settings` table with `custom_price_oz3_4/oz1_7/roller` (bespoke blends are priced
uniformly by size, not per blend). `GET/PATCH /api/settings` (admin, negatives→400)
+ a "Custom blend pricing" card in Admin. Validated. Custom-blend *sharing* (public
page/QR for a blend) is still deferred — the pricing it needed now exists.

**Step 3 — Square Hosted Checkout (BUILT 2026-07-27, unverified against real Square).**
Superseded by Milestone 9 below: Squarespace was removed entirely and Square is now
the billing system. Still needs the owner's Square credentials to be exercised for
real.

## Milestone 9: Squarespace → Square billing migration (2026-07-27)

Owner asked to drop Squarespace, send the "cart" to Square so all transactions happen
there, and be able to reconcile site sales against Square transactions. Done, with
one honest caveat: **no Square credentials exist on this box, so the live HTTP path
has never made a real request.** Everything below is validated against the mock
backend and covered by 24 unit tests (`cargo test`).

**Removed.** `api/src/squarespace/`, `routes/webhooks.rs`, `models/webhook.rs`, the
`webhook_events` table, `orders.squarespace_order_id`, and both `SQUARESPACE_*` env
vars. `customers.squarespace_contact_id` was **renamed** to `square_customer_id` (it
only ever held nulls). Nothing of value was lost — the old order ids were all mock
values like `mock_order_…`; no real Squarespace record ever existed.

**New schema (migration 0011).** `carts`, `cart_items`, `square_webhook_events`,
`reconciliation_runs`. Cart money is `bigint` **cents**, matching Square, which never
accepts decimals; `orders.amount numeric(10,2)` is untouched and the cart converts
from it at build time via `square::money::to_cents` (banker's rounding, unit-tested —
the one place a silent 100x error could hide without ever throwing).

**The model.** A *cart* is one checkout = one Square order = one payment. Its lines
either point at an `orders` row (a blend) or stand alone (event deposit, rush fee,
hotel line — the booking-terms items). `cart_items.order_id` is uniquely indexed, so
a blend can never be billed on two carts; cancelling a cart nulls those links to
release them (`billing::cancel_cart`), which is also how the 24h abandoned-checkout
sweep works.

**The flow.** intake (order, `lead`, no money) → `POST /api/carts` → `POST
/api/carts/:id/checkout` (Square order + hosted payment link) → operator shows
`/api/carts/:id/checkout.svg` as a **QR code** → customer pays on their own phone on
Square's page → `payment.updated` webhook → cart `paid`, its orders `paid`.

**Webhook receiver.** `POST /api/webhooks/square`, public but signature-verified —
that verification is the whole security boundary, since the endpoint marks carts
paid. Square signs `notification_url + raw_body`, HMAC-SHA256, **base64** (the old
Squarespace one was hex, and its header/encoding were guesses; Square's is
documented). `SQUARE_WEBHOOK_URL` is configured, **not** derived from `Host`, because
`Host` is attacker-controlled and deriving it would let a caller choose the string
their forged signature was computed over. Unset key or URL ⇒ receiver returns 503.
Nine tests cover this, including tampered body, wrong key, wrong URL, and malformed
base64.

**Missed-webhook backstop.** `POST /api/carts/:id/refresh` ("Check Square" on the
checkout screen) pulls payment state directly. Without it a paid cart would sit at
`pending_payment` forever while the customer waves a Square receipt. `billing::apply_payment`
is shared by both paths so push and pull can't drift, and is idempotent under
retry/race (row lock + status guards).

**Reconciliation.** `GET /api/square/reconcile?from=&to=&save=` buckets every sale on
both sides into matched / amount_mismatch / missing_in_square / missing_locally /
awaiting_payment, joined on `carts.square_order_id`. Admin → Reconciliation renders
it; "Run & save" snapshots to `reconciliation_runs`. Window edges are handled so a
23:59 sale doesn't report as missing on one side and orphaned on the other — see the
module doc in `routes/reconciliation.rs`.

**Contact sync retargeted.** The `sync_outbox` worker now upserts into **Square
Customers** (marketing consent → Square's inverse `preferences.email_unsubscribed`).
Orders no longer flow through the outbox: checkout is synchronous because an operator
is standing there waiting for a link.

**Frontend.** New `/checkout` view (in nav, plus "Take payment" from Lookup); the
Admin "Squarespace integration" card replaced by "Square billing" + "Reconciliation" +
"Recent Square events". Both the checkout screen and the admin panel show a **red
mock-mode warning** whenever credentials are absent, so nobody mistakes a fake link
for a real one.

**What is NOT done / next up:**
- **Nothing has touched real Square.** Follow "Going live on Square" in README.md —
  sandbox first, test card, confirm reconciliation balances, only then production.
  Expect the HTTP client to need small corrections; that is what the sandbox pass is
  for.
- Portal **Reorder** could route through the same public checkout now that it exists.

## Milestone 10: public buy button on share pages (2026-07-27)

`POST /api/public/checkout` — the one endpoint where an anonymous caller can set
money in motion. Someone sent a `/s/<scent-id>` link picks a size, enters an email,
and is handed off to Square's hosted page.

**Why it is safe to expose:**
- **Price comes from the database**, keyed on the requested size — never from the
  request body. A caller picks *what* to buy, never what to pay. (E2E asserts that
  `amount`/`unit_amount`/`total_cents` in the body are ignored.)
- **An existing customer row is never modified.** The upsert is
  `on conflict (email) do update set email = customers.email` — a deliberate no-op
  so RETURNING works without letting a stranger rewrite a real customer's name or
  marketing consent by buying with their email. New rows get
  `marketing_consent = false`; a purchase is not consent.
- **Rate limited** 10/IP/5min (`api/src/ratelimit.rs`), keyed on the **rightmost**
  `X-Forwarded-For` — the entry Caddy wrote. The leftmost is client-forgeable, so
  the usual "take the first" would let anyone mint a new identity per request.
  Verified live through Caddy: 10 through, then 429.
- **503 when Square is not live**, so a customer is never sent to a mock link. The
  check sits *after* input validation and *before* the first write: a bad request
  hears that it is bad, nothing is persisted for a checkout that cannot proceed,
  and the validation stays testable while the app is still on the mock.
- Square's error text is never returned to an anonymous caller (it can carry
  account/config detail); it is logged instead.

**Fulfilment.** These orders carry `external_ref = 'public_share'` and a cart note
so staff know the blend must be *made up*, not handed over at the bar. If the
checkout is abandoned, `billing::cancel_cart` deletes the speculative order rather
than leaving a phantom on that email's account — bar orders are left alone, since
those blends physically exist.

**Frontend.** `site/share.js` gained an email/name form and posts to the endpoint,
then redirects to Square. On 503 it swaps in the "message us to order" fallback
instead of a broken button. New `site/thanks.html` is the post-payment landing page
(noindex), and it says plainly that blends are hand-made so nobody waits on a
tracking number.

**Verified:** 31 unit tests + 44 E2E checks. The success path (real link issued)
still cannot be exercised until Square credentials exist — in mock mode the
endpoint correctly refuses, which is what the E2E asserts.

## Milestone 11: fourth bottle size — Spray (2026-07-27)

Owner: *"add a 4th size and label it as Spray — it's a 10ml with a spray top
instead of a roller."*

**The load-bearing detail:** the base formula is 3.4oz ≈ 100 ml, and the roller is
a tenth of it = 10 ml. The spray is *also* 10 ml. So spray and roller share a pour
factor (0.1) and differ only in the closure (atomiser vs rollerball) and in price.
No new scaling maths anywhere — `BOTTLE_SIZES` in `web/src/lib/bottle.js` just
gains a fourth row with the same factor, and the pour chart picks it up
automatically since it iterates that list.

Migration 0012: widens the `orders_size_check` constraint to include `'spray'`,
and adds `scents.price_spray` + `settings.custom_price_spray` (nullable, like the
others — null means "not sold in this size" and the share page hides the option).

Rust `BottleSize::Spray` labels as **"Spray (10 ml)"**. Two new tests pin the
things that would break quietly: that every size has a *distinct* label (an
operator reading "Roller" on a spray order builds the wrong thing) and that the
wire strings are stable (they are persisted in `orders.size` and sent by the
public share page).

Touched for the new size: scent price editor, custom-blend pricing card, the
public share page's size options, the customer portal's size labels, and the
public checkout's price lookup.

**Owner action needed:** spray prices are **null everywhere**. Until an admin sets
them (Admin → per-scent Prices, and Custom blend pricing), the spray option will
not appear on share pages and a spray order cannot be carted. That is deliberate —
the app will not invent a price.

## Milestone 12: chat notifications + README auth fix (2026-07-27)

**README fix.** The "Operator UI" section still documented device-token pairing
(`issue-device-token`, paste-the-token screen, `localStorage`), which Phase 2b
replaced with employee accounts + TOTP. Anyone onboarding would have tried an auth
path that no longer exists. Rewritten to the real flow, with an explicit
*superseded* note: the CLI subcommand still exists but nothing checks the tokens
it issues, and `auth::require_operator_token` is dead code kept only until the
subcommand goes.

**Notifications.** `notify::` posts to Discord / Slack / Teams on two events:

- `sale.online` — a cart with `created_by is null` is paid. That column is the
  "no employee involved" test: the public share-page checkout leaves it null,
  operator checkout sets it.
- `event.booked` — a cart containing an `event_deposit` line is paid. Fires
  regardless of who built the cart, because the owner's own published terms say
  the event is not booked until the deposit is paid.

**Sales at the bar deliberately do not notify** — the staff member is standing
there, and a noisy channel is an ignored channel. This was the explicit ask
("triggered by a customer but not by an employee").

**There is still no event *booking* flow.** The site's "Book an Event" section
links to Instagram DMs; nothing in the app captures an enquiry. `event.booked`
therefore fires at deposit payment, which is the first moment the app knows about
an event at all. If notification-at-enquiry is wanted, a booking form has to exist
first — that is separate work.

**Schema (0013).** `cart_items.kind` (`blend`/`event_deposit`/`fee`/`other`) —
explicit, because matching a deposit on its free-text label breaks the first time
someone retypes it; the migration back-fills historical rows by label as a
best-effort. Plus `notification_targets` and `notification_deliveries` (queue +
audit + dedup in one table, unique on `(target, cart, event)`).

**Security.** The webhook URL is a bearer credential, so it is write-only from the
browser — the API returns a redacted hint, never the URL. URLs are validated
against a **per-platform host allowlist** rather than a private-IP blocklist:
this is an admin-supplied URL the server fetches, i.e. textbook SSRF, and a
blocklist has to anticipate every spelling of an internal address. Rejects
non-https, `user@host` disguises, lookalikes (`discord.com.evil.example`),
loopback, and cloud metadata. Customer email is opt-in, off by default.

**Verified 2026-07-27.** 49 unit tests (16 new) + 60 E2E checks. Behaviour proven
live against the running stack: a public cart queued `sale.online` only; a staff
cart with a deposit queued `event.booked` only and *not* `sale.online`;
re-settling both produced no duplicates. Delivery reached **api.discord.com and
came back 404 "Unknown Webhook"** — a real response from Discord, so the outbound
transport, TLS and payload are proven; only a valid webhook URL is missing.
Unlike Square, this integration *has* touched its real third party.

## Milestone 13: email (2026-07-27)

The app had never sent an email. Portal sign-in links were logged rather than
mailed, which is the only reason `PORTAL_BYPASS_EMAIL` existed, and the
online-order thank-you page promised "we'll email you when it's ready" with
nothing behind it.

**Two transports behind a `Mailer` trait**, picked in order:

1. **Gmail API + service account with domain-wide delegation** (preferred).
   Signs an RS256 JWT asserting "I am this service account, acting as
   <mailbox>, scope gmail.send", exchanges it for a 1-hour access token
   (cached, refreshed a minute early), and POSTs a base64url RFC-5322 message
   to `users/me/messages/send`.
2. **Workspace SMTP relay** (`smtp-relay.gmail.com:587`, TLS *required*, not
   opportunistic). IP-allowlisted needs no credentials at all.
3. Mock that logs.

**Why a service account and not 3-legged OAuth** — the owner asked about this
after reading a Targetprocess guide, which describes a *different* shape: a
plugin reading a mailbox, authenticating as a user via consent. For unattended
outbound mail a user refresh token can be revoked or lapse, and when it does the
sign-in links stop and customers are locked out of the portal with no human
awake to re-consent. A service account has no such moment.

**Correction to an earlier claim:** the first version of this work said the
Gmail API route would need a CASA assessment. That was wrong for this
deployment. `gmail.send` is a *sensitive* scope, not *restricted*, and Google
requires no verification or CASA for an app used only inside its own Workspace.
The README overstated the barrier and has been fixed.

**Design decisions:**
- **No message body is ever persisted.** `email_deliveries` is metadata only: a
  sign-in mail carries a token that grants a customer session, already stored
  hashed in `customer_login_tokens`, and writing the rendered body would put a
  working credential at rest in a second place. Queued mail re-renders from ids.
- **Sign-in links send inline; order-ready queues.** Someone is watching a
  "check your email" screen and the token expires in minutes.
- `POST /api/customer/login` answers identically for known and unknown
  addresses, including on send failure, so it cannot enumerate customers.
- Plain text *and* HTML on every message; a link that only renders in HTML fails
  for anyone whose client blocks it.
- One "ready" email per order (partial unique index).
- The service-account key is mounted read-only from a host path rather than
  passed as an env var, keeping it out of `docker inspect`.

**Admin → Email tab:** sender identity, order-ready toggle, test send, delivery
log. Transport host and credentials are never settable or readable there.

**Verified:** 70 unit tests (16 new, incl. the Gmail message encoding round-trip
and base64url correctness) and 13 live checks. **Not yet exercised against real
Google credentials** — no service account exists yet.

**Still open:** remove `PORTAL_BYPASS_EMAIL` once a transport is live.

## Not started

## Security posture (reviewed 2026-07-24)

The app is public on 80/443 behind Caddy. A security review was done; findings and
status:

**Already good:** only 22/80/443 listen on public interfaces — Postgres (5432) and
the API (8080) are Docker-internal only, never internet-exposed. Real TLS. All SQL
is parameterized (`sqlx` binds). Device tokens are 256-bit random, stored SHA-256
hashed. Errors don't leak internals. No Squarespace secrets reach the browser.

**Done — "fix now" tier:**
- Rotated the default Postgres password; strong random webhook secret; removed the
  dead `OPERATOR_AUTH_SECRET`; deleted the world-readable plaintext PII backup.

**Done — hardening tier:**
- **Security headers** in `Caddyfile` (site-level `header` block, applies to app +
  API): HSTS (1yr, includeSubDomains), `X-Content-Type-Options: nosniff`,
  `X-Frame-Options: DENY`, `Referrer-Policy`, `Permissions-Policy`, a CSP
  (`default-src 'self'`; `style-src` allows `'unsafe-inline'` for Vue `:style`
  attrs), and `-Server`. Verified the CSP does **not** break the SPA (0 console
  violations on intake+admin).
- **Backup download**: `GET /api/admin/backup` (`api/src/routes/admin.rs`, authed)
  runs `pg_dump --no-owner --no-privileges` and streams the `.sql` as an
  attachment; the api image now ships `postgresql-client-16` (PGDG, matches the
  PG16 server) — see `api/Dockerfile`, tokio `process` feature. Admin UI has a
  "Download database backup" button (`AdminView` + `downloadBackup()` in api.js,
  blob download). **Restore-tested**: the dump reloaded cleanly into a scratch DB
  with matching row counts. ⚠️ It's a full PII export gated only by device auth —
  reinforces the admin-RBAC need below.

- **Scheduled backups (2026-08-24)**: encrypted, off-box, on a cron schedule.
  Migration `0018_backup_schedules.sql` (`backup_destinations` + `backup_runs`);
  code in `api/src/backup/` (`mod.rs` pipeline + worker, `schedule.rs` cron,
  `destination.rs` trait + email backend, `drive.rs` Google Drive); routes in
  `api/src/routes/backup_admin.rs`; UI in `web/src/components/BackupScheduler.vue`
  (Admin → Data). Worker spawned in `main.rs` beside `sync::run_worker`, polling
  every 60s.

  - **Pipeline**: `pg_dump | gzip | age(passphrase)`. Compression *before*
    encryption — ciphertext does not compress. 66KB dump → 14KB artefact in
    practice. The manual download endpoint now shares the same `pg_dump` so the
    two cannot drift.
  - **Format is stock `age`** (`age-encryption.org/v1`), deliberately: restore is
    `age -d f.sql.gz.age | gunzip | psql "$DATABASE_URL"` and does **not** need
    this application. That is the whole point — the scenario you need a backup in
    is the one where the app is gone.
  - **Passphrase lives on the secrets volume** (`/var/lib/blendbar/secrets/backup-passphrase`,
    0600), or `BACKUP_PASSPHRASE` in the env, **never in Postgres** — a passphrase
    in a table would be dumped into every backup it encrypts, so every file would
    carry its own key. Write-only from the browser. Verified absent from a real
    `pg_dump`.
  - **Schedules** are standard 5-field cron + an IANA timezone, resolved per run
    so "daily at 3:30am" survives DST. Six-field expressions are *rejected*: the
    crate accepts them and `0 2 * * *` would silently mean "every second of
    02:00". UI presets cover hourly / every N hours / daily / weekly / custom.
    Known gap, tested and documented: a job scheduled in the hour DST skips
    (2am on the spring-forward day) does not run that day — which is why the
    daily preset defaults to 3:30am.
  - **Retention** deletes only files this scheduler uploaded and still has a
    Drive file id for, so a shared folder cannot lose someone else's document.
    Email has no delete, so retention does not apply there (said so in the UI).
  - **Drive** uses `drive.file` scope via domain-wide delegation, impersonating a
    real Workspace user — a service account has no Drive quota of its own and
    uploading as one fails with `storageQuotaExceeded`. It needs a **second**
    delegation entry alongside `gmail.send`; that is the step people miss, and
    the 403 message says so.
  - **Verified 2026-08-24**: 106 unit tests (26 new). End-to-end on this box —
    migration applied, worker started, every validation rejection confirmed over
    HTTP (4-field cron, 6-field cron, no recipient, sharepoint, 30 February,
    retention 0), next-run times correct across three timezones, failures
    recorded in `backup_runs` rather than swallowed, unauth → 401. **Restore
    proven**: a real artefact built by the live pipeline was decrypted with the
    stock `age` CLI 1.2.1, gunzipped to a byte-exact 66,732-byte dump, and
    reloaded into a scratch database — 31 tables, 104 rows, *identical* counts.
    All test artefacts (scratch DB, plaintext dump, test destinations, test
    passphrase, temp session) were destroyed afterwards.
  - ⚠️ **Not yet live**: no passphrase is set and no destination can deliver
    (`GOOGLE_SA_KEY_FILE`, `GOOGLE_IMPERSONATE`, `SMTP_HOST` are all empty). The
    email backend deliberately **refuses** rather than succeeding against the mock
    mailer — a green history with nothing sent anywhere is the worst outcome for a
    backup system.
  - **SharePoint** is accepted by the schema's check constraint and refused by the
    code with a clear message; there is no Microsoft tenant behind this
    deployment. Adding it later is a new file behind `backup::destination::Backend`,
    not a migration.

**Chosen direction:** put **Cloudflare** in front (WAF + rate limiting + DDoS +
hide origin IP) — not yet configured; it's the planned network layer. (VPN-only was
the alternative, not taken because customer access is likely later — see below.)

**Still open / recommended, not yet done:**
- Rate limiting (none today) — comes with Cloudflare, or `caddy-ratelimit`+fail2ban.
- SSH hardening (key-only, fail2ban, restrict :22), host `ufw` default-deny.
- ~~**Automated, scheduled, encrypted, off-box** DB backups + retention.~~ **BUILT
  2026-08-24** — see "Scheduled backups" below. ⚠️ Restore-verified, but **nothing
  is scheduled yet**: it needs a passphrase set in Admin → Data and a destination
  that can actually deliver (neither Google nor SMTP is configured on this box).
  Until both are done the only real backup is still the manual pull.
- Least-privilege DB role for the app (currently connects as the `blendbar` owner).
- Auth model: tokens never expire/rotate, and **any paired device can reach
  `/admin`** (no roles). Add an admin role + token revocation before scaling
  devices — and definitely before the customer-facing plan below.

**Future intent:** the owner may open this to **customers for online scent
reordering**. That's a major security shift — it means real customer login/auth,
a public untrusted surface, per-customer authorization (a customer may only see
their own data), and almost certainly the RBAC/rate-limiting/headers work above as
prerequisites. Treat any customer-facing work as needing its own security pass.

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
- The `mock_*` external ids concern is now moot — the DB was wiped clean on
  2026-07-24 (see "What's actually running" above), so there are no stale synced
  rows left. The catalogs are empty and ready for real data.

## How to pick this back up

1. `cd /opt/app && git status` — see whether anything's changed since this was
   written; commit first if not already done.
2. `docker compose ps` — confirm the stack is still healthy.
3. Skim this file and `README.md`. All seven milestones are done; the remaining
   work is going live (see "Not started" above): get the Squarespace API key +
   real webhook secret, swap them in, and verify the two untested live HTTP paths
   (`HttpSquarespace` push/`get_order` and the webhook signature wire format).

Note: the instance was wiped clean on 2026-07-24 for real-data entry — empty
ingredient/scent catalogs, no customers/orders. Add the real ingredient catalog
(with Base/Top Note/Heart Note types) and scents via `/admin` first. Pre-wipe
backup at `/opt/blendbar-preclear-backup-20260724-142713.sql` if needed.
