-- The online shop: products the admin lists for sale on the customer site.
--
-- Two kinds of listing share one table, because the shop shows them side by
-- side and the admin orders them as one list:
--
--   product  a standalone item (gift set, candle, ready-made bottle) with its
--            own price. Sold as a plain cart line; nothing is crafted.
--   scent    a house scent from the catalogue. Its price is NOT stored here —
--            it is read from the scent's per-size prices at checkout, exactly as
--            the share page does, so the catalogue stays the one place a scent
--            is priced. Each bottle sold raises an order for staff to make up.

-- Product photos live in Postgres rather than on a volume: they are small, they
-- belong in every backup alongside the products that use them, and a restore
-- that brought back the products without their pictures would be half a restore.
create table shop_images (
    id           uuid primary key default gen_random_uuid(),
    content_type text not null check (content_type in ('image/jpeg', 'image/png', 'image/webp')),
    bytes        bytea not null,
    created_at   timestamptz not null default now()
);

create table shop_products (
    id          uuid primary key default gen_random_uuid(),
    kind        text not null check (kind in ('product', 'scent')),
    -- Required for a product. For a scent listing, null means "use the scent's
    -- own name", so renaming the scent renames the listing.
    name        text,
    description text,
    price       numeric(10, 2),
    scent_id    uuid references scents (id),
    image_id    uuid references shop_images (id) on delete set null,
    -- Hidden listings stay in the admin list but never reach the site.
    active      boolean not null default true,
    -- Shown on the site, but cannot be added to the bag.
    sold_out    boolean not null default false,
    position    integer not null default 0,
    -- A listing that has ever been sold is archived rather than deleted, so the
    -- cart lines that reference it keep pointing at something.
    archived_at timestamptz,
    created_at  timestamptz not null default now(),
    updated_at  timestamptz not null default now(),
    check (
        (kind = 'product' and name is not null and price is not null and price >= 0
            and scent_id is null)
        or (kind = 'scent' and scent_id is not null and price is null)
    )
);

-- One live listing per scent: two listings of the same scent would offer the
-- same bottles at the same prices twice.
create unique index shop_products_scent_idx
    on shop_products (scent_id)
    where scent_id is not null and archived_at is null;

-- --- Cart lines -------------------------------------------------------------
-- Which listing a line was bought from. Name and price are still snapshotted on
-- the line as before; this is for "how many of these have we sold".
alter table cart_items add column shop_product_id uuid references shop_products (id);

alter table cart_items drop constraint cart_items_kind_check;
alter table cart_items add constraint cart_items_kind_check
    check (kind in ('blend', 'event_deposit', 'fee', 'other', 'product'));

-- --- Shipping ---------------------------------------------------------------
-- A flat rate per shop order, optionally waived above a subtotal. Cents, like
-- every other amount the checkout handles.
alter table settings
    add column shop_shipping_cents           bigint not null default 0
        check (shop_shipping_cents >= 0),
    add column shop_free_shipping_over_cents bigint
        check (shop_free_shipping_over_cents is null or shop_free_shipping_over_cents > 0);
