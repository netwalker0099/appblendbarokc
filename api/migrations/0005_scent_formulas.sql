-- Set-perfume scents are house recipes, so give them an ingredient formula just
-- like a custom mix's. Same shape and rules as mix_items: amounts are the base
-- 3.4oz formula (1.7oz is half, roller a tenth — derived at read time), positive,
-- and one row per ingredient. The 8-ingredient cap is enforced in Rust, as with
-- mixes. A scent may have zero rows (formula not defined yet).
create table scent_items (
    scent_id       uuid not null references scents (id),
    ingredient_id  uuid not null references ingredients (id),
    amount_ml      numeric(6, 2) not null check (amount_ml > 0),
    primary key (scent_id, ingredient_id)
);
