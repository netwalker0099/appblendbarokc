<script setup>
/**
 * Stand intake.
 *
 * One submission, several items: a customer taking a 3.4oz and a roller is one
 * intake with two lines, each with its own quantity.
 *
 * There is no status to choose. Intake records what was made; nothing is owed
 * until the order goes into a cart and that cart is checked out on the Checkout
 * screen. Pricing comes from the catalogue — the amount box is an override, not
 * a requirement.
 */
import { computed, onMounted, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'

import MixBuilder from '../components/MixBuilder.vue'
import PayPanel from '../components/PayPanel.vue'
import { api } from '../lib/api.js'
import { BOTTLE_SIZES, bottleLabel, bottleMl, formatMl, formulaCheck, fromBaseMl, ORDER_TYPES } from '../lib/bottle.js'

const route = useRoute()
const router = useRouter()

const ingredients = ref([])
const scents = ref([])
const bundles = ref([])

const email = ref('')
const name = ref('')
const marketingConsent = ref(false)

/** Each line is one blend in one size, with a quantity. */
const lines = ref([])
const bundleId = ref('')

const loading = ref(true)
const busy = ref(false)
const error = ref('')
const result = ref(null)
// The cart Save intake builds for this sale, ready to charge. Null if it
// couldn't be built (cartError says why) — the intake itself is still saved.
const cartId = ref(null)
const cartError = ref('')
const paid = ref(false)

/// Held steady across retries so a resubmitted attempt cannot record the same
/// intake twice; regenerated only when a fresh intake is started.
const idempotencyKey = ref(newKey())

function newKey() {
  return crypto.randomUUID ? crypto.randomUUID() : `${Date.now()}-${Math.random().toString(16).slice(2)}`
}

const activeScents = computed(() => scents.value.filter((s) => s.active))
const activeBundles = computed(() => bundles.value.filter((b) => b.active))
const selectedBundle = computed(() => bundles.value.find((b) => b.id === bundleId.value) || null)

/** What a package contains, as staff need to read it off the screen. */
function describeBundleItem(item) {
  const what =
    item.type === 'custom_mix'
      ? 'Custom blend'
      : scents.value.find((s) => s.id === item.scent_id)?.name || 'Scent'
  const qty = item.quantity > 1 ? `${item.quantity} × ` : ''
  return `${qty}${what}`
}

function ingredientName(id) {
  return ingredients.value.find((i) => i.id === id)?.name ?? 'Unknown'
}

function scentName(id) {
  return scents.value.find((s) => s.id === id)?.name ?? '—'
}

function newLine(overrides = {}) {
  return {
    type: 'custom_mix',
    size: 'oz3_4',
    quantity: 1,
    scent_id: '',
    mixName: '',
    items: [],
    amount: '',
    ...overrides,
  }
}

function addLine() {
  lines.value.push(newLine())
}

function removeLine(i) {
  lines.value.splice(i, 1)
}

/** House formula for a chosen set-perfume, as poured into the line's bottle. */
function scentFormula(id, size) {
  const scent = scents.value.find((s) => s.id === id)
  if (!scent || !scent.items?.length) return ''
  const parts = scent.items.map(
    (i) => `${ingredientName(i.ingredient_id)} ${formatMl(fromBaseMl(i.amount_ml, size))} ml`,
  )
  return `${parts.join(' · ')} = ${formatMl(bottleMl(size))} ml for the ${bottleLabel(size)}`
}

/**
 * A house scent can only be sold once its formula totals 100 ml at the base,
 * so every bottle pours exactly (50 / 30 / 10 ml). The API refuses it
 * otherwise; this greys it out first.
 */
function scentReady(id) {
  const scent = scents.value.find((s) => s.id === id)
  return Boolean(scent?.items?.length) && formulaCheck(scent.items).ok
}

/** House scents in the chosen package that can't be sold yet, by name. */
const bundleBlockers = computed(() => {
  if (!selectedBundle.value) return []
  return selectedBundle.value.items
    .filter((item) => item.type === 'set_perfume' && !scentReady(item.scent_id))
    .map((item) => scents.value.find((s) => s.id === item.scent_id)?.name || 'A scent')
})

/** Catalogue price for a line, shown so staff know what will be charged. */
function catalogPrice(line) {
  if (line.type === 'set_perfume') {
    const scent = scents.value.find((s) => s.id === line.scent_id)
    return scent ? scent[`price_${line.size}`] : null
  }
  return settings.value ? settings.value[`custom_price_${line.size}`] : null
}
const settings = ref(null)

function lineIsValid(line) {
  if (!(Number(line.quantity) >= 1)) return false
  if (line.type === 'set_perfume') return Boolean(line.scent_id) && scentReady(line.scent_id)
  // A blend nobody named is one nobody can find again.
  if (!line.mixName.trim()) return false
  // Must fill the chosen bottle exactly (100 / 50 / 30 / 10 ml); MixBuilder
  // shows how far off it is.
  return formulaCheck(line.items, line.size).ok
}

const canSubmit = computed(() => {
  if (busy.value) return false
  if (!email.value.includes('@')) return false
  if (!lines.value.length && !bundleId.value) return false
  if (bundleBlockers.value.length) return false
  return lines.value.every(lineIsValid)
})

onMounted(async () => {
  try {
    const [ing, sc, bd, set] = await Promise.all([
      api.listIngredients(),
      api.listScents(),
      api.listBundles(),
      api.getSettings().catch(() => null), // workers can't read settings; prices just won't preview
    ])
    ingredients.value = ing
    scents.value = sc
    bundles.value = bd
    settings.value = set
    await prefillFromQuery()
    if (!lines.value.length) addLine()
  } catch (err) {
    error.value = err.message
  } finally {
    loading.value = false
  }
})

/// Reorder path: /intake?mix=<id>&customer=<id> arrives from the lookup view.
async function prefillFromQuery() {
  const { mix: mixId, customer: customerId } = route.query
  if (!mixId && !customerId) return

  if (customerId) {
    const customer = await api.getCustomer(customerId)
    email.value = customer.email
    name.value = customer.name || ''
    marketingConsent.value = customer.marketing_consent
  }

  if (mixId) {
    const detail = await api.getMix(mixId)
    // Drop ingredients that have since been deactivated — the API would reject
    // the whole mix otherwise, with nothing pointing at the culprit.
    const activeIds = new Set(ingredients.value.filter((i) => i.active).map((i) => i.id))
    const usable = detail.items.filter((i) => activeIds.has(i.ingredient_id))
    if (usable.length !== detail.items.length) {
      error.value = 'Some ingredients in this blend are no longer active and were left out.'
    }
    lines.value = [
      newLine({
        type: 'custom_mix',
        mixName: detail.name || '',
        items: usable.map((i) => ({ ingredient_id: i.ingredient_id, amount_ml: Number(i.amount_ml) })),
      }),
    ]
  }
}

async function submit() {
  busy.value = true
  error.value = ''
  try {
    const payload = {
      email: email.value.trim(),
      name: name.value.trim() || null,
      marketing_consent: marketingConsent.value,
      bundle_id: bundleId.value || null,
      items: lines.value.map((line) => ({
        type: line.type,
        size: line.size,
        quantity: Number(line.quantity) || 1,
        scent_id: line.type === 'set_perfume' ? line.scent_id : null,
        mix:
          line.type === 'custom_mix'
            ? {
                name: line.mixName.trim(),
                items: line.items.map((i) => ({ ...i, amount_ml: Number(i.amount_ml) })),
              }
            : null,
        amount: line.amount === '' ? null : Number(line.amount),
      })),
    }
    result.value = await api.submitIntake(payload, idempotencyKey.value)
    await buildCart()
  } catch (err) {
    error.value = err.message
    if (err.status === 401) router.push({ name: 'login' })
  } finally {
    busy.value = false
  }
}

/// Put everything just saved on a cart, so it's ready to charge without a trip
/// to the Checkout screen. Failing here doesn't undo the intake — the orders
/// are saved and can still be sold from Checkout.
async function buildCart() {
  cartId.value = null
  cartError.value = ''
  paid.value = false
  try {
    const cart = await api.createCart({
      customer_id: result.value.customer.id,
      order_ids: result.value.orders.map((o) => o.id),
    })
    cartId.value = cart.id
  } catch (err) {
    cartError.value = err.message
  }
}

function startAnother() {
  email.value = ''
  name.value = ''
  marketingConsent.value = false
  lines.value = [newLine()]
  bundleId.value = ''
  result.value = null
  cartId.value = null
  cartError.value = ''
  paid.value = false
  error.value = ''
  idempotencyKey.value = newKey()
  if (Object.keys(route.query).length) router.replace({ name: 'intake' })
}
</script>

<template>
  <p class="muted" v-if="loading">Loading…</p>

  <template v-else-if="result">
    <div class="card success">
      <h2>Intake saved</h2>
      <dl class="summary">
        <dt>Customer</dt>
        <dd>{{ result.customer.name || result.customer.email }}</dd>
        <dt>Email</dt>
        <dd>{{ result.customer.email }}</dd>
      </dl>

      <h3 style="margin-top: 1rem">
        {{ result.orders.length }} item{{ result.orders.length === 1 ? '' : 's' }}
      </h3>
      <div v-for="r in result.orders" :key="r.id" class="list-item" style="cursor: default">
        <span class="grow">
          <strong>
            <template v-if="r.quantity > 1">{{ r.quantity }} × </template>
            {{ r.order_type === 'custom_mix' ? (r.mix?.name || 'Custom blend') : scentName(r.scent_id) }}
          </strong>
          <span class="muted">{{ bottleLabel(r.size) }}</span>
        </span>
        <span class="badge" v-if="r.amount">${{ r.amount }}</span>
        <span class="badge danger-badge" v-else>No price</span>
      </div>

    </div>

    <PayPanel
      v-if="cartId"
      :key="cartId"
      :cart-id="cartId"
      @paid="paid = true"
      @canceled="cartId = null"
    />
    <div class="card" v-else>
      <p class="danger-text" v-if="cartError">Couldn't set up payment: {{ cartError }}</p>
      <p class="muted" style="margin: 0">
        Nothing has been charged. Take payment from the
        <RouterLink :to="{ name: 'checkout', query: { customer: result.customer.id } }">
          Checkout
        </RouterLink>
        screen when ready.
      </p>
    </div>

    <div class="row" style="gap: 0.5rem">
      <button :class="paid || !cartId ? 'primary' : 'ghost'" type="button" @click="startAnother">
        {{ paid ? 'Next customer' : 'Start another intake' }}
      </button>
    </div>
  </template>

  <form v-else @submit.prevent="submit">
    <p class="error" v-if="error">{{ error }}</p>

    <div class="card">
      <h2>Customer</h2>
      <div class="field">
        <label for="name">Name</label>
        <input id="name" v-model="name" type="text" autocomplete="off" />
      </div>
      <div class="field">
        <label for="email">Email</label>
        <input
          id="email"
          v-model="email"
          type="email"
          inputmode="email"
          autocapitalize="none"
          autocomplete="off"
          spellcheck="false"
          required
        />
      </div>
      <label class="checkbox">
        <input type="checkbox" v-model="marketingConsent" />
        Marketing consent
      </label>
    </div>

    <div class="card" v-if="activeBundles.length">
      <h2>Package deal</h2>
      <p class="muted">
        Optional. The package's bottles are added on top of the items below, priced
        to its package total.
      </p>
      <div class="chips">
        <button
          type="button"
          :aria-pressed="bundleId === ''"
          @click="bundleId = ''"
        >
          None
        </button>
        <button
          v-for="b in activeBundles"
          :key="b.id"
          type="button"
          :aria-pressed="bundleId === b.id"
          @click="bundleId = b.id"
        >
          {{ b.name }} — ${{ b.price }}
        </button>
      </div>

      <!-- Contents shown read-only once a package is chosen. The sizes are part
           of the package definition, so they are deliberately not editable here:
           changing one would mean the customer is no longer buying the package
           that was priced. -->
      <div v-if="selectedBundle" class="bundle-locked">
        <p class="muted" style="margin: 0 0 0.5rem">
          Included in <strong>{{ selectedBundle.name }}</strong> — sizes are fixed
          by the package.
        </p>
        <div v-for="item in selectedBundle.items" :key="item.id" class="locked-row">
          <span class="grow">{{ describeBundleItem(item) }}</span>
          <span class="badge">{{ bottleLabel(item.size) }}</span>
          <span class="lock" aria-label="fixed by the package" title="Fixed by the package">
            &#128274;
          </span>
        </div>
        <p class="muted" style="margin: 0.5rem 0 0; font-size: 0.85rem">
          To sell a different size, add it as its own item below instead.
        </p>
        <p class="danger-text" v-if="bundleBlockers.length" role="alert" style="margin: 0.5rem 0 0">
          {{ bundleBlockers.join(', ') }} {{ bundleBlockers.length > 1 ? 'have' : 'has' }} no
          complete formula yet, so this package can’t be sold. Add one totalling 100 ml in
          Admin → Catalog.
        </p>
      </div>
    </div>

    <div class="card" v-for="(line, i) in lines" :key="i">
      <div class="row" style="align-items: baseline">
        <h2 class="grow">Item {{ i + 1 }}</h2>
        <button
          v-if="lines.length > 1"
          class="icon"
          type="button"
          aria-label="Remove item"
          @click="removeLine(i)"
        >
          ✕
        </button>
      </div>

      <div class="field">
        <label>Type</label>
        <div class="seg">
          <button
            v-for="option in ORDER_TYPES"
            :key="option.value"
            type="button"
            :aria-pressed="line.type === option.value"
            @click="line.type = option.value"
          >
            {{ option.label }}
          </button>
        </div>
      </div>

      <div class="field">
        <label>Bottle size</label>
        <div class="seg">
          <button
            v-for="option in BOTTLE_SIZES"
            :key="option.value"
            type="button"
            :aria-pressed="line.size === option.value"
            @click="line.size = option.value"
          >
            {{ option.label }}
          </button>
        </div>
      </div>

      <div class="row">
        <div class="field" style="flex: none; width: 7rem">
          <label>Quantity</label>
          <input v-model="line.quantity" type="number" inputmode="numeric" min="1" step="1" />
        </div>
        <div class="field grow">
          <label>Price override (optional)</label>
          <input v-model="line.amount" type="number" inputmode="decimal" min="0" step="0.01" />
        </div>
      </div>
      <!-- Below the row, not inside the field: help text in a flex-end row
           extends that column and pushes the quantity input out of line. -->
      <p class="muted field-help">
        <template v-if="line.amount === '' && catalogPrice(line)">
          Catalogue price ${{ catalogPrice(line) }} each.
        </template>
        <template v-else-if="line.amount === ''">
          No catalogue price set for this size — add one in Admin, or type a price.
        </template>
        <template v-else>Overriding the catalogue price.</template>
      </p>

      <template v-if="line.type === 'set_perfume'">
        <div class="field">
          <label>Scent</label>
          <p class="muted" v-if="!activeScents.length">No active scents to choose from.</p>
          <div class="chips" v-else>
            <button
              v-for="scent in activeScents"
              :key="scent.id"
              type="button"
              :aria-pressed="line.scent_id === scent.id"
              :disabled="!scentReady(scent.id)"
              :title="scentReady(scent.id) ? '' : 'No complete formula yet — add one in Admin → Catalog'"
              @click="line.scent_id = scent.id"
            >
              {{ scent.name }}{{ scentReady(scent.id) ? '' : ' — no formula' }}
            </button>
          </div>
          <p class="muted" v-if="activeScents.some((s) => !scentReady(s.id))" style="margin-top: 0.4rem; font-size: 0.85rem">
            Scents marked “no formula” can’t be sold here until their formula (totalling
            100 ml) is added in Admin → Catalog — staff need it to pour the bottle.
          </p>
          <p class="muted" v-if="scentFormula(line.scent_id, line.size)" style="margin-top: 0.5rem">
            {{ scentFormula(line.scent_id, line.size) }}
          </p>
        </div>
      </template>

      <template v-else>
        <div class="field">
          <label :for="`mixname-${i}`">Blend name</label>
          <input
            :id="`mixname-${i}`"
            v-model="line.mixName"
            type="text"
            autocomplete="off"
            required
            placeholder="e.g. Amber Evening"
          />
          <p class="muted" style="margin: 0.3rem 0 0; font-size: 0.85rem">
            Required — this is how the customer finds it again to reorder.
          </p>
        </div>
        <MixBuilder v-model="line.items" :ingredients="ingredients" :size="line.size" />
      </template>
    </div>

    <button class="ghost" type="button" @click="addLine">+ Add another item</button>

    <button class="primary" type="submit" :disabled="!canSubmit" style="margin-top: 0.8rem">
      {{ busy ? 'Saving…' : 'Save intake' }}
    </button>
    <p class="muted" style="margin-top: 0.5rem; font-size: 0.88rem">
      Saving records what was made and sets the sale up to charge — nothing is charged until you press Charge.
    </p>
  </form>
</template>

<style scoped>
.bundle-locked {
  margin-top: 0.9rem;
  padding: 0.8rem;
  background: var(--surface-alt);
  border-radius: var(--radius);
}

.locked-row {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  padding: 0.45rem 0;
  border-bottom: 1px solid var(--border);
}

.locked-row:last-of-type {
  border-bottom: 0;
}

.locked-row .grow {
  flex: 1;
  min-width: 0;
}

/* Not a disabled control — there is no control. The package defines these, and
   showing a greyed-out dropdown would imply it could be changed. */
.lock {
  flex: none;
  opacity: 0.55;
  font-size: 0.9rem;
}
</style>
