# The Blend Bar — Executive Brief

**Project:** Perfume stand intake & billing platform
**Prepared:** 2026-07-27
**Status:** Built and deployed · **not yet able to take real money**
**Owner action required:** Yes — see [What we need from you](#what-we-need-from-you)

---

## In one paragraph

The Blend Bar app records customers, their scent preferences, and the blends made
for them, and it now takes payment through **Square**. This cycle replaced the old
Squarespace connection with a Square-based checkout, added an online "Buy" path so
someone sent a scent link can purchase it themselves, added the new 10 ml Spray
bottle, and published the event booking terms on the public site. Everything is
live on the server and covered by automated tests. **One thing stands between this
and taking real payments: Square account credentials, which we do not have yet.**

---

## What changed this cycle

| # | Delivered | Why it matters |
|---|---|---|
| 1 | **Square replaces Squarespace for billing** | Squarespace never actually collected money — staff typed "paid" by hand. Square now takes the payment and tells us the result. |
| 2 | **Cart + hosted checkout** | Staff build a cart (one or more blends, plus deposits or fees) and show a QR code. The customer pays on Square's page using their own phone. |
| 3 | **Sales reconciliation** | A report that compares what the app recorded selling against what Square actually collected, and flags every discrepancy. |
| 4 | **Public "Buy" button on share links** | Someone sent a `/s/…` scent link can now buy it themselves without an account. |
| 5 | **Fourth bottle size: Spray (10 ml)** | Sits alongside 3.4 oz, 1.7 oz, and Roller. |
| 6 | **Booking & cancellation terms published** | Now shown in the "Book an Event" section of the public site. |

---

## Why Square, and why it is handled this way

**No card details ever touch our system.** The app hands the cart to Square, and the
customer types their card on Square's own payment page. Card data never crosses the
tablet, our server, or the shop's network.

This is a deliberate compliance choice. It keeps the business at the lightest tier
of card-industry obligation (**PCI SAQ-A**) instead of taking on the burden — and
the liability — of handling card data. It also means a security incident on our
server cannot expose customer card numbers, because they were never there.

---

## Reconciliation: knowing the books are right

Every sale is sorted into exactly one bucket:

| Bucket | Meaning | Action |
|---|---|---|
| **Matched** | Both sides agree, to the cent | None |
| **Amount mismatch** | Both have the sale, totals differ | Check for a tip, discount, or a price edited in Square |
| **Missing in Square** | We recorded a sale Square has no payment for | **Investigate** — money may not have been taken |
| **Paid but unrecorded** | Square collected, our record didn't update | One-click fix; usually a lost notification |
| **Only in Square** | Square collected, no matching record here | Normally a sale rung up directly in the Square POS |

Reports can be saved, so a discrepancy found today stays inspectable months later
even after the underlying records are corrected.

---

## Current status

| Area | State |
|---|---|
| Public site (sandbox.theblendbarokc.com) | **Live** — booking terms published |
| Staff app (app.theblendbarokc.com) | **Live** — intake, lookup, checkout, admin |
| Customer portal | **Live** |
| Square billing | **Built, running in safe simulation mode** |
| Taking real payments | **Blocked — needs Square credentials** |
| Automated tests | 33 unit tests + 50 end-to-end checks, all passing |
| Code | Committed and pushed to GitHub |

### What "safe simulation mode" means

Without Square credentials the app runs against a built-in stand-in. It behaves
correctly end to end, but **charges nobody**. This is shown in red on both the
staff checkout screen and the admin panel, and the public Buy button deliberately
refuses rather than sending a customer to a link that cannot work. Nobody can
mistake a simulated sale for a real one.

---

## Risks and honest caveats

**1. The live Square connection has never been tested. (Highest risk)**
The code follows Square's published specification, but no request has ever been
sent to Square from this server, because there are no credentials. Expect small
corrections during first setup. **Do not switch straight to live payments** — use
Square's free sandbox and a test card first. The step-by-step checklist is in the
technical README under "Going live on Square".

**2. Spray prices are not set.**
The size exists everywhere, but no price is attached to it. Until prices are
entered, Spray will not appear as a purchase option. This is intentional — the
system will not invent a price.

**3. The booking terms are website copy, not a signed agreement.**
They are published where guests will read them before enquiring, which is the right
place. But a website section is not something a client actively agrees to. If
deposits are being collected, the same wording should appear on the invoice or a
signed event agreement. **This is a legal question worth putting to a professional,
not a software one.**

**4. Payment notifications are not yet switched on.**
Square can automatically notify the app when a payment lands. That requires a
setting we cannot configure without account access. Until then, staff press
"Check Square" on the checkout screen to confirm a payment — which works, but is a
manual step.

**5. Online orders are made by hand.**
A blend bought through a share link does not exist yet when it is paid for. Orders
are flagged for staff as "to be crafted", and the confirmation page tells the buyer
their blend is made by hand so they are not left waiting for a tracking number.
**There is no fulfilment or shipping workflow yet** — someone has to watch for these
orders and act on them.

---

## What we need from you

| # | Action | Why | Effort |
|---|---|---|---|
| 1 | **Square Sandbox** access token + location ID | Unblocks testing the real connection | ~10 min in Square's developer dashboard |
| 2 | Then: **Square Production** credentials | Switches on real payments, once sandbox passes | ~10 min |
| 3 | **Set Spray prices** — per scent and for custom blends | Spray cannot be sold until priced | ~10 min in Admin |
| 4 | **Confirm booking terms wording** with a legal advisor | These are contractual terms | Your call |
| 5 | **Decide who watches online orders** | Nobody is assigned to fulfil them | Your call |

Items 1–3 are the critical path to taking money. Nothing else blocks it.

---

## Recommended sequence

1. Create a Square **sandbox** application; send us the token and location ID.
2. We connect it and run a full test purchase with a Square test card.
3. We confirm the reconciliation report matches the Square dashboard exactly.
4. Switch on payment notifications.
5. Only then, swap in **production** credentials and take a small real payment as a
   final check.
6. Set Spray prices and open the size for sale.

Steps 1–5 are a single short working session once credentials exist.

---

## Deferred, deliberately

- **Reorder-to-checkout in the customer portal** — customers can request a reorder,
  but it does not yet flow through the new online payment path.
- **Fulfilment/shipping workflow** for online orders (see risk 5).
- **Custom blend share pages** — only set scents can be shared and bought online.
- **Automated customer email** — the app does not send email; Square sends the
  payment receipt.

---

*Technical detail: `README.md`. Full engineering history and open questions:
`RESUME.md`.*
