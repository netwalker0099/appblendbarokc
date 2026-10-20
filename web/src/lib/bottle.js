/// Mix amounts are always stored as the 3.4oz base formula; every other size is
/// derived here at display time, never stored per size. Staff may *enter* a
/// blend in any size's ml (MixBuilder converts with toBaseMl). Mirrors the note
/// on `MixItem` in the API.
///
/// Two pairs share a factor because they hold the same volume and so take the
/// same pour, differing only in the bottle and the price:
///
///   * **Vintage / 3.4 oz** — both 100 ml; a vintage-style bottle vs the standard.
///   * **Roller / Spray** — both 10 ml; a rollerball vs an atomiser.
///
/// If either pair ever stops being the same volume they need separate factors
/// here *and* in the API's `BottleSize`.
///
/// Every factor is the bottle's volume over the 100 ml base, so a formula that
/// totals BASE_FORMULA_ML fills every bottle exactly: 100 / 100 / 50 / 30 / 10
/// / 10 ml.
///
/// Order is largest bottle first, and it is the order the operator UI offers.
export const BOTTLE_SIZES = [
  { value: 'vintage', label: 'Vintage', factor: 1 },
  { value: 'oz3_4', label: '3.4 oz', factor: 1 },
  { value: 'oz1_7', label: '1.7 oz', factor: 0.5 },
  // 30 ml. Was a 1.2 oz bottle (factor 1.2/3.4) until 2026-10-05; the code
  // `oz1_2` is kept because orders, packages and prices already store it.
  { value: 'oz1_2', label: '1 oz', factor: 0.3 },
  { value: 'roller', label: 'Roller', factor: 0.1 },
  { value: 'spray', label: 'Spray', factor: 0.1 },
]

export const ORDER_TYPES = [
  { value: 'custom_mix', label: 'Custom mix' },
  { value: 'set_perfume', label: 'Set perfume' },
]

/// Perfumery classification for ingredients. Order here drives grouping order in
/// the mix/scent builders and the admin picker.
export const INGREDIENT_TYPES = [
  { value: 'base', label: 'Base' },
  { value: 'top_note', label: 'Top Note' },
  { value: 'heart_note', label: 'Heart Note' },
]

export function ingredientTypeLabel(value) {
  return INGREDIENT_TYPES.find((t) => t.value === value)?.label ?? value
}

// Order status is no longer chosen anywhere: intake always records a lead, and
// 'paid' is set by Square settling a cart. Kept only as the label lookup for
// displaying a status that already exists.
export const ORDER_STATUS_LABELS = {
  lead: 'Lead',
  paid: 'Paid',
  fulfilled: 'Fulfilled',
}

export function bottleLabel(size) {
  return BOTTLE_SIZES.find((s) => s.value === size)?.label ?? size
}

export function bottleFactor(size) {
  return BOTTLE_SIZES.find((s) => s.value === size)?.factor ?? 1
}

/// Scales a base (3.4oz) amount to the given bottle size. `baseMl` may be a
/// string — the API serialises decimals as strings.
export function scaleMl(baseMl, size) {
  const n = Number(baseMl)
  if (!Number.isFinite(n)) return 0
  return n * bottleFactor(size)
}

/// Trims trailing zeroes so 4.50 reads as "4.5" and 0.10 as "0.1".
export function formatMl(value) {
  const n = Number(value)
  if (!Number.isFinite(n)) return '—'
  return `${parseFloat(n.toFixed(3))}`
}

export function totalMl(items, size = 'oz3_4') {
  return items.reduce((sum, item) => sum + scaleMl(item.amount_ml, size), 0)
}

/// What every formula must total at the 3.4 oz base. Mirrors BASE_FORMULA_ML in
/// the API, which refuses anything else — this only says so before saving.
export const BASE_FORMULA_ML = 100

/// How full a bottle is, in ml: 100 / 100 / 50 / 30 / 10 / 10. A blend for that
/// size must add up to exactly this.
export function bottleMl(size) {
  return BASE_FORMULA_ML * bottleFactor(size)
}

/// Formula amounts are stored at the 3.4 oz base to four decimal places
/// (numeric(8,4) in the API). Staff enter a blend in the ml of the bottle they
/// are filling, and these convert between the two.
///
/// Most sizes convert exactly (x2 for 1.7 oz, x10 for roller and spray). The
/// 1 oz bottle is x100/30, which repeats — 10 ml there is 33.3333… at the base —
/// hence four places, and hence the check below allowing for that last place.
const BASE_DP = 10000

export function toBaseMl(sizeMl, size) {
  const n = Number(sizeMl)
  if (!Number.isFinite(n)) return n
  return Math.round((n / bottleFactor(size)) * BASE_DP) / BASE_DP
}

/// A base amount as it reads in the given size, to the 0.01 ml staff measure in.
export function fromBaseMl(baseMl, size) {
  const n = Number(baseMl)
  if (!Number.isFinite(n)) return n
  return Math.round(n * bottleFactor(size) * 100) / 100
}

/// Where a formula stands for the bottle being filled: { ok, total, message }.
/// `items` hold base amounts (as stored); `size` is the bottle the amounts were
/// entered for, and the message speaks in that bottle's ml.
///
/// The test is the API's: base amounts total 100 ml, give or take half of the
/// fourth decimal place per ingredient (the 1 oz rounding above). Worked in
/// ten-thousandths so float arithmetic can't nudge the answer.
export function formulaCheck(items, size = 'oz3_4') {
  const target = bottleMl(size)
  const label = bottleLabel(size)
  if (!items.length) {
    return { ok: false, total: 0, message: `Add ingredients totalling ${formatMl(target)} ml for the ${label}.` }
  }
  let units = 0
  for (const item of items) {
    const n = Number(item.amount_ml)
    if (item.amount_ml === '' || !Number.isFinite(n) || n <= 0) {
      return { ok: false, total: null, message: 'Every ingredient needs an amount above 0.' }
    }
    units += Math.round(n * BASE_DP)
  }
  const gapUnits = BASE_FORMULA_ML * BASE_DP - units
  // In the bottle's own ml, to the hundredth staff can measure.
  const total = Math.round((units / BASE_DP) * bottleFactor(size) * 100) / 100
  if (Math.abs(gapUnits) <= items.length / 2) {
    return { ok: true, total: target, message: `Adds up to ${formatMl(target)} ml for the ${label}.` }
  }
  const gap = Math.round((gapUnits / BASE_DP) * bottleFactor(size) * 100) / 100
  return {
    ok: false,
    total,
    message:
      gap > 0
        ? `Totals ${formatMl(total)} ml — add ${formatMl(gap)} ml to fill the ${label} (${formatMl(target)} ml).`
        : `Totals ${formatMl(total)} ml — remove ${formatMl(-gap)} ml to fit the ${label} (${formatMl(target)} ml).`,
  }
}
