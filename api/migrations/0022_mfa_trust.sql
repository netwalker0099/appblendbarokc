-- Remembering a device for a while after MFA, so staff aren't re-prompted for a
-- code every time they sign in at the stand.
--
-- The prompt staff actually hit is the *second* factor on an ordinary re-login:
-- the tablet sleeps, the session lapses, and the same person on the same device
-- is made to fetch their phone again. This adds a trust window: once MFA has
-- been verified on a device, a later sign-in **with the correct password** from
-- that same device skips the code until the window closes.
--
-- What it deliberately does not do:
--
--   * It never skips the password. The trust cookie on its own authenticates
--     nobody; it is only consulted after argon2 has verified the password, so a
--     stolen tablet with no password is no further ahead than before.
--   * It is bound to one employee. A trust grant issued to alice is ignored when
--     bob signs in on the same tablet — otherwise the shared stand device would
--     hand every employee a free pass off one person's MFA.
--   * It is not a session. It carries no authority of its own, cannot be used to
--     call any endpoint, and is pointless without the password.
--
-- The window is enforced at *check* time against both the row's own expiry and
-- the current setting, so shortening `mfa_trust_hours` (or setting it to 0)
-- takes effect immediately on grants already out in the wild rather than only on
-- newly issued ones. That is what makes the admin toggle a real control and not
-- just a default for the future.

-- 0 disables the window entirely — MFA on every sign-in, the old behaviour. The
-- ceiling is a week: past that the second factor has stopped meaning anything,
-- and "trusted forever" should not be reachable by typing a big number into a
-- box.
alter table settings
    add column mfa_trust_hours integer not null default 4
    check (mfa_trust_hours >= 0 and mfa_trust_hours <= 168);

create table employee_mfa_trust (
    id          uuid primary key default gen_random_uuid(),
    -- sha-256 of the cookie value, never the value itself: the table is in every
    -- backup, and a readable trust token would be a standing MFA bypass for
    -- anyone holding a dump.
    token_hash  text not null unique,
    employee_id uuid not null references employees (id) on delete cascade,
    created_at  timestamptz not null default now(),
    expires_at  timestamptz not null,
    -- Diagnostics only, for "which device is this?" in support. Never matched
    -- against on the auth path: user agents are trivially forged, so treating a
    -- mismatch as suspicious would only add a false sense of binding.
    user_agent  text
);

create index employee_mfa_trust_employee_idx on employee_mfa_trust (employee_id);
create index employee_mfa_trust_expires_idx on employee_mfa_trust (expires_at);
