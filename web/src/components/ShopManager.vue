<script setup>
/**
 * The online shop: what the customer site's /shop page sells.
 *
 * Two kinds of listing sit in one list, because the shop shows them side by
 * side:
 *   - a **product** — a standalone item with its own price;
 *   - a **scent** — a house scent from the Catalog, sold at the per-size prices
 *     set there. Its price is never typed here, so the Catalog stays the one
 *     place a scent is priced.
 *
 * Loads its own data rather than taking it from AdminView: nothing else on the
 * page needs it, and the list changes on almost every action here.
 */
import { computed, onMounted, reactive, ref, watch } from 'vue'

import { api, uploadShopImage } from '../lib/api.js'
import { BOTTLE_SIZES } from '../lib/bottle.js'

const props = defineProps({
  scents: { type: Array, default: () => [] },
})
// A scent's size prices are saved on the scent itself, so AdminView's copy
// (which Catalog and the price summaries read) has to hear about it.
const emit = defineEmits(['scent-updated'])

const products = ref([])
const loading = ref(true)
const busy = ref('')
const error = ref('')
const notice = ref('')

const shipping = reactive({ fee: '', freeOver: '' })

const dollars = (cents) => (cents == null ? '' : (Number(cents) / 100).toFixed(2))
const money = (v) => `$${Number(v).toFixed(2).replace(/\.00$/, '')}`

onMounted(load)

async function load() {
  loading.value = true
  error.value = ''
  try {
    const [list, settings] = await Promise.all([api.listShopProducts(), api.getShopSettings()])
    products.value = list
    shipping.fee = dollars(settings.shop_shipping_cents)
    shipping.freeOver = dollars(settings.shop_free_shipping_over_cents)
  } catch (err) {
    error.value = err.message
  } finally {
    loading.value = false
  }
}

async function reload() {
  try {
    products.value = await api.listShopProducts()
  } catch (err) {
    error.value = err.message
  }
}

function say(msg) {
  notice.value = msg
  error.value = ''
}

/* --- Listing summaries -------------------------------------------------- */

function scentFor(p) {
  return props.scents.find((s) => s.id === p.scent_id)
}

/** The catalogue prices a scent listing will sell at, smallest first. */
function scentPrices(scent) {
  if (!scent) return []
  return BOTTLE_SIZES.map((s) => ({ label: s.label, price: scent[`price_${s.value}`] }))
    .filter((s) => s.price != null && Number(s.price) > 0)
    .sort((a, b) => Number(a.price) - Number(b.price))
}

function priceText(p) {
  if (p.kind === 'product') return money(p.price)
  const prices = scentPrices(scentFor(p))
  if (!prices.length) return 'No prices set'
  return prices.length === 1 ? money(prices[0].price) : `From ${money(prices[0].price)}`
}

/** "Sizes: 3.4 oz, Roller" for a scent listing — what the customer can choose from. */
function sizesText(p) {
  if (p.kind !== 'scent') return ''
  const scent = scentFor(p)
  if (!scent) return ''
  const sold = BOTTLE_SIZES.filter((s) => scent[`price_${s.value}`] != null && Number(scent[`price_${s.value}`]) > 0)
  return sold.length ? `Sizes: ${sold.map((s) => s.label).join(', ')}` : ''
}

/** A scent with a single price can't offer a choice, which is easy to miss from here. */
function singleSize(p) {
  return p.kind === 'scent' && scentPrices(scentFor(p)).length === 1
}

/** Why a listing that is switched on still won't appear in the shop, if it won't. */
function problem(p) {
  if (p.kind !== 'scent') return ''
  if (p.scent_active === false) return 'This scent is inactive in the Catalog, so the shop hides it.'
  if (!scentPrices(scentFor(p)).length)
    return 'This scent has no prices yet, so the shop hides it. Edit it to add them.'
  return ''
}

/* --- Shipping ----------------------------------------------------------- */

