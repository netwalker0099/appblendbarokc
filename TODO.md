# TODO

Open items are `- [ ]`; tick one off with `- [x]` when done. Every open item is
shown at the start of each Claude Code session until it is ticked.

- [x] **Promote the shop to production.** Built 2026-10-05 (RESUME.md →
  Milestone 16) and live on the sandbox site at `/shop`, where the "Shop Now"
  button now points. The production site still links to the old Squarespace
  store. First, add the real products in Admin → Shop and look over
  `sandbox.theblendbarokc.com/shop` in a browser, on desktop and phone. Then
  promote with `rsync -a --delete site/ site-prod/` and
  `docker compose up --build -d caddy`. This also brings across the redesign
  `site-prod/` has been missing — see README "Promoting the sandbox site".
  Shop checkout stays off until the Square go-live below.
  Done 2026-10-12: promoted with one listing (The Blend No. 1, $7 shipping).
  Every page returns 200, with no Squarespace links left. Still to do: look
  it over in a real browser on desktop and phone.

**Square go-live** — resume here. Context: RESUME.md → Milestone 15 (Sandbox) and
Milestone 17 (Square Terminal, 2026-10-08).

- [x] **Buy a Square Terminal** (the standalone Wi-Fi unit, not a Reader/puck).
  The app charges it from any browser — laptop or iPad — so no iPad is required.
  Purchased (confirmed 2026-10-12).
- [x] **OAuth with least-privilege permissions**: built and deployed 2026-10-12
  as **Connect Square** (README → "Connect Square (OAuth)", RESUME → Milestone
  18). Production app id + secret are in `.env`; contact sync kept, so
  `CUSTOMERS_READ/WRITE` are included. Dormant until `SQUARE_ENV=production`.

- [x] **`chmod 600 /opt/app/.env`.** It is `644`; it now holds the Square token.
- [x] **Square sandbox webhooks.** Developer dashboard → Sandbox → Webhooks →
  subscribe `https://app.theblendbarokc.com/api/webhooks/square` (exactly) to
  `payment.created`, `payment.updated`, `refund.created`, `refund.updated`,
  `terminal.checkout.created`, `terminal.checkout.updated`, `device.code.paired`. Owner
  adds the signature key to `.env` as `SQUARE_WEBHOOK_SIGNATURE_KEY`; then
  `docker compose up -d api` and check the receiver is no longer "disabled".
- [x] **Try the Terminal flow yourself in Sandbox**: Admin → Billing → Square
  Terminals → add the "Approves" test device; then Intake → Save → Charge. (Sandbox
  approves up to $25.) Then a refund test in the Sandbox dashboard.
  Done 2026-10-12: $2.00 Terminal charge (cart `5c125933…`) paid, then refunded
  via the Refunds API (the sandbox dashboard showed no refund button); webhook
  moved the cart paid → refunded within a second.
- [x] **Decide on the $1.00 test cart** `2ee6ac46…` on "Ryan Taylor (example)" —
  confirm that customer is a test record; cancel or keep as a marked test.
  Owner confirmed 2026-10-12: it is a test record. Refunded in Sandbox via the
  Refunds API the same day; the webhook moved the cart paid → refunded.
- [x] **Commit.** ~32 files of already-deployed work plus the 2026-10-01
  public-checkout guard (`api/src/routes/public.rs`) and doc updates are
  uncommitted. Commit the guard + docs on their own.
  Done 2026-10-12: the guard + docs were already in `bac483e` (2026-10-01); the
  rest is `38ad145`, together with `site-prod/` (web/Dockerfile copies it in).
- [ ] **Look at the live site in a browser**, on desktop and phone:
  `theblendbarokc.com` and `/shop`. If anything is wrong, `site-prod/` is in git
  (commit before `c781ca4`) to roll back.
- [ ] **Go live on Square**, in this order:
  1. Developer Dashboard → the app → **Production** → OAuth: set Redirect URL to
     `https://app.theblendbarokc.com/api/square/oauth/callback` (exactly).
  2. Production → Webhooks: subscribe the same URL to the same seven events as
     sandbox; the owner replaces `SQUARE_WEBHOOK_SIGNATURE_KEY` in `.env` with
     the **production** subscription's key.
  3. Owner sets in `.env`: `SQUARE_ENV=production`, `SQUARE_LOCATION_ID` = the
     production location (Developer Dashboard → Locations, Production), and
     deletes the `SQUARE_ACCESS_TOKEN` line (the sandbox token; ignored anyway).
  4. `docker compose up -d api`; log should say `auth="oauth"` and "not
     connected yet". Admin → Billing → **Connect Square** → approve. Check the
     permissions shown match README → "Square permissions".
  5. Pair the real Terminal (Admin → Billing → Get a pairing code).
  6. One small real Terminal payment, then refund it in the Square Dashboard;
     confirm the cart goes paid → refunded.
  7. A share-link purchase and a shop order with shipping — check the address
     appears on the Square order (public and shop checkout open automatically).
