-- Two more bottle sizes: **Vintage** and **1.2 oz**.
--
-- Vintage is a 100 ml bottle in a vintage style — the *same pour* as the 3.4 oz,
-- so it introduces no new scaling factor, exactly like Spray to Roller. What it
-- needs is its own price, because the bottle costs more than the standard one.
--
-- 1.2 oz is a genuinely new volume and does bring a new factor (1.2 / 3.4), which
-- lives in the frontend's `bottle.js` alongside the others. Nothing is stored per
-- size: amounts remain the 3.4 oz base formula and every other size is derived at
-- display time.
--
-- Sizes stay a text check constraint rather than a Postgres enum, matching how
-- `type` and `status` are already modelled — and matching migration 0012, which
-- added Spray the same way.

alter table orders drop constraint orders_size_check;
alter table orders add constraint orders_size_check
    check (size in ('vintage', 'oz3_4', 'oz1_7', 'oz1_2', 'roller', 'spray'));

-- Package deals name their bottles by size too, and were missed by nothing so
-- far only because no size had been added since they shipped. Both constraints
-- have to list the same set or a package could never contain the new bottles.
alter table bundle_items drop constraint bundle_items_size_check;
alter table bundle_items add constraint bundle_items_size_check
    check (size in ('vintage', 'oz3_4', 'oz1_7', 'oz1_2', 'roller', 'spray'));

-- Per-scent retail prices. Nullable like every other size: null means "not sold
-- in this size", and the public share page hides the option rather than offering
-- a bottle with no price.
alter table scents
    add column price_vintage numeric(10, 2),
    add column price_oz1_2   numeric(10, 2);

-- Bespoke blends are priced uniformly by size, so the singleton settings row
-- needs the same two columns.
alter table settings
    add column custom_price_vintage numeric(10, 2),
    add column custom_price_oz1_2   numeric(10, 2);
