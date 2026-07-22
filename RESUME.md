# Blend Bar — Resume Notes

Last updated: 2026-07-22, after Milestone 3.

Read this first in a new session, then README.md for deploy mechanics.

## ⚠️ Nothing is committed to git yet

`git init` was run in `/opt/app` but there has never been a commit. All work exists
only as uncommitted files on this VPS's disk. If this box is lost, so is everything.
First thing to do in a new session: review `git status`, commit, and push to a real
remote if one doesn't exist yet.

## Where this lives

This repo lives directly on the target VPS at `/opt/app` (hostname `app`, Ubuntu
26.04). Docker, the Compose stack, and all validation in Milestones 1–3 have been
run for real on this box, not in a separate sandbox.

## Status: Milestones 1–3 done and validated live on this VPS

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

  Decide before real launch: wipe this back to empty, or keep it as fixtures for
  building the Milestone 4 UI against. Nobody has answered this yet.

## Not started

- **Milestone 4** — Vue intake + mix-builder UI + reorder view. `web/` still only
  has the Milestone 1 placeholder page that fetches `/api/health`. Nothing else
  has been built on the frontend.
- **Milestone 5** — Squarespace sync layer (Contacts + Orders APIs) behind a
  mockable trait. Not started; `SQUARESPACE_API_KEY` in `.env` is blank.
- **Milestone 6** — Webhook receiver + signature verification + reconciliation.
  Not started.
- **Milestone 7** — `GET /api/customers/:id/reorder`. This route does not exist
  yet at all.

## Open items nobody has answered yet

- Squarespace API key not yet obtained or set.
- Squarespace webhook signing-secret handling — will come up when Milestone 6
  starts; no decision made.
- Whether to wipe or keep the fixture data described above.

## How to pick this back up

1. `cd /opt/app && git status` — see whether anything's changed since this was
   written; commit first if not already done.
2. `docker compose ps` — confirm the stack is still healthy.
3. Skim this file and `README.md`, then ask for the Milestone 4 plan to continue.