async function saveShipping() {
  busy.value = 'shipping'
  try {
    const fee = shipping.fee === '' ? 0 : Math.round(Number(shipping.fee) * 100)
    const freeOver = shipping.freeOver === '' ? null : Math.round(Number(shipping.freeOver) * 100)
    if (!Number.isFinite(fee) || fee < 0) throw new Error('Enter a shipping charge of 0 or more.')
    if (freeOver !== null && (!Number.isFinite(freeOver) || freeOver <= 0))
      throw new Error('Leave free shipping blank, or enter an amount above zero.')
    const set = await api.updateShopSettings({
      shop_shipping_cents: fee,
      shop_free_shipping_over_cents: freeOver,
    })
    shipping.fee = dollars(set.shop_shipping_cents)
    shipping.freeOver = dollars(set.shop_free_shipping_over_cents)
    say('Saved shipping.')
  } catch (err) {
    error.value = err.message
  } finally {
    busy.value = ''
  }
}

/* --- The form ----------------------------------------------------------- */

const editing = ref(null)
const blank = () => ({
  kind: 'product',
  name: '',
  description: '',
  price: '',
  scent_id: '',
  image_id: null,
  image_url: null,
  active: true,
  sold_out: false,
  // Per-size prices for a scent listing, as typed (strings; '' = not sold).
  prices: Object.fromEntries(BOTTLE_SIZES.map((s) => [s.value, ''])),
})
const draft = reactive(blank())

/// Fill the size prices from the chosen scent whenever the scent changes, so
/// the boxes always start from what the scent is really sold at.
function loadScentPrices(scentId) {
  const scent = props.scents.find((s) => s.id === scentId)
  for (const size of BOTTLE_SIZES) {
    const v = scent ? scent[`price_${size.value}`] : null
    draft.prices[size.value] = v == null ? '' : String(Number(v).toFixed(2))
  }
}
watch(() => draft.scent_id, loadScentPrices)
const uploading = ref(false)
const fileInput = ref(null)

/** Scents that can still be listed — one live listing per scent. */
const listableScents = computed(() => {
  const taken = new Set(
    products.value.filter((p) => p.scent_id && p.id !== editing.value).map((p) => p.scent_id),
  )
  return props.scents.filter((s) => !taken.has(s.id))
})


function reset() {
  Object.assign(draft, blank())
  draft.prices = blank().prices
  editing.value = null
  if (fileInput.value) fileInput.value.value = ''
}

function edit(p) {
  editing.value = p.id
  Object.assign(draft, {
    kind: p.kind,
    name: p.name || '',
    description: p.description || '',
    price: p.price ?? '',
    scent_id: p.scent_id || '',
    image_id: p.image_id,
    image_url: p.image_url,
    active: p.active,
    sold_out: p.sold_out,
  })
  loadScentPrices(p.scent_id)
  notice.value = ''
  document.getElementById('shop-form')?.scrollIntoView({ behavior: 'smooth', block: 'start' })
}

async function pickImage(event) {
  const file = event.target.files?.[0]
  if (!file) return
  uploading.value = true
  error.value = ''
  try {
    const img = await uploadShopImage(file)
    draft.image_id = img.id
    draft.image_url = img.url
  } catch (err) {
    error.value = err.message
  } finally {
    uploading.value = false
    event.target.value = ''
  }
}

function clearImage() {
  draft.image_id = null
  draft.image_url = null
}

function bodyFrom(source) {
  const isProduct = source.kind === 'product'
  return {
    kind: source.kind,
    name: (source.name || '').trim() || null,
    description: (source.description || '').trim() || null,
    price: isProduct && source.price !== '' && source.price != null ? Number(source.price) : null,
    scent_id: isProduct ? null : source.scent_id || null,
    image_id: source.image_id || null,
    active: source.active,
    sold_out: source.sold_out,
  }
}

