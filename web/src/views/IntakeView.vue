<script setup>
import { computed, onMounted, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'

import MixBuilder from '../components/MixBuilder.vue'
import { api } from '../lib/api.js'
import { BOTTLE_SIZES, ORDER_STATUSES, ORDER_TYPES, bottleLabel, formatMl } from '../lib/bottle.js'

const route = useRoute()
const router = useRouter()

const ingredients = ref([])
const scents = ref([])

const email = ref('')
const name = ref('')
const marketingConsent = ref(false)
const scentPreferenceIds = ref([])

const orderType = ref('custom_mix')
const size = ref('oz3_4')
const status = ref('paid')
const amount = ref('')
const scentId = ref('')
const mixName = ref('')
const items = ref([])

const loading = ref(true)
const busy = ref(false)
const error = ref('')
const result = ref(null)

/// Held steady across retries so a resubmitted attempt cannot double-charge a
/// customer; regenerated only when a fresh intake is started.
const idempotencyKey = ref(newKey())

function newKey() {
  return crypto.randomUUID ? crypto.randomUUID() : `${Date.now()}-${Math.random().toString(16).slice(2)}`
}

const activeScents = computed(() => scents.value.filter((s) => s.active))

function ingredientName(id) {
  return ingredients.value.find((i) => i.id === id)?.name ?? 'Unknown'
}

// The house formula for the currently selected set-perfume scent, shown so the
// operator can see what's actually in it — the set-perfume analogue of the mix.
const selectedScentFormula = computed(() => {
  const scent = scents.value.find((s) => s.id === scentId.value)
  if (!scent || !scent.items?.length) return ''
  return scent.items.map((i) => `${ingredientName(i.ingredient_id)} ${formatMl(i.amount_ml)}ml`).join(' · ')
})

const canSubmit = computed(() => {
  if (busy.value) return false
  if (!email.value.includes('@')) return false
  if (orderType.value === 'set_perfume') return Boolean(scentId.value)
  return items.value.length > 0 && items.value.every((i) => Number(i.amount_ml) > 0)
})

function toggleScentPreference(id) {
  const next = new Set(scentPreferenceIds.value)
  next.has(id) ? next.delete(id) : next.add(id)
  scentPreferenceIds.value = [...next]
}

onMounted(async () => {
  try {
    const [ing, sc] = await Promise.all([api.listIngredients(), api.listScents()])
    ingredients.value = ing
    scents.value = sc
    await prefillFromQuery()
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
    orderType.value = 'custom_mix'
    mixName.value = detail.name || ''
    // Drop ingredients that have since been deactivated — the API would
    // reject the whole mix otherwise, with nothing pointing at the culprit.
    const activeIds = new Set(ingredients.value.filter((i) => i.active).map((i) => i.id))
    const usable = detail.items.filter((i) => activeIds.has(i.ingredient_id))
    if (usable.length !== detail.items.length) {
      error.value = 'Some ingredients in this mix are no longer active and were left out.'
    }
    items.value = usable.map((i) => ({ ingredient_id: i.ingredient_id, amount_ml: Number(i.amount_ml) }))
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
      order: {
        type: orderType.value,
        size: size.value,
        status: status.value,
        scent_id: orderType.value === 'set_perfume' ? scentId.value : null,
        mix:
          orderType.value === 'custom_mix'
            ? { name: mixName.value.trim() || null, items: items.value.map((i) => ({ ...i, amount_ml: Number(i.amount_ml) })) }
            : null,
        amount: amount.value === '' ? null : Number(amount.value),
      },
    }
    result.value = await api.submitIntake(payload, idempotencyKey.value)
  } catch (err) {
    error.value = err.message
    if (err.status === 401) router.push({ name: 'pair' })
  } finally {
    busy.value = false
  }
}

function startAnother() {
  email.value = ''
  name.value = ''
  marketingConsent.value = false
  scentPreferenceIds.value = []
  orderType.value = 'custom_mix'
  size.value = 'oz3_4'
  status.value = 'paid'
  amount.value = ''
  scentId.value = ''
  mixName.value = ''
  items.value = []
  result.value = null
  error.value = ''
  idempotencyKey.value = newKey()
  if (Object.keys(route.query).length) router.replace({ name: 'intake' })
}

function scentName(id) {
  return scents.value.find((s) => s.id === id)?.name ?? '—'
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
        <dt>Order</dt>
        <dd>
          {{ result.order.order_type === 'custom_mix' ? 'Custom mix' : scentName(result.order.scent_id) }}
          · {{ bottleLabel(result.order.size) }} · {{ result.order.status }}
        </dd>
        <dt v-if="result.order.amount">Amount</dt>
        <dd v-if="result.order.amount">${{ result.order.amount }}</dd>
        <template v-if="result.mix">
          <dt>Mix</dt>
          <dd>
            {{ result.mix.name || 'Unnamed' }} —
            {{ result.mix.items.length }} ingredient{{ result.mix.items.length === 1 ? '' : 's' }}
          </dd>
        </template>
      </dl>
    </div>
    <button class="primary" type="button" @click="startAnother">Start another intake</button>
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

    <div class="card">
      <h2>Order</h2>

      <div class="field">
        <label>Type</label>
        <div class="seg">
          <button
            v-for="option in ORDER_TYPES"
            :key="option.value"
            type="button"
            :aria-pressed="orderType === option.value"
            @click="orderType = option.value"
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
            :aria-pressed="size === option.value"
            @click="size = option.value"
          >
            {{ option.label }}
          </button>
        </div>
      </div>

      <div class="field">
        <label>Status</label>
        <div class="seg">
          <button
            v-for="option in ORDER_STATUSES"
            :key="option.value"
            type="button"
            :aria-pressed="status === option.value"
            @click="status = option.value"
          >
            {{ option.label }}
          </button>
        </div>
      </div>

      <div class="field">
        <label for="amount">Amount (optional)</label>
        <input id="amount" v-model="amount" type="number" inputmode="decimal" min="0" step="0.01" />
      </div>
    </div>

    <div class="card" v-if="orderType === 'set_perfume'">
      <h2>Scent</h2>
      <p class="muted" v-if="!activeScents.length">No active scents to choose from.</p>
      <div class="chips" v-else>
        <button
          v-for="scent in activeScents"
          :key="scent.id"
          type="button"
          :aria-pressed="scentId === scent.id"
          @click="scentId = scent.id"
        >
          {{ scent.name }}
        </button>
      </div>
      <p class="muted" v-if="scentId" style="margin-top: 0.6rem">
        {{ selectedScentFormula || 'No formula set for this scent yet.' }}
      </p>
    </div>

    <template v-else>
      <div class="card">
        <h2>Mix name</h2>
        <input v-model="mixName" type="text" placeholder="Optional" aria-label="Mix name" />
      </div>
      <MixBuilder v-model="items" :ingredients="ingredients" :size="size" />
    </template>

    <button class="primary" type="submit" :disabled="!canSubmit">
      {{ busy ? 'Saving…' : 'Submit intake' }}
    </button>
  </form>
</template>
