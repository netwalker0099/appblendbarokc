create table customers (
    id                      uuid primary key default gen_random_uuid(),
    email                   text not null unique,
    name                    text,
    marketing_consent       boolean not null default false,
    marketing_consent_at    timestamptz,
    squarespace_contact_id  text,
    created_at              timestamptz not null default now()
);

create table ingredients (
    id          uuid primary key default gen_random_uuid(),
    name        text not null unique,
    active      boolean not null default true,
    created_at  timestamptz not null default now()
);

create table scents (
    id          uuid primary key default gen_random_uuid(),
    name        text not null unique,
    active      boolean not null default true,
    created_at  timestamptz not null default now()
);

create table customer_scent_preferences (
    customer_id  uuid not null references customers (id),
    scent_id     uuid not null references scents (id),
    created_at   timestamptz not null default now(),
    primary key (customer_id, scent_id)
);

create index customer_scent_preferences_scent_id_idx on customer_scent_preferences (scent_id);

create table mixes (
    id           uuid primary key default gen_random_uuid(),
    customer_id  uuid not null references customers (id),
    name         text,
    created_at   timestamptz not null default now()
);

create index mixes_customer_id_idx on mixes (customer_id);

-- amount_ml is the ingredient's amount in the base 3.4oz formula.
-- The 1.7oz bottle is half of these amounts and the roller is a tenth;
-- both are derived at read/order time, never stored separately.
create table mix_items (
    mix_id         uuid not null references mixes (id),
    ingredient_id  uuid not null references ingredients (id),
    amount_ml      numeric(6, 2) not null check (amount_ml > 0),
    primary key (mix_id, ingredient_id)
);

create table orders (
    id                     uuid primary key default gen_random_uuid(),
    customer_id            uuid not null references customers (id),
    type                   text not null check (type in ('set_perfume', 'custom_mix')),
    size                   text not null check (size in ('oz3_4', 'oz1_7', 'roller')),
    mix_id                 uuid references mixes (id),
    scent_id               uuid references scents (id),
    status                 text not null check (status in ('lead', 'paid', 'fulfilled')),
    squarespace_order_id   text,
    external_ref           text,
    amount                 numeric(10, 2),
    created_at             timestamptz not null default now()
);

create index orders_customer_id_idx on orders (customer_id);
