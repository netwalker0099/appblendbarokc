/// Mix amounts are always stored as the 3.4oz base formula. The 1.7oz bottle is
/// half of that; the roller and the spray are a tenth — derived here at display
/// time, never stored per size. Mirrors the note on `MixItem` in the API.
///
/// Roller and spray share a factor because both are 10 ml; they differ only in
/// the closure (rollerball vs atomiser) and in price. If that ever stops being
/// true they need separate factors here *and* in the API's `BottleSize`.
export const BOTTLE_SIZES = [
  { value: 'oz3_4', label: '3.4 oz', factor: 1 },
  { value: 'oz1_7', label: '1.7 oz', factor: 0.5 },
  { value: 'roller', label: 'Roller', factor: 0.1 },
  { value: 'spray', label: 'Spray (10 ml)', factor: 0.1 },
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