async function save() {
  if (draft.kind === 'product' && (!draft.name.trim() || draft.price === '')) {
    error.value = 'A product needs a name and a price.'
    return
  }
  if (draft.kind === 'scent' && !draft.scent_id) {
    error.value = 'Choose a scent to list.'
    return
  }
  let sizePrices = null
  if (draft.kind === 'scent') {
    sizePrices = {}
    for (const size of BOTTLE_SIZES) {
      const raw = String(draft.prices[size.value] ?? '').trim()
      const n = Number(raw)
      if (raw !== '' && (!Number.isFinite(n) || n < 0)) {
        error.value = `The ${size.label} price isn't a valid amount.`
        return
      }
      sizePrices[size.value] = raw === '' || n === 0 ? null : n
    }
    if (!Object.values(sizePrices).some((v) => v !== null)) {
      error.value = 'Enter a price for at least one size, or the shop has nothing to sell.'
      return
    }
  }
  busy.value = 'save'
  error.value = ''
  try {
    // Prices first: they live on the scent, and the listing is pointless
    // without them. Sending every size sets them all (a blank clears one).
    if (sizePrices) {
      const updated = await api.updateScent(draft.scent_id, { prices: sizePrices })
      emit('scent-updated', updated)
    }
    const body = bodyFrom(draft)
    if (editing.value) {
      await api.updateShopProduct(editing.value, body)
      say('Saved changes.')
    } else {
      await api.createShopProduct(body)
      say(body.active ? 'Added to the shop.' : 'Added, hidden from the shop.')
    }
    reset()
    await reload()
  } catch (err) {
    error.value = err.message
  } finally {
    busy.value = ''
  }
}

/* --- Row actions -------------------------------------------------------- */

async function patch(p, changes, message) {
  busy.value = p.id
  error.value = ''
  try {
    await api.updateShopProduct(p.id, { ...bodyFrom({ ...p, price: p.price ?? '' }), ...changes })
    say(message)
    await reload()
  } catch (err) {
    error.value = err.message
  } finally {
    busy.value = ''
  }
}

const toggleActive = (p) =>
  patch(p, { active: !p.active }, p.active ? `${p.display_name} is hidden from the shop.` : `${p.display_name} is in the shop.`)

const toggleSoldOut = (p) =>
  patch(p, { sold_out: !p.sold_out }, p.sold_out ? `${p.display_name} is back in stock.` : `${p.display_name} is marked sold out.`)

async function remove(p) {
  if (!window.confirm(`Remove “${p.display_name}” from the shop?`)) return
  busy.value = p.id
  error.value = ''
  try {
    const { archived } = await api.removeShopProduct(p.id)
    say(
      archived
        ? `Removed ${p.display_name}. It has been sold before, so it is archived rather than deleted — those sales keep their record.`
        : `Removed ${p.display_name}.`,
    )
    if (editing.value === p.id) reset()
    await reload()
  } catch (err) {
    error.value = err.message
  } finally {
    busy.value = ''
  }
}

async function move(index, delta) {
  const target = index + delta
  if (target < 0 || target >= products.value.length) return
  const list = [...products.value]
  ;[list[index], list[target]] = [list[target], list[index]]
  // Move it on screen first; put it back if the save fails.
  const before = products.value
  products.value = list
  try {
    await api.reorderShopProducts(list.map((p) => p.id))
  } catch (err) {
    products.value = before
    error.value = err.message
  }
}
</script>

