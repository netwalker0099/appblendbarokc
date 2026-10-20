-- Formula amounts to four decimal places (were two).
--
-- Since 2026-10-20 staff enter a blend in the ml of the bottle they are
-- actually filling (a roller's amounts total 10 ml, a 1 oz bottle's 30 ml), and
-- the app scales it up to the 3.4 oz base that is stored. Most sizes scale
-- exactly (x2, x10), but the 1 oz bottle scales by 100/30, so 10 ml of an
-- ingredient there is 33.3333... ml at the base. At two decimals a 10/10/10
-- 1 oz blend would be stored as 33.33 x 3 = 99.99 ml and refused, or pour
-- 0.003 ml short in every bottle. Four decimals keeps the error under
-- 0.00005 ml per ingredient, which the formula check allows for.
--
-- Widening a numeric is lossless; every existing amount keeps its value.
alter table mix_items   alter column amount_ml type numeric(8, 4);
alter table scent_items alter column amount_ml type numeric(8, 4);
