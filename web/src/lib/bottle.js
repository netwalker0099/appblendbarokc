/// Mix amounts are always stored as the 3.4oz base formula. The 1.7oz bottle is
/// half of that and the roller a tenth — derived here at display time, never
/// stored per size. Mirrors the note on `MixItem` in the API.
export const BOTTLE_SIZES = [
  { value: 'oz3_4', label: '3.4 oz', factor: 1 },
  { value: 'oz1_7', label: '1.7 oz', factor: 0.5 },
  { value: 'roller', label: 'Roller', factor: 0.1 },
]

export const ORDER_TYPES = [
  { value: 'custom_mix', label: 'Custom mix' },
  { value: 'set_perfume', label: 'Set perfume' },
]

export const ORDER_STATUSES = [
  { value: 'paid', label: 'Paid' },
  { value: 'lead', label: 'Lead' },
]

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
