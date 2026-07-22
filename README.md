# Blend Bar — Perfume Stand Intake App

Self-hosted stand-intake app: Rust/Axum API, Vue 3 frontend, Postgres, all behind
Caddy for automatic HTTPS. Squarespace is a downstream sink (Contacts + Orders APIs);
this app's own Postgres database is the source of truth for customers, preferences,
mixes, and orders.

## Services

- `db` — Postgres 16, persisted in the `pgdata` volume.
- `api` — Rust (Axum) backend, internal port 8080, not published directly.
- `caddy` — serves the built Vue static assets and reverse-proxies `/api/*` to `api`.
  Owns ports 80/443 and handles automatic TLS via Let's Encrypt for `DOMAIN`.

## Prerequisites on the VPS

- Docker Engine + the Docker Compose plugin (`docker compose version`).
- Ports 80 and 443 open in the firewall.
- A DNS A/AAAA record for the app's domain pointed at this VPS's public IP.
  **Caddy cannot issue a TLS certificate until this DNS record resolves** — until
  then it will retry the ACME challenge in the background and log failures; this is
  expected and not a build problem.

## Deploy

```bash
git clone <this repo> blendbar
cd blendbar
cp .env.example .env
# edit .env: set real POSTGRES_PASSWORD, OPERATOR_AUTH_SECRET, SQUARESPACE_API_KEY,
# and DOMAIN if it differs from app.theblendbarokc.com

docker compose up --build -d
docker compose ps        # all three services should report healthy
```

Verify end to end once DNS has propagated:

```bash
curl https://app.theblendbarokc.com/api/health
# {"status":"ok"}
```

Before DNS is live, you can still verify the containers build and talk to each other
over plain HTTP from inside the VPS:

```bash
docker compose exec caddy wget -qO- http://api:8080/api/health
```

## Local development

- `api`: `cd api && cargo run` (requires a local Postgres reachable at `DATABASE_URL`
  once the schema lands in a later milestone).
- `web`: `cd web && npm install && npm run dev` — Vite dev server. `/api/*` is proxied
  to `http://localhost:8080`; override with `API_ORIGIN=... npm run dev`.

## Operator UI

The UI is operator-driven: staff run it on the stand tablet. It needs a device token
before it will do anything.

```bash
docker compose exec api blendbar-api issue-device-token "Stand iPad"
```

Open the site, paste the token on the pairing screen, and it is kept in
`localStorage` until "Unpair" is used or the API rejects it (a 401 forces re-pairing).

- **Intake** — customer details, marketing consent, scent preferences, and the order
  (type, bottle size, status, amount). Custom mixes use the mix builder, capped at 8
  ingredients. Amounts are entered as the 3.4oz base formula; the 1.7oz and roller
  amounts are shown derived, never stored.
- **Lookup** — search customers by email, see their saved mixes and orders, and
  "Reorder" a mix to open intake with that customer and mix prefilled.

Each submission carries a generated `Idempotency-Key` that is held steady across
retries, so a double-tap or a flaky connection cannot create two orders.

### Smoke test

`web/smoke.js` drives the whole flow in a real headless browser against a running
stack. It writes one customer and one order per run — see the header comment in that
file for the exact command.

## Status

Milestones 1–4 are done and validated live on the VPS: scaffold + TLS, schema,
operator auth / CRUD / intake, and the operator UI described above. Squarespace sync
(5), the webhook receiver and reconciliation (6), and `GET /api/customers/:id/reorder`
(7) are not started — see `RESUME.md` for the current state and open questions.
