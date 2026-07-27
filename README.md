# Blend Bar — Perfume Stand Intake App

Self-hosted stand-intake app: Rust/Axum API, Vue 3 frontend, Postgres, all behind
Caddy for automatic HTTPS. **Square is the billing system** — carts are handed to
Square's hosted checkout, so no card ever touches this app — while this app's own
Postgres remains the source of truth for customers, preferences, mixes, and orders.
The two are reconciled on demand; see [Square billing](#square-billing).

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
- **Admin** — manage the ingredient and scent catalogs (add, activate/deactivate).
  Every ingredient is classified by perfumery **type** — Base, Top Note, or Heart
  Note — set on add and editable per row; the mix/scent builders group their
  ingredient picker by type. Each scent carries an editable **ingredient formula**
  (a house recipe), built with the same mix builder as custom mixes; set-perfume
  orders show that breakdown in intake and in a customer's order history. Also
  shows **Square billing** status (live vs. mock, webhook receiver, cart counts,
  money awaiting payment), the **reconciliation** report over a date range, and
  recent inbound Square events. Server-side secrets (`SQUARE_ACCESS_TOKEN`,
  `SQUARE_WEBHOOK_SIGNATURE_KEY`, …) are set in `.env`, not from the UI.
- **Checkout** — build a cart from a customer's unsold orders plus any ad-hoc lines,
  then show the Square payment link as a QR code for the customer to scan. See
  [Square billing](#square-billing).

Each submission carries a generated `Idempotency-Key` that is held steady across
retries, so a double-tap or a flaky connection cannot create two orders.

### Smoke test

`web/smoke.js` drives the whole flow in a real headless browser against a running
stack. It writes one customer and one order per run — see the header comment in that
file for the exact command.

## Square billing

Square is the billing system. **This app never sees a card.** It assembles a cart,
hands it to Square as an order plus a hosted payment link, and the customer pays on
Square's own page — so card data never crosses the tablet, this server, or the shop's
network, and PCI scope stays with Square.

### The flow

```
intake                 checkout                  Square hosted page     webhook
──────                 ────────                  ──────────────────     ───────
order (status=lead) →  cart (one or more     →   customer scans QR  →   payment.updated
no money moved         orders + ad-hoc lines)    and pays               → cart=paid,
                       → Square order +                                   orders=paid
                         payment link
```

1. **Intake** creates orders exactly as before, `status = lead`. Nothing is owed yet.
2. **Checkout** (`/checkout` in the operator app) gathers one or more of that
   customer's unsold orders, plus any ad-hoc lines — an event deposit, a rush fee,
   the multi-day hotel line from the booking terms — into a **cart**.
3. `POST /api/carts/:id/checkout` pushes the cart to Square and gets back a hosted
   payment link. The screen shows it as a **QR code**: the customer scans with their
   own phone and pays there.
4. Square reports the result by webhook; the cart flips to `paid` and its orders with
   it. The screen updates on its own.

A cart is the unit of a checkout: one cart = one Square order = one payment. An order
can be claimed by at most one cart at a time (a unique index enforces it), so the same
blend can never be billed twice.

Cancelling a cart — by hand, or automatically after 24h unpaid — releases its orders
so they can be sold again **and voids the payment link at Square**. Both halves are
required: releasing without voiding would leave a live link that could still be paid
against blends since sold on another cart, and Square would be holding money nothing
here could explain.

Money is stored in **integer cents** on carts, matching Square, which never accepts
decimals. The conversion from the decimal prices operators type lives in
`api/src/square/money.rs` and is unit-tested — that is the one place a rounding bug
would silently charge the wrong amount without ever throwing.

### Buying from a share link (public checkout)

`POST /api/public/checkout` lets someone who was sent a `/s/<scent-id>` link buy
that scent without an account. It is the only endpoint in the app where an
anonymous caller can set money in motion, so it is deliberately narrow:

- **The price is read from the database**, keyed on the requested size — never
  taken from the request. A caller chooses *what* to buy, never what to pay.
- **An existing customer row is never modified.** Buying with someone else's email
  must not let a stranger rewrite their name or flip their marketing consent, so
  an existing record is used exactly as-is. New records are created with
  `marketing_consent = false` — a purchase is not consent.
- **Rate limited** to 10 attempts per IP per 5 minutes, keyed on the *rightmost*
  `X-Forwarded-For` entry (the one Caddy wrote; the leftmost is client-forgeable).
- **Refused with 503 when Square is not live.** A staff member seeing a mock link
  is an inconvenience; sending a paying customer to a dead URL is not, so the
  endpoint refuses rather than hand back a fake checkout. The share page then
  shows a "message us to order" fallback instead of a broken button.
- Square's error text is never returned to an anonymous caller — it can carry
  account and configuration detail.

Orders created this way carry `external_ref = 'public_share'` and a cart note, so
staff can see the blend has to be **made up and shipped/collected**, not handed
over at the bar. If such a checkout is abandoned, expiry deletes the speculative
order rather than leaving a phantom on that customer's account — orders taken at
the bar are left alone, because those blends physically exist.

After payment Square returns the buyer to `/thanks`.

### Reconciliation

`GET /api/square/reconcile?from=&to=` compares what this app recorded selling against
what Square actually collected, joined on `carts.square_order_id`. Every sale on
either side lands in exactly one bucket:

| Bucket | Meaning | What to do |
|---|---|---|
| `matched` | Both sides agree to the cent | nothing |
| `amount_mismatch` | Both have it, totals differ | check for a tip, a discount, or a price edited in the Square dashboard |
| `missing_in_square` | Marked paid here, no Square payment | **investigate** — the bucket that means money may not have been taken |
| `unrecorded_payment` | Square collected, but the cart never left `pending_payment` | a lost webhook — press "Check Square" on that cart, then fix the subscription |
| `missing_locally` | Square collected, no cart here at all | usually a sale rung up directly in the Square POS |
| `awaiting_payment` | Link issued, not yet paid | informational; expires after 24h |

Run it from Admin → Reconciliation. "Run & save" stores a snapshot in
`reconciliation_runs`, so a discrepancy found today is still inspectable next month
even after the underlying rows are fixed up. `GET /api/square/reconcile/history`
lists saved runs.

Window edges are handled deliberately: Square's list is authoritative for the period,
and the local side then pulls in any cart *either* paid in the window *or* named by a
payment Square returned — otherwise a sale at 23:59 would report as missing on one
side and orphaned on the other.

### Inbound webhooks

`POST /api/webhooks/square` is public (Square can't present an employee session) but
**signature-verified**, which is the entire security boundary — it can mark carts
paid. Square signs `notification_url + raw_body` with HMAC-SHA256, base64-encoded, in
`x-square-hmacsha256-signature`.

`SQUARE_WEBHOOK_URL` is configured explicitly rather than reconstructed from request
headers: `Host` is attacker-influenced, and deriving the signed message from it would
let a caller choose the string their forged signature was computed over. It must match
what's in the Square dashboard **byte for byte**, trailing slash included.

**With `SQUARE_WEBHOOK_SIGNATURE_KEY` or `SQUARE_WEBHOOK_URL` unset the receiver is
disabled and returns 503** — it never runs unauthenticated. Until webhooks are wired,
"Check Square" on the checkout screen (`POST /api/carts/:id/refresh`) pulls the payment
state directly; that button is also the recovery path for a webhook lost to a deploy.

- `GET /api/square/status` — backend, live-vs-mock, webhook state, cart counts.
- `GET /api/square/events` — recent inbound webhooks, for debugging.

### Contact sync

Customers still flow outward to **Square Customers** through the transactional
`sync_outbox` and its background worker (exponential backoff, at-least-once). Marketing
consent maps to Square's inverse field, `preferences.email_unsubscribed`, so someone
who did not opt in is not mailed. Orders no longer go through the outbox — they reach
Square through checkout, which has to be synchronous because an operator is standing
there waiting for a link.

- `GET /api/sync/status` — contact job counts, recent failures.
- `POST /api/sync/retry` — requeue every failed job.

### Going live on Square

The app runs on an **in-process mock** until credentials are set: it produces fake
payment links, charges nobody, and says so in red on both the checkout screen and the
admin panel. The mock still exercises the full path (checkout → payment →
reconciliation), which is how the logic is tested without credentials.

⚠️ The live HTTP client in `api/src/square/http.rs` follows Square's documented
Connect v2 API but **has never made a real request from this box**. Work through this
before taking real money:

1. In the Square developer dashboard, create an application. Take the **Sandbox**
   access token and a sandbox **location id** (`GET /v2/locations`).
2. Set `SQUARE_ENV=sandbox`, `SQUARE_ACCESS_TOKEN`, `SQUARE_LOCATION_ID` in `.env`,
   then `docker compose up -d --build api`. Admin → Square billing should read
   "Live — square-sandbox".
3. Take a full test payment with a [Square test card][sq-test] end to end: build a
   cart, scan the QR, pay, and confirm the cart flips to paid on its own.
4. Create the webhook subscription (`payment.updated`, `refund.updated`) pointing at
   `https://<your-domain>/api/webhooks/square`. Put Square's signature key in
   `SQUARE_WEBHOOK_SIGNATURE_KEY` and the identical URL in `SQUARE_WEBHOOK_URL`.
   Restart, then confirm events land in Admin → Recent Square events.
5. Run Admin → Reconciliation over the test window and confirm it balances against
   what the Square dashboard shows.
6. Only then swap in the **production** token and location and set
   `SQUARE_ENV=production`. The API logs a warning at startup in that mode.

[sq-test]: https://developer.squareup.com/docs/devtools/sandbox/payments

## Status

Milestones 1–7 are done and validated live on the VPS: scaffold + TLS, schema,
operator auth / CRUD / intake, the operator UI, and the reorder endpoint. Billing was
migrated from Squarespace to Square (carts, hosted checkout, webhook receiver,
reconciliation) — validated end-to-end against the mock backend and covered by unit
tests, but **not yet exercised against real Square credentials**. See `RESUME.md` for
current state and open questions.
