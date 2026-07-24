-- Global app settings (single row). Holds the per-size price for custom (bespoke)
-- blends, which are priced uniformly by size rather than per blend.
create table settings (
    id                  boolean primary key default true,
    custom_price_oz3_4  numeric(10, 2),
    custom_price_oz1_7  numeric(10, 2),
    custom_price_roller numeric(10, 2),
    constraint settings_singleton check (id)
);

insert into settings (id) values (true);
