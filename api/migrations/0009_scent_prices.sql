-- Per-size retail prices for scents (3.4oz / 1.7oz / roller). Nullable until an
-- admin sets them; required before a scent can be sold/shared for purchase.
alter table scents
    add column price_oz3_4  numeric(10, 2),
    add column price_oz1_7  numeric(10, 2),
    add column price_roller numeric(10, 2);
