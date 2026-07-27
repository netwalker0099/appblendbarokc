#!/usr/bin/env python3
"""
End-to-end check of the Square billing path against a running stack.

Drives the real HTTP API the way the operator app does: sign in as an employee,
take an intake, build a cart, check it out, settle it, and reconcile. Against the
mock Square backend this exercises every piece of local logic — cart assembly,
money conversion, the payment-link call, apply_payment, and all five
reconciliation buckets — without needing Square credentials.

It does NOT prove anything about the live Square wire format. Only a sandbox pass
with real credentials can do that; see "Going live on Square" in README.md.

Usage (from /opt/app):
    docker compose exec -T api blendbar-api create-admin e2e@blendbar.local
    python3 api/e2e_square.py --email e2e@blendbar.local --password <temp>

Writes real rows (one customer, orders, a cart). Intended for a test account.
"""
import argparse
import base64
import hashlib
import hmac

import json
import struct
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from datetime import datetime, timedelta, timezone

BASE = "http://localhost:8080"

# The session cookie is correctly marked Secure, so a standard cookie jar will not
# replay it over the plain-HTTP loopback this script uses. Rather than weaken the
# cookie (it should stay Secure) or reach through Caddy's TLS, carry it by hand:
# capture Set-Cookie and echo it back on later requests.
opener = urllib.request.build_opener()
session_cookie = None

failures = []
checks = 0


def check(label, condition, detail=""):
    global checks
    checks += 1
    if condition:
        print(f"  ok   {label}")
    else:
        print(f"  FAIL {label} {detail}")
        failures.append(label)


def call(method, path, body=None, headers=None):
    global session_cookie
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(f"{BASE}{path}", data=data, method=method)
    if data:
        req.add_header("Content-Type", "application/json")
    if session_cookie:
        req.add_header("Cookie", session_cookie)
    for k, v in (headers or {}).items():
        req.add_header(k, v)

    def capture(resp_headers):
        global session_cookie
        raw = resp_headers.get("Set-Cookie")
        if raw:
            session_cookie = raw.split(";", 1)[0]

    try:
        with opener.open(req, timeout=30) as resp:
            capture(resp.headers)
            raw = resp.read()
            if not raw:
                return resp.status, None
            try:
                return resp.status, json.loads(raw)
            except json.JSONDecodeError:
                # Not every endpoint is JSON — the checkout QR is an SVG.
                return resp.status, raw
    except urllib.error.HTTPError as e:
        capture(e.headers)
        raw = e.read()
        try:
            return e.code, json.loads(raw)
        except Exception:
            return e.code, {"error": raw.decode(errors="replace")}


