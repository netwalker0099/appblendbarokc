-- Add a fourth bottle size: Spray — a 10 ml bottle with a spray top.
--
-- Volume-wise this is the same 10 ml as the roller; only the closure differs
-- (atomiser instead of a rollerball). That means it takes the **same pour** as
-- the roller — a tenth of the stored 3.4oz base formula — so no new scaling
-- factor is introduced anywhere. What it does need is its own retail price,
-- because a spray top costs more than a rollerball.
--
-- Sizes remain a text check constraint rather than a Postgres enum, matching how
-- `type` and `status` are already modelled on this table.

alter table orders drop constraint orders_size_check;
alter table orders add constraint orders_size_check
    check (size in ('oz3_4', 'oz1_7', 'roller', 'spray'));

-- Per-scent retail price for the new size. Nullable like the others: null means
-- "not sold in this size", and the public share page hides the option.
alter table scents add column price_spray numeric(10, 2);

-- Custom bespoke blends are priced uniformly by size, so the singleton settings
-- row needs the same new column.
alter table settings add column custom_price_spray numeric(10, 2);
