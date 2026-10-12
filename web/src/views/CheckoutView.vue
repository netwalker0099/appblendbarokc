<script setup>
/**
 * Checkout: build a cart for a customer's unsold blends (plus any deposits or
 * fees) and take payment.
 *
 * Payment itself is PayPanel: on a paired Square Terminal by default, or a
 * Square payment link / QR. Either way the operator never touches a card, and
 * no card details pass through this device or this app.
 */
import { computed, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'

import PayPanel from '../components/PayPanel.vue'
import { api } from '../lib/api.js'
import { bottleLabel } from '../lib/bottle.js'

const route = useRoute()
const router = useRouter()

const query = ref('')
const customers = ref([])
const customer = ref(null)

const orders = ref([])
const selectedOrderIds = ref(new Set())
const extras = ref([])

const cart = ref(null)
const squareLive = ref(true)

const busy = ref('')
const error = ref('')
const notice = ref('')

onMounted(async () => {
  try {
    const status = await api.getSquareStatus()
    squareLive.value = status.live
  } catch {
    // Status is advisory; a failure here must not block taking a payment.
  }
  const preselect = route.query.customer
  if (preselect) {
    try {
      customer.value = await api.getCustomer(preselect)
      await loadOrders()
    } catch (err) {
      error.value = err.message
    }
  } else {
    await search()
  }
})

async function search() {
  busy.value = 'search'
  error.value = ''
  try {
    customers.value = await api.listCustomers(query.value.trim())
  } catch (err) {
    error.value = err.message
  } finally {
    busy.value = ''
  }
}

async function pick(c) {
  customer.value = c
  await loadOrders()
}

async function loadOrders() {
  busy.value = 'orders'
  error.value = ''
  selectedOrderIds.value = new Set()
  try {
    // `uncarted` hides anything already on a cart, so a blend can't be sold twice.
    orders.value = await api.listOrders(customer.value.id, { uncarted: true })
    // Pre-tick everything still unpaid — the common case is "sell what we just made".
    for (const o of orders.value) {
      if (o.status === 'lead' && o.amount) selectedOrderIds.value.add(o.id)
    }
  } catch (err) {
    error.value = err.message
  } finally {
    busy.value = ''
  }
}

function toggle(order) {
  const next = new Set(selectedOrderIds.value)
  next.has(order.id) ? next.delete(order.id) : next.add(order.id)
  selectedOrderIds.value = next
}

function addExtra() {
  extras.value.push({ name: '', unit_amount: '', quantity: 1, kind: 'other' })
}

function removeExtra(index) {
  extras.value.splice(index, 1)
}

/**
 * Common non-bottle lines, straight from the published booking terms.
 *
 * `kind` is what matters here, not the label: a paid `event_deposit` is what
 * marks an event as booked and fires the chat notification, so it is carried
 * explicitly rather than guessed from the wording.
 */
function addPreset(name, kind) {
  extras.value.push({ name, unit_amount: '', quantity: 1, kind })
}

const selectedOrders = computed(() =>
  orders.value.filter((o) => selectedOrderIds.value.has(o.id)),
)

const totalDollars = computed(() => {
  let cents = 0
  for (const o of selectedOrders.value) cents += Math.round(Number(o.amount || 0) * 100)
  for (const e of extras.value) {
    const amount = Number(e.unit_amount)
    const qty = Number(e.quantity) || 0
    if (Number.isFinite(amount)) cents += Math.round(amount * 100) * qty
  }
  return cents / 100
})

const canCreate = computed(
  () =>
    !!customer.value &&
    (selectedOrders.value.length > 0 || extras.value.some((e) => e.name && e.unit_amount)),
)

function money(cents) {
  return `$${(cents / 100).toFixed(2)}`
}

async function createCart() {
  busy.value = 'cart'
  error.value = ''
  notice.value = ''
  try {
    const body = {
      customer_id: customer.value.id,
      order_ids: [...selectedOrderIds.value],
      items: extras.value
        .filter((e) => e.name.trim() && e.unit_amount !== '')
        .map((e) => ({
          name: e.name.trim(),
          quantity: Number(e.quantity) || 1,
          unit_amount: String(e.unit_amount),
          kind: e.kind || 'other',
        })),
    }
    cart.value = await api.createCart(body)
  } catch (err) {
    error.value = err.message
  } finally {
    busy.value = ''
  }
}

function onCanceled() {
  reset()
  notice.value = 'Sale canceled. Those blends are free to sell again.'
}

function reset() {
  cart.value = null
  extras.value = []
  if (customer.value) loadOrders()
}

function startOver() {
  cart.value = null
  customer.value = null
  extras.value = []
  orders.value = []
  router.replace({ name: 'checkout' })
  search()
}

watch(customer, () => {
  notice.value = ''
})
</script>

<template>
  <p class="error" v-if="error">{{ error }}</p>
  <p class="notice" v-if="notice">{{ notice }}</p>

  <div class="card warn-card" v-if="!squareLive">
    <strong>Square is not connected.</strong>
    <p class="muted" style="margin: 0.35rem 0 0">
      This app is running against the built-in mock, so no card will be charged and
      any payment link it produces is not real. An admin needs to set the Square
      credentials on the server before taking money.
    </p>
  </div>

  <!-- Step 1: who is paying -->
  <template v-if="!customer">
    <div class="card">
      <h2>Who's paying?</h2>
      <form class="row" @submit.prevent="search">
        <div>
          <input
            v-model="query"
            type="text"
            inputmode="email"
            autocapitalize="none"
            spellcheck="false"
            placeholder="Search by email"
            aria-label="Search by email"
          />
        </div>
        <button class="ghost" type="submit" style="flex: none" :disabled="busy === 'search'">
          {{ busy === 'search' ? '…' : 'Search' }}
        </button>
      </form>
    </div>

    <div class="card">
      <h2>{{ query ? 'Matches' : 'Recent customers' }}</h2>
      <p class="muted" v-if="!customers.length">No customers found.</p>
      <button
        v-for="c in customers"
        :key="c.id"
        class="list-item"
        type="button"
        @click="pick(c)"
      >
        <span class="grow">
          <strong>{{ c.name || c.email }}</strong>
          <span class="muted">{{ c.email }}</span>
        </span>
      </button>
    </div>
  </template>

  <!-- Step 2: build the cart -->
  <template v-else-if="!cart">
    <button class="ghost" type="button" @click="startOver">← Different customer</button>

    <div class="card" style="margin-top: 1rem">
      <h2>{{ customer.name || customer.email }}</h2>
      <p class="muted" style="margin: 0">{{ customer.email }}</p>
    </div>

    <div class="card">
      <h2>Blends to sell</h2>
      <p class="muted" v-if="busy === 'orders'">Loading…</p>
      <p class="muted" v-else-if="!orders.length">
        Nothing waiting to be sold for this customer. Take an intake first, or add a
        line below.
      </p>
      <label
        v-for="order in orders"
        :key="order.id"
        class="list-item"
        :class="{ 'is-selected': selectedOrderIds.has(order.id) }"
      >
        <input
          type="checkbox"
          :checked="selectedOrderIds.has(order.id)"
          :disabled="!order.amount"
          @change="toggle(order)"
        />
        <span class="grow">
          <strong>{{ order.order_type === 'custom_mix' ? 'Custom mix' : 'Set perfume' }}</strong>
          <span class="muted">{{ bottleLabel(order.size) }} · {{ order.status }}</span>
          <span class="muted danger-text" v-if="!order.amount">
            No price set — price this order before it can be sold.
          </span>
        </span>
        <span class="badge" v-if="order.amount">${{ order.amount }}</span>
      </label>
    </div>

    <div class="card">
      <h2>Other lines</h2>
      <p class="muted">
        Anything that isn't a bottle — event deposits, rush fees, the multi-day hotel
        line.
      </p>

      <div class="row" style="flex-wrap: wrap; gap: 0.5rem">
        <button
          class="ghost"
          type="button"
          style="flex: none"
          @click="addPreset('Event deposit (50%)', 'event_deposit')"
        >
          + Event deposit
        </button>
        <button
          class="ghost"
          type="button"
          style="flex: none"
          @click="addPreset('Rush / administrative fee', 'fee')"
        >
          + Rush fee
        </button>
        <button
          class="ghost"
          type="button"
          style="flex: none"
          @click="addPreset('Hotel room — product storage', 'fee')"
        >
          + Hotel room
        </button>
        <button class="ghost" type="button" style="flex: none" @click="addExtra">
          + Blank line
        </button>
      </div>

      <div v-for="(extra, i) in extras" :key="i" class="extra-row">
        <select v-model="extra.kind" aria-label="Line type" class="kind">
          <option value="other">Other</option>
          <option value="event_deposit">Event deposit</option>
          <option value="fee">Fee</option>
        </select>
        <input v-model="extra.name" type="text" placeholder="Description" aria-label="Line description" />
        <input
          v-model="extra.quantity"
          type="number"
          min="1"
          step="1"
          aria-label="Quantity"
          class="qty"
        />
        <input
          v-model="extra.unit_amount"
          type="number"
          min="0"
          step="0.01"
          inputmode="decimal"
          placeholder="0.00"
          aria-label="Unit amount"
          class="amt"
        />
        <button class="icon" type="button" @click="removeExtra(i)" aria-label="Remove line">✕</button>
      </div>
    </div>

    <div class="card total-card">
      <div class="row" style="align-items: baseline">
        <span class="grow"><strong>Total</strong></span>
        <span class="total">${{ totalDollars.toFixed(2) }}</span>
      </div>
      <button
        class="primary"
        type="button"
        style="margin-top: 0.9rem"
        :disabled="!canCreate || busy === 'cart'"
        @click="createCart"
      >
        {{ busy === 'cart' ? 'Building the cart…' : 'Continue to payment' }}
      </button>
    </div>
  </template>

  <!-- Step 3: take the payment -->
  <template v-else>
    <PayPanel :key="cart.id" :cart-id="cart.id" @canceled="onCanceled" />
    <div class="row" style="justify-content: center; margin-top: 0.5rem">
      <button class="primary" type="button" style="flex: none" @click="startOver">
        Next customer
      </button>
    </div>
  </template>
</template>

<style scoped>
.warn-card {
  border-color: var(--danger, #b4462f);
}

.extra-row {
  display: flex;
  gap: 0.5rem;
  align-items: center;
  margin-top: 0.6rem;
}

.extra-row input {
  min-width: 0;
}

.extra-row .kind {
  width: 8.5rem;
  flex: none;
}

.extra-row .qty {
  width: 4.5rem;
  flex: none;
}

.extra-row .amt {
  width: 7rem;
  flex: none;
}

.is-selected {
  border-color: var(--gold, #ac854a);
}

.list-item input[type='checkbox'] {
  width: 1.15rem;
  height: 1.15rem;
  flex: none;
  margin-right: 0.75rem;
}

.total {
  font-size: 1.8rem;
  font-weight: 600;
}

.total-card {
  position: sticky;
  bottom: 0;
}

</style>