def totp(secret_b32, when=None):
    """RFC 6238 TOTP, 6 digits / 30s — same as the app's authenticator."""
    key = base64.b32decode(secret_b32.upper() + "=" * (-len(secret_b32) % 8))
    counter = int((when or time.time()) // 30)
    digest = hmac.new(key, struct.pack(">Q", counter), hashlib.sha1).digest()
    offset = digest[-1] & 0x0F
    code = struct.unpack(">I", digest[offset : offset + 4])[0] & 0x7FFFFFFF
    return f"{code % 1000000:06d}"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--email", required=True)
    ap.add_argument("--password", required=True)
    ap.add_argument("--totp-secret", help="skip enrollment if already enrolled")
    args = ap.parse_args()

    print("\n== sign in ==")
    status, body = call(
        "POST", "/api/auth/login", {"email": args.email, "password": args.password}
    )
    check("login accepted", status == 200, f"{status} {body}")
    if status != 200:
        return 1

    # Login returns status = "enroll_required" (no MFA yet) or "mfa_required".
    secret = args.totp_secret
    if body.get("status") == "enroll_required":
        status, enroll = call("POST", "/api/auth/mfa/enroll")
        check("mfa enrollment offered", status == 200, f"{status} {enroll}")
        secret = enroll.get("secret")

    if not secret:
        print("  need --totp-secret for an already-enrolled account")
        return 1

    status, body = call("POST", "/api/auth/mfa/verify", {"code": totp(secret)})
    check("mfa verified", status == 200, f"{status} {body}")

    status, me = call("GET", "/api/auth/me")
    check("session is live", status == 200 and me.get("email") == args.email, str(me))

    print("\n== square status ==")
    status, sq = call("GET", "/api/square/status")
    check("square status readable", status == 200, f"{status} {sq}")
    check("running on the mock backend", sq.get("live") is False, str(sq.get("backend")))
    print(f"       backend={sq.get('backend')} webhooks={sq.get('webhook_receiver_enabled')}")

    print("\n== intake: create a priced order ==")
    email = f"e2e-{uuid.uuid4().hex[:8]}@example.invalid"
    status, scents = call("GET", "/api/scents")
    active = [s for s in scents if s.get("active")]
    check("an active scent exists to sell", bool(active))
    if not active:
        return 1

    status, intake = call(
        "POST",
        "/api/intake",
        {
            "email": email,
            "name": "E2E Test",
            "marketing_consent": False,
            "order": {
                "type": "set_perfume",
                "size": "oz3_4",
                "status": "lead",
                "scent_id": active[0]["id"],
                "amount": "60.00",
            },
        },
        {"Idempotency-Key": str(uuid.uuid4())},
    )
    check("intake created", status == 201, f"{status} {intake}")
    if status != 201:
        return 1
    customer_id = intake["customer"]["id"]
    order_id = intake["order"]["id"]
    check("order starts as a lead", intake["order"]["status"] == "lead")

    print("\n== cart: blend + an ad-hoc deposit line ==")
    status, cart = call(
        "POST",
        "/api/carts",
        {
            "customer_id": customer_id,
            "order_ids": [order_id],
            # Exercises the ad-hoc path and a x2 quantity at once.
            "items": [{"name": "Event deposit (50%)", "quantity": 2, "unit_amount": "19.99"}],
        },
    )
    check("cart created", status == 201, f"{status} {cart}")
    if status != 201:
        return 1
    cart_id = cart["id"]
    # 60.00 + (19.99 x 2) = 99.98 -> 9998 cents. Catches any rounding slip.
    check("total is exact to the cent", cart["total_cents"] == 9998, f"got {cart['total_cents']}")
    check("cart starts open", cart["status"] == "open")
    check("idempotency key is not leaked to clients", "idempotency_key" not in cart)

    print("\n== double-sell guard ==")
    status, dup = call(
        "POST", "/api/carts", {"customer_id": customer_id, "order_ids": [order_id]}
    )
    check("same order cannot be carted twice", status >= 400, f"{status} {dup}")

    status, uncarted = call("GET", f"/api/orders?customer_id={customer_id}&uncarted=true")
    check("carted order is hidden from the checkout list", all(o["id"] != order_id for o in uncarted))

    print("\n== checkout ==")
    status, co = call("POST", f"/api/carts/{cart_id}/checkout")
    check("checkout returned a link", status == 200, f"{status} {co}")
    if status != 200:
        return 1
    check("charged amount matches the cart", co["total_cents"] == 9998)
    check("mock mode is reported to the UI", co["live"] is False)

    status, again = call("POST", f"/api/carts/{cart_id}/checkout")
    check(
        "pressing checkout twice reuses the same Square order",
        status == 200 and again["square_order_id"] == co["square_order_id"],
        f"{status} {again}",
    )

    status, qr = call("GET", f"/api/carts/{cart_id}/checkout.svg")
    check(
        "QR code renders as SVG",
        status == 200 and isinstance(qr, bytes) and b"<svg" in qr,
        f"{status}",
    )

    print("\n== settle (pull path, as if the webhook was lost) ==")
    status, refreshed = call("POST", f"/api/carts/{cart_id}/refresh")
    check("refresh found the payment", status == 200 and refreshed["found"], f"{status} {refreshed}")
    check("cart is now paid", refreshed["status"] == "paid", str(refreshed))

    status, again = call("POST", f"/api/carts/{cart_id}/refresh")
    check("refreshing twice is idempotent", status == 200 and again["status"] == "paid")

    status, final = call("GET", f"/api/carts/{cart_id}")
    check("paid amount recorded", final["paid_cents"] == 9998, str(final.get("paid_cents")))
    check("paid_at stamped", final.get("paid_at") is not None)

    status, order = call("GET", f"/api/orders/{order_id}")
    check("the blend flipped to paid", order["status"] == "paid", str(order["status"]))

    print("\n== reconciliation ==")
    frm = (datetime.now(timezone.utc) - timedelta(hours=1)).isoformat()
    to = (datetime.now(timezone.utc) + timedelta(minutes=5)).isoformat()
    status, rep = call(
        "GET",
        f"/api/square/reconcile?from={urllib.parse.quote(frm)}&to={urllib.parse.quote(to)}",
    )
    check("report generated", status == 200, f"{status} {rep}")
    if status != 200:
        return 1

    # Assertions are scoped to the cart this run created. The database is shared
    # and may legitimately hold discrepancies from earlier runs or real trading;
    # a global "everything balances" check would be flaky and, worse, would train
    # whoever runs this to ignore red.
    ours = [m for m in rep["matched"] if m["cart_id"] == cart_id]
    check("our cart reconciles as matched", len(ours) == 1, rep["summary"])
    check("matched at the right amount", bool(ours) and ours[0]["cents"] == 9998)

    def not_bucketed(bucket):
        return all(r.get("cart_id") != cart_id for r in rep[bucket])

    check("our cart is not an amount mismatch", not_bucketed("amount_mismatch"))
    check("our cart is not missing in Square", not_bucketed("missing_in_square"))
    check("our cart is not an unrecorded payment", not_bucketed("unrecorded_payment"))
    check("report knows it is not live", rep["live"] is False)
    print(f"       {rep['summary']}")
    for bucket in ("amount_mismatch", "missing_in_square", "unrecorded_payment", "missing_locally"):
        if rep[bucket]:
            print(f"       (pre-existing in DB: {len(rep[bucket])} {bucket})")

    print("\n== cancellation releases the order ==")
    status, c2 = call("POST", "/api/carts", {"customer_id": customer_id, "items": [
        {"name": "Rush fee", "quantity": 1, "unit_amount": "25.00"}]})
    check("second cart created", status == 201, f"{status} {c2}")
    if status == 201:
        status, canceled = call("POST", f"/api/carts/{c2['id']}/cancel")
        check("cart canceled", status == 200 and canceled["status"] == "canceled", str(canceled))
        status, _ = call("POST", f"/api/carts/{cart_id}/cancel")
        check("a paid cart cannot be canceled", status >= 400, f"got {status}")

    print("\n== public share checkout (no auth) ==")
    saved_cookie = globals()["session_cookie"]
    globals()["session_cookie"] = None  # prove these paths need no session

    scent_id = active[0]["id"]
    status, pub = call("GET", f"/api/public/scent/{scent_id}")
    check("public scent view is readable anonymously", status == 200, f"{status}")
    check(
        "formula amounts are never exposed publicly",
        isinstance(pub, dict) and "items" not in pub and "amount_ml" not in json.dumps(pub),
        str(pub)[:200],
    )

    # Square is on the mock here, so checkout must refuse rather than hand a real
    # customer a fake link. That refusal IS the expected behaviour in this mode.
    status, res = call(
        "POST",
        "/api/public/checkout",
        {"scent_id": scent_id, "size": "oz3_4", "email": "buyer@example.invalid"},
    )
    if status == 503:
        check("mock mode refuses public checkout rather than faking a link", True)
        print("       (Square is on the mock — this is the correct refusal)")
    else:
        check("public checkout succeeded", status == 200, f"{status} {res}")
        check("returned a checkout url", bool((res or {}).get("checkout_url")))

    # Input validation runs BEFORE the Square-availability check, so these assert
    # exact codes. If validation ever moves back behind the 503 these fail loudly
    # rather than passing vacuously, which is the whole point of pinning them.
    status, _ = call(
        "POST",
        "/api/public/checkout",
        {"scent_id": scent_id, "size": "oz3_4", "email": "not-an-email"},
    )
    check("bad email rejected with 400", status == 400, f"got {status}")

    status, _ = call(
        "POST",
        "/api/public/checkout",
        {"scent_id": str(uuid.uuid4()), "size": "oz3_4", "email": "buyer@example.invalid"},
    )
    check("unknown scent rejected with 404", status == 404, f"got {status}")

    status, _ = call(
        "POST",
        "/api/public/checkout",
        {"scent_id": scent_id, "size": "not_a_size", "email": "buyer@example.invalid"},
    )
    check("bogus size rejected", status in (400, 422), f"got {status}")

    # Extra price-shaped fields in the body must be ignored outright. Reaching the
    # 503 proves the request was accepted as valid and priced from the database —
    # had any of these been honoured as a price, it would have failed earlier.
    status, _ = call(
        "POST",
        "/api/public/checkout",
        {
            "scent_id": scent_id,
            "size": "oz3_4",
            "email": "buyer@example.invalid",
            "amount": "0.01",
            "unit_amount": "0.01",
            "total_cents": 1,
        },
    )
    check("price fields in the request are ignored", status == 503, f"got {status}")

    globals()["session_cookie"] = saved_cookie

    # A refused checkout must not leave anything behind. Needs the session back,
    # so it runs after the anonymous block above.
    status, found = call("GET", "/api/customers?email=buyer@example.invalid")
    check(
        "a refused checkout persisted nothing",
        status == 200 and not any(c["email"] == "buyer@example.invalid" for c in (found or [])),
        f"{status} {found}",
    )

    print("\n== webhook receiver is refusing unsigned calls ==")
    status, body = call("POST", "/api/webhooks/square", {"event_id": "x", "type": "payment.updated"})
    check(
        "unsigned webhook rejected (503 disabled / 401 unsigned)",
        status in (401, 503),
        f"got {status} {body}",
    )

    print(f"\n{checks - len(failures)}/{checks} checks passed")
    if failures:
        print("FAILURES:")
        for f in failures:
            print(f"  - {f}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
