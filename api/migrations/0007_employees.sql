-- Employee accounts + RBAC + MFA, replacing the device-token model. Each employee
-- has a password (argon2), a role, and (after enrollment) a TOTP secret; MFA is
-- required for everyone. Sessions are server-side and referenced by an httpOnly
-- cookie.
create table employees (
    id            uuid primary key default gen_random_uuid(),
    email         text not null unique,
    password_hash text not null,
    role          text not null check (role in ('worker', 'admin')),
    -- base32 TOTP secret; null until the employee enrolls MFA.
    totp_secret   text,
    mfa_enrolled  boolean not null default false,
    active        boolean not null default true,
    created_at    timestamptz not null default now(),
    last_login_at timestamptz
);

create table employee_sessions (
    id           uuid primary key default gen_random_uuid(),
    token_hash   text not null unique,
    employee_id  uuid not null references employees (id) on delete cascade,
    -- true between password and MFA steps: the cookie exists but only the MFA
    -- endpoints accept it. Flipped false once MFA is verified.
    mfa_pending  boolean not null default true,
    created_at   timestamptz not null default now(),
    expires_at   timestamptz not null
);

create index employee_sessions_employee_idx on employee_sessions (employee_id);
create index employee_sessions_expires_idx on employee_sessions (expires_at);