<template>
  <div class="card">
    <h2>Shipping</h2>
    <p class="muted">
      Shop orders are shipped. Square's checkout page asks the customer for their
      address, and the address appears on the order in the Square dashboard.
    </p>
    <div class="row">
      <div class="field grow">
        <label for="shop-ship-fee">Shipping charge per order ($)</label>
        <input id="shop-ship-fee" v-model="shipping.fee" type="number" inputmode="decimal" min="0" step="0.01" placeholder="0.00" />
      </div>
      <div class="field grow">
        <label for="shop-ship-free">Free shipping on orders of ($)</label>
        <input id="shop-ship-free" v-model="shipping.freeOver" type="number" inputmode="decimal" min="0" step="0.01" placeholder="Never" />
      </div>
    </div>
    <p class="muted field-help">
      0 ships free. Leave the second box blank to always charge shipping; otherwise
      orders at or above that amount (before any coupon) ship free.
    </p>
    <button class="ghost" type="button" :disabled="busy === 'shipping'" @click="saveShipping">
      {{ busy === 'shipping' ? 'Saving…' : 'Save shipping' }}
    </button>
  </div>

  <div class="card">
    <h2>In the shop</h2>
    <p class="muted">
      Listed in this order on the site. Hidden listings stay here but are not shown;
      sold-out ones are shown but can't be bought.
    </p>

    <p class="error" v-if="error" role="alert">{{ error }}</p>
    <p class="notice" v-else-if="notice" role="status">{{ notice }}</p>

    <p class="muted" v-if="loading">Loading…</p>
    <p class="muted" v-else-if="!products.length">
      Nothing in the shop yet — add the first listing below.
    </p>

    <ul class="shop-list" v-else>
      <li v-for="(p, i) in products" :key="p.id" class="shop-row" :class="{ inactive: !p.active }">
        <div class="shop-thumb">
          <img v-if="p.image_url" :src="p.image_url" alt="" loading="lazy" />
          <span v-else aria-hidden="true">No photo</span>
        </div>
        <div class="grow shop-info">
          <strong>{{ p.display_name }}</strong>
          <span class="muted">
            {{ p.kind === 'scent' ? 'Scent' : 'Product' }} · {{ priceText(p) }}
            <template v-if="p.units_sold"> · {{ p.units_sold }} sold</template>
          </span>
          <span class="muted" v-if="sizesText(p)">{{ sizesText(p) }}</span>
          <span class="danger-text shop-problem" v-if="p.active && problem(p)">{{ problem(p) }}</span>
          <span class="shop-problem muted" v-else-if="singleSize(p)">
            Only one size has a price, so customers can't choose. Edit this
            listing to add prices for other sizes.
          </span>
          <span class="shop-badges">
            <span class="badge" :class="p.active ? 'ok-badge' : ''">{{ p.active ? 'Shown' : 'Hidden' }}</span>
            <span class="badge danger-badge" v-if="p.sold_out">Sold out</span>
          </span>
        </div>
        <div class="shop-actions">
          <button class="icon" type="button" :disabled="i === 0" :aria-label="`Move ${p.display_name} up`" @click="move(i, -1)">↑</button>
          <button class="icon" type="button" :disabled="i === products.length - 1" :aria-label="`Move ${p.display_name} down`" @click="move(i, 1)">↓</button>
          <button class="ghost" type="button" :disabled="busy === p.id" @click="edit(p)">Edit</button>
          <button class="ghost" type="button" :disabled="busy === p.id" @click="toggleActive(p)">
            {{ p.active ? 'Hide' : 'Show' }}
          </button>
          <button class="ghost" type="button" :disabled="busy === p.id" @click="toggleSoldOut(p)">
            {{ p.sold_out ? 'In stock' : 'Sold out' }}
          </button>
          <button class="ghost danger-text" type="button" :disabled="busy === p.id" @click="remove(p)">
            Remove
          </button>
        </div>
      </li>
    </ul>
  </div>

  <div class="card" id="shop-form">
    <h2>{{ editing ? 'Edit listing' : 'Add to the shop' }}</h2>

    <fieldset class="shop-kind">
      <legend class="muted">What is it?</legend>
      <label class="checkbox">
        <input type="radio" value="product" v-model="draft.kind" />
        A product with its own price
      </label>
      <label class="checkbox">
        <input type="radio" value="scent" v-model="draft.kind" />
        A scent from the Catalog
      </label>
    </fieldset>

    <template v-if="draft.kind === 'product'">
      <div class="row">
        <div class="field grow">
          <label for="shop-name">Name</label>
          <input id="shop-name" v-model="draft.name" type="text" maxlength="120" placeholder="e.g. Discovery Set" />
        </div>
        <div class="field" style="flex: none; width: 8rem">
          <label for="shop-price">Price ($)</label>
          <input id="shop-price" v-model="draft.price" type="number" inputmode="decimal" min="0" step="0.01" />
        </div>
      </div>
    </template>

    <template v-else>
      <div class="row">
        <div class="field grow">
          <label for="shop-scent">Scent</label>
          <select id="shop-scent" v-model="draft.scent_id">
            <option value="">Choose a scent…</option>
            <option v-for="s in listableScents" :key="s.id" :value="s.id">
              {{ s.name }}{{ s.active ? '' : ' (inactive)' }}
            </option>
          </select>
        </div>
        <div class="field grow">
          <label for="shop-scent-name">Shop name (optional)</label>
          <input id="shop-scent-name" v-model="draft.name" type="text" maxlength="120" placeholder="Uses the scent's name" />
        </div>
      </div>
      <template v-if="draft.scent_id">
        <label class="muted" style="display: block; margin: 0.4rem 0">Price per size ($)</label>
        <div class="row shop-size-prices">
          <div class="field" v-for="size in BOTTLE_SIZES" :key="size.value">
            <label :for="`shop-size-${size.value}`">{{ size.label }}</label>
            <input
              :id="`shop-size-${size.value}`"
              v-model="draft.prices[size.value]"
              type="number"
              inputmode="decimal"
              min="0"
              step="0.01"
              placeholder="Not sold"
            />
          </div>
        </div>
        <p class="muted field-help">
          Customers choose from every size with a price; leave a size blank to not
          sell it. These are the scent's own prices, so they also change in
          Catalog and at intake.
        </p>
      </template>
    </template>

    <div class="field">
      <label for="shop-desc">Description (optional)</label>
      <textarea id="shop-desc" v-model="draft.description" rows="3" maxlength="2000" placeholder="Shown under the name in the shop"></textarea>
    </div>

    <div class="field">
      <label for="shop-photo">Photo</label>
      <div class="shop-photo">
        <div class="shop-thumb large">
          <img v-if="draft.image_url" :src="draft.image_url" alt="Selected photo" />
          <span v-else aria-hidden="true">No photo</span>
        </div>
        <div>
          <input id="shop-photo" ref="fileInput" type="file" accept="image/jpeg,image/png,image/webp" :disabled="uploading" @change="pickImage" />
          <p class="muted field-help">
            {{ uploading ? 'Uploading…' : 'JPEG, PNG or WebP, up to 5MB. A square photo looks best.' }}
          </p>
          <button v-if="draft.image_url" class="ghost" type="button" @click="clearImage">Remove photo</button>
        </div>
      </div>
    </div>

    <label class="checkbox">
      <input type="checkbox" v-model="draft.active" />
      Show in the shop
    </label>
    <label class="checkbox">
      <input type="checkbox" v-model="draft.sold_out" />
      Sold out
    </label>

    <div class="row" style="gap: 0.5rem; margin-top: 0.8rem">
      <button class="ghost" type="button" style="flex: none" :disabled="busy === 'save' || uploading" @click="save">
        {{ busy === 'save' ? 'Saving…' : editing ? 'Save changes' : 'Add to shop' }}
      </button>
      <button v-if="editing" class="ghost" type="button" style="flex: none" @click="reset">Cancel</button>
    </div>
  </div>
