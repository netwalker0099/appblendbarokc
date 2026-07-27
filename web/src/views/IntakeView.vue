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
import { api } from '../lib/api.js'
import { BOTTLE_SIZES, ORDER_TYPES, bottleLabel, formatMl } from '../lib/bottle.js'

const route = useRoute()
const router = useRouter()

const ingredients = ref([])
const scents = ref([])
const bundles = ref([])

const email = ref('')
const name = ref('')
const marketingConsent = ref(false)
const scentPreferenceIds = ref([])

/** Each line is one blend in one size, with a quantity. */
const lines = ref([])
const bundleId = ref('')

const loading = ref(true)
const busy = ref(false)
const error = ref('')
const result = ref(null)

/// Held steady across retries so a resubmitted attempt cannot record the same
/// intake twice; regenerated only when a fresh intake is started.
const idempotencyKey = ref(newKey())

function newKey() {
  return crypto.randomUUID ? crypto.randomUUID() : `${Date.now()}-${Math.random().toString(16).slice(2)}`
}

const activeScents = computed(() => scents.value.filter((s) => s.active))
const activeBundles = computed(() => bundles.value.filter((b) => b.active))

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

/** House formula for a chosen set-perfume, so the operator can see what's in it. */
function scentFormula(id) {
  const scent = scents.value.find((s) => s.id === id)
  if (!scent || !scent.items?.length) return ''
  return scent.items.map((i) => `${ingredientName(i.ingredient_id)} ${formatMl(i.amount_ml)}ml`).join(' · ')
}

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
  if (line.type === 'set_perfume') return Boolean(line.scent_id)
  // A blend nobody named is one nobody can find again.
  if (!line.mixName.trim()) return false
  return line.items.length > 0 && line.items.every((i) => Number(i.amount_ml) > 0)
}

const canSubmit = computed(() => {
  if (busy.value) return false
  if (!email.value.includes('@')) return false
  if (!lines.value.length && !bundleId.value) return false
  return lines.value.every(lineIsValid)
})

function toggleScentPreference(id) {
  const next = new Set(scentPreferenceIds.value)
  next.has(id) ? next.delete(id) : next.add(id)
  scentPreferenceIds.value = [...next]
}

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
      scent_preference_ids: scentPreferenceIds.value.length ? scentPreferenceIds.value : null,
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
  } catch (err) {
    error.value = err.message
    if (err.status === 401) router.push({ name: 'login' })
  } finally {
    busy.value = false
  }
}

function startAnother() {
  email.value = ''
  name.value = ''
  marketingConsent.value = false
  scentPreferenceIds.value = []
  lines.value = [newLine()]
  bundleId.value = ''
  result.value = null
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

      <p class="muted" style="margin-top: 0.9rem">
        Nothing has been charged yet. Take payment on the
        <RouterLink :to="{ name: 'checkout', query: { customer: result.customer.id } }">
          Checkout
        </RouterLink>
        screen.
      </p>
    </div>
    <div class="row" style="gap: 0.5rem">
      <button class="primary" type="button" @click="startAnother">Start another intake</button>
      <RouterLink
        class="ghost"
        :to="{ name: 'checkout', query: { customer: result.customer.id } }"
      >
        Take payment
      </RouterLink>
    </div>
  </template>

  <form v-else @submit.prevent="submit">
    <p class="error" v-if="error">{{ error }}</p>

    <div class="card">
      <h2>Customer</h2>
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
      <div class="field">
        <label for="name">Name</label>
        <input id="name" v-model="name" type="text" autocomplete="off" />
      </div>
      <label class="checkbox">
        <input type="checkbox" v-model="marketingConsent" />
        Marketing consent
      </label>
    </div>

    <div class="card" v-if="activeScents.length">
      <h2>Scents they liked</h2>
      <div class="chips">
        <button
          v-for="scent in activeScents"
          :key="scent.id"
          type="button"
          :aria-pressed="scentPreferenceIds.includes(scent.id)"
          @click="toggleScentPreference(scent.id)"
        >
          {{ scent.name }}
        </button>
      </div>
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
              @click="line.scent_id = scent.id"
            >
              {{ scent.name }}
            </button>
          </div>
          <p class="muted" v-if="scentFormula(line.scent_id)" style="margin-top: 0.5rem">
            {{ scentFormula(line.scent_id) }}
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
      Saving records what was made. Payment is taken separately on the Checkout screen.
    </p>
  </form>
</template>
