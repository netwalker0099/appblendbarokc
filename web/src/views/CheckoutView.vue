<script setup>
/**
 * Checkout: build a cart and hand it to Square.
 *
 * The operator never touches a card. This screen assembles the cart, asks the
 * API for a Square-hosted payment link, and then shows a QR code the customer
 * scans to pay on their own phone. Card details go straight from that phone to
 * Square — never through the tablet, this app, or the shop's network.
 */
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'

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
const checkout = ref(null)
const squareLive = ref(true)

const busy = ref('')
const error = ref('')
const notice = ref('')

// While a link is live, poll Square through our API so the screen flips to PAID
// on its own. The webhook usually gets there first; this covers the case where
// webhooks aren't configured yet, and means the operator isn't left guessing.
let poll = null

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

onUnmounted(stopPolling)

function stopPolling() {
  if (poll) {
    clearInterval(poll)
    poll = null
  }
}

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

async function createAndCheckout() {
  busy.value = 'checkout'
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
    checkout.value = await api.checkoutCart(cart.value.id)
    squareLive.value = checkout.value.live
    startPolling()
  } catch (err) {
    error.value = err.message
  } finally {
    busy.value = ''
  }
}

function startPolling() {
  stopPolling()
  poll = setInterval(async () => {
    try {
      const fresh = await api.getCart(cart.value.id)
      cart.value = fresh
      if (fresh.status !== 'pending_payment') stopPolling()
    } catch {
      // Transient; the next tick tries again.
    }
  }, 5000)
}

/** Ask Square directly — the backstop when a webhook never arrives. */
async function refresh() {
  busy.value = 'refresh'
  error.value = ''
  notice.value = ''
  try {
    const result = await api.refreshCart(cart.value.id)
    notice.value = result.detail
    cart.value = await api.getCart(cart.value.id)
    if (cart.value.status !== 'pending_payment') stopPolling()
  } catch (err) {
    error.value = err.message
  } finally {
    busy.value = ''
  }
}

async function cancel() {
  busy.value = 'cancel'
  error.value = ''
  try {
    await api.cancelCart(cart.value.id)
    stopPolling()
    reset()
    notice.value = 'Cart canceled. Those blends are free to sell again.'
  } catch (err) {
    error.value = err.message
  } finally {
    busy.value = ''
  }
}

function reset() {
  stopPolling()
  cart.value = null
  checkout.value = null
  extras.value = []
  if (customer.value) loadOrders()
}

function startOver() {
  stopPolling()
  cart.value = null
  checkout.value = null
  customer.value = null
  extras.value = []
  orders.value = []
  router.replace({ name: 'checkout' })
  search()
}

async function copyLink() {
  try {
    await navigator.clipboard.writeText(checkout.value.checkout_url)
    notice.value = 'Payment link copied.'
  } catch {
    notice.value = 'Could not copy — long-press the link to copy it.'
  }
}

const paid = computed(() => cart.value?.status === 'paid')
const shortfall = computed(() => {
  if (!paid.value || cart.value.paid_cents == null) return 0
  return cart.value.paid_cents - cart.value.total_cents
})

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
  <template v-else-if="!checkout">
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
        :disabled="!canCreate || busy === 'checkout'"
        @click="createAndCheckout"
      >
        {{ busy === 'checkout' ? 'Contacting Square…' : 'Create payment link' }}
      </button>
    </div>
  </template>

  <!-- Step 3: take the payment -->
  <template v-else>
    <div class="card pay-card">
      <h2 v-if="!paid">Scan to pay</h2>
      <h2 v-else class="paid-head">Paid ✓</h2>

      <p class="total" style="text-align: center">{{ money(cart.total_cents) }}</p>

      <template v-if="!paid && cart.status === 'pending_payment'">
        <p class="muted" style="text-align: center">
          The customer scans this with their phone camera and pays on Square's page.
        </p>
        <div class="qr">
          <img :src="`/api/carts/${cart.id}/checkout.svg`" alt="Payment QR code" />
        </div>
        <div class="row" style="justify-content: center; gap: 0.5rem; flex-wrap: wrap">
          <a class="ghost" :href="checkout.checkout_url" target="_blank" rel="noopener" style="flex: none">
            Open link
          </a>
          <button class="ghost" type="button" style="flex: none" @click="copyLink">Copy link</button>
          <button
            class="ghost"
            type="button"
            style="flex: none"
            :disabled="busy === 'refresh'"
            @click="refresh"
          >
            {{ busy === 'refresh' ? 'Checking…' : 'Check Square' }}
          </button>
        </div>
        <p class="muted" style="text-align: center; margin-top: 0.75rem">
          This page updates on its own when the payment lands.
        </p>
      </template>

      <template v-else-if="paid">
        <p style="text-align: center">
          Square collected {{ money(cart.paid_cents ?? cart.total_cents) }}.
        </p>
        <p class="danger-text" style="text-align: center" v-if="shortfall !== 0">
          That is {{ money(Math.abs(shortfall)) }}
          {{ shortfall > 0 ? 'more' : 'less' }} than the quoted total — it will show up
          on the reconciliation report.
        </p>
        <p class="muted" style="text-align: center">
          The blends on this cart are now marked paid.
        </p>
      </template>

      <template v-else>
        <p style="text-align: center">This cart is {{ cart.status }}.</p>
      </template>

      <div class="row" style="justify-content: center; gap: 0.5rem; margin-top: 1rem">
        <button class="primary" type="button" style="flex: none" @click="startOver">
          Next customer
        </button>
        <button
          v-if="cart.status === 'pending_payment'"
          class="ghost"
          type="button"
          style="flex: none"
          :disabled="busy === 'cancel'"
          @click="cancel"
        >
          Cancel cart
        </button>
      </div>
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

.pay-card {
  text-align: center;
}

.paid-head {
  color: var(--ok, #2f7d4f);
}

.qr {
  display: flex;
  justify-content: center;
  margin: 1rem 0;
}

.qr img {
  width: 260px;
  height: 260px;
  background: #fff;
  padding: 0.75rem;
  border-radius: 4px;
}
</style>