</template>

<style scoped>
.shop-list {
  list-style: none;
  margin: 0;
  padding: 0;
}

.shop-row {
  display: flex;
  gap: 0.85rem;
  align-items: center;
  padding: 0.7rem;
  margin-bottom: 0.5rem;
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  flex-wrap: wrap;
}

.shop-row.inactive .shop-thumb,
.shop-row.inactive .shop-info strong {
  opacity: 0.55;
}

.shop-thumb {
  flex: none;
  width: 56px;
  height: 56px;
  border-radius: var(--radius);
  background: var(--surface-alt);
  overflow: hidden;
  display: grid;
  place-items: center;
  font-size: 0.65rem;
  color: var(--muted);
  text-align: center;
}

.shop-thumb.large {
  width: 96px;
  height: 96px;
}

.shop-thumb img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.shop-info {
  display: flex;
  flex-direction: column;
  gap: 0.15rem;
  min-width: 12rem;
}

.shop-problem {
  font-size: 0.85rem;
}

.shop-badges {
  display: flex;
  gap: 0.35rem;
  margin-top: 0.2rem;
}

.shop-actions {
  display: flex;
  gap: 0.35rem;
  flex-wrap: wrap;
  align-items: center;
}

.shop-kind {
  border: 0;
  padding: 0;
  margin: 0 0 0.9rem;
  display: flex;
  gap: 1.2rem;
  flex-wrap: wrap;
}

.shop-kind legend {
  padding: 0;
  margin-bottom: 0.4rem;
}

.shop-size-prices {
  flex-wrap: wrap;
}

.shop-size-prices .field {
  flex: 1 1 7rem;
}

.shop-photo {
  display: flex;
  gap: 1rem;
  align-items: flex-start;
  flex-wrap: wrap;
}

/* The global textarea is monospace for pasted keys and formulas; a product
   description is prose. */
textarea {
  min-height: 5rem;
  font-family: inherit;
  font-size: inherit;
  line-height: 1.5;
}
</style>
