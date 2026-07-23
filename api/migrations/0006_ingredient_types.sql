-- Classify each ingredient by its perfumery role. The three tiers a fragrance is
-- built from: base notes, top notes, and heart (middle) notes. Both custom mixes
-- and scent formulas draw from these, so the classification applies everywhere an
-- ingredient is used. NOT NULL with a default so existing rows get a value; new
-- ingredients set it explicitly from the admin UI.
alter table ingredients
    add column type text not null default 'heart_note'
    check (type in ('base', 'top_note', 'heart_note'));

-- Reasonable starting classifications for the seed ingredients (no-ops on a fresh
-- deploy that has no ingredients yet).
update ingredients set type = 'top_note' where name = 'Bergamot';
update ingredients set type = 'base' where name = 'Sandalwood';
