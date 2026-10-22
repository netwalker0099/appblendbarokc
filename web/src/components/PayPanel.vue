<script setup>
/**
 * Take payment for a cart.
 *
 * Two ways, chosen per sale:
 *   - **Square Terminal** (the default when one is paired): the cart is sent to
 *     the Terminal through Square's cloud. The customer taps or inserts there;
 *     this panel polls and flips to Paid on its own. Works from any browser —
 *     the card never comes near this device.
 *   - **Payment link / QR**: the customer scans and pays on their own phone.
 *
 * Used by Intake (straight after Save) and by Checkout. Takes a cart id and
 * keeps its own copy of the cart up to date.
 */
import { computed, onMounted, onUnmounted, ref } from 'vue'

import { api } from '../lib/api.js'

const props = defineProps({
  cartId: { type: String, required: true },
})
const emit = defineEmits(['paid', 'canceled'])

const cart = ref(null)
const terminals = ref([])
/// From /api/square/ready. Null until known; a failure to ask leaves the
/// buttons on, since the server refuses anything that can't work anyway.
const ready = ref(null)
const notConnected = computed(() => ready.value?.awaiting_connection === true)
const link = ref(null)
const busy = ref('')
const error = ref('')
const notice = ref('')

let timer = null

const money = (cents) => `$${(Number(cents || 0) / 100).toFixed(2)}`

const paired = computed(() => terminals.value.filter((t) => t.status === 'paired'))
const defaultTerminal = computed(() => paired.value.find((t) => t.is_default) || paired.value[0])
const chosenTerminal = ref(null)

const status = computed(() => cart.value?.status)
const method = computed(() => cart.value?.payment_method)
const tStatus = computed(() => cart.value?.terminal_status)
const onTerminal = computed(() => ['PENDING', 'IN_PROGRESS', 'CANCEL_REQUESTED'].includes(tStatus.value))

const terminalName = computed(() => {
  const t = terminals.value.find((x) => x.device_id === cart.value?.terminal_device_id)
  return t ? t.name : 'the Terminal'
})

const CANCEL_REASONS = {
  BUYER_CANCELED: 'The customer canceled on the Terminal.',
  SELLER_CANCELED: 'Canceled.',
  TIMED_OUT: 'The Terminal timed out waiting for a card.',
}

onMounted(async () => {
  try {
    const [c, t, r] = await Promise.all([
      api.getCart(props.cartId),
      api.listTerminals(),
      api.squareReady().catch(() => null),
    ])
    cart.value = c
    terminals.value = t
    ready.value = r
    chosenTerminal.value = defaultTerminal.value?.id || null
    if (status.value === 'pending_payment') startPolling()
  } catch (err) {
    error.value = err.message
  }
})

onUnmounted(stopPolling)

function stopPolling() {
  if (timer) clearTimeout(timer)
  timer = null
}

/// Terminal sales are checked every 2 seconds — someone is standing there
/// waiting. A link is checked every 5; the customer is on their own phone.
function startPolling() {
  stopPolling()
  const tick = async () => {
    try {
      const fresh =
        method.value === 'terminal' ? await api.terminalStatus(props.cartId) : await api.getCart(props.cartId)
      cart.value = fresh
      if (fresh.status === 'paid') {
        stopPolling()
        emit('paid', fresh)
        return
      }
      if (fresh.status !== 'pending_payment') return stopPolling()
      // A canceled Terminal checkout needs a person to decide what's next.
      if (method.value === 'terminal' && !onTerminal.value && tStatus.value !== 'COMPLETED') {
        return stopPolling()
      }
    } catch {
      // Transient; try again next tick.
    }
    timer = setTimeout(tick, method.value === 'terminal' ? 2000 : 5000)
  }
  timer = setTimeout(tick, method.value === 'terminal' ? 1500 : 5000)
}

async function chargeTerminal() {
  busy.value = 'terminal'
  error.value = ''
  notice.value = ''
  try {
    cart.value = await api.chargeOnTerminal(props.cartId, chosenTerminal.value)
    startPolling()
  } catch (err) {
    error.value = err.message
  } finally {
    busy.value = ''
  }
}

async function cancelOnTerminal() {
  busy.value = 'cancel-terminal'
  error.value = ''
  try {
    cart.value = await api.cancelOnTerminal(props.cartId)
    if (status.value === 'paid') emit('paid', cart.value)
  } catch (err) {
    error.value = err.message
  } finally {
    busy.value = ''
  }
}

async function useLink() {
  busy.value = 'link'
  error.value = ''
  notice.value = ''
  try {
    link.value = await api.checkoutCart(props.cartId)
    cart.value = await api.getCart(props.cartId)
    startPolling()
  } catch (err) {
    error.value = err.message
  } finally {
    busy.value = ''
  }
}

/** Ask Square directly about a link payment — the backstop for a missed webhook. */
async function checkSquare() {
  busy.value = 'refresh'
  error.value = ''
  try {
    const result = await api.refreshCart(props.cartId)
    notice.value = result.detail
    cart.value = await api.getCart(props.cartId)
    if (status.value === 'paid') {
      stopPolling()
      emit('paid', cart.value)
    }
  } catch (err) {
    error.value = err.message
  } finally {
    busy.value = ''
  }
}

async function cancelSale() {
  if (!window.confirm('Cancel this sale? The blends stay saved and can be sold later.')) return
  busy.value = 'cancel'
  error.value = ''
  try {
    cart.value = await api.cancelCart(props.cartId)
    stopPolling()
    emit('canceled', cart.value)
  } catch (err) {
    error.value = err.message
  } finally {
    busy.value = ''
  }
}

const linkUrl = computed(() => link.value?.checkout_url || cart.value?.checkout_url)
const shortfall = computed(() =>
  status.value === 'paid' && cart.value.paid_cents != null ? cart.value.paid_cents - cart.value.total_cents : 0,
)
</script>

<template>
  <div class="card pay">
    <p class="error" v-if="error" role="alert">{{ error }}</p>
    <p class="notice" v-if="notice" role="status">{{ notice }}</p>

    <p class="muted" v-if="!cart">Loading…</p>

    <template v-else>
      <p class="pay-total">{{ money(cart.total_cents) }}</p>
      <ul class="pay-lines">
        <li v-for="item in cart.items" :key="item.id">
          <span>{{ item.quantity > 1 ? `${item.quantity} × ` : '' }}{{ item.name }}</span>
          <span>{{ money(item.unit_amount_cents * item.quantity) }}</span>
        </li>
        <li v-if="cart.discount_cents" class="muted">
          <span>Discount</span><span>−{{ money(cart.discount_cents) }}</span>
        </li>
      </ul>

      <!-- Paid -->
      <template v-if="status === 'paid'">
        <h2 class="paid-head">Paid ✓</h2>
        <p>Square collected {{ money(cart.paid_cents ?? cart.total_cents) }}.</p>
        <p class="muted" v-if="shortfall > 0">
          {{ money(shortfall) }} more than the total — most likely a tip on the Terminal.
        </p>
        <p class="danger-text" v-else-if="shortfall < 0">
          {{ money(-shortfall) }} less than the total. Check this sale in Square.
        </p>
      </template>

      <!-- Canceled / refunded -->
      <p v-else-if="status !== 'open' && status !== 'pending_payment'">This sale is {{ status }}.</p>

      <!-- Not sent yet: choose how to pay -->
      <template v-else-if="status === 'open' && notConnected">
        <p class="danger-text" role="alert">
          Square isn’t connected yet, so no payment can be taken. An admin needs to press
          <strong>Connect Square</strong> in Admin → Billing. The sale is saved and can be
          charged once that’s done.
        </p>
      </template>
      <template v-else-if="status === 'open'">
        <template v-if="paired.length">
          <div class="field" v-if="paired.length > 1">
            <label for="pay-terminal">Terminal</label>
            <select id="pay-terminal" v-model="chosenTerminal">
              <option v-for="t in paired" :key="t.id" :value="t.id">{{ t.name }}</option>
            </select>
          </div>
          <button class="primary" type="button" :disabled="!!busy" @click="chargeTerminal">
            {{ busy === 'terminal' ? 'Sending to the Terminal…' : `Charge ${money(cart.total_cents)} on ${paired.length > 1 ? 'Terminal' : defaultTerminal.name}` }}
          </button>
        </template>
        <p class="muted" v-else>
          No Square Terminal is paired yet. An admin can pair one in Admin → Billing.
        </p>
        <button class="ghost" type="button" :disabled="!!busy" @click="useLink">
          {{ busy === 'link' ? 'Contacting Square…' : 'Use a payment link / QR instead' }}
        </button>
      </template>

      <!-- On the Terminal -->
      <template v-else-if="method === 'terminal'">
        <template v-if="onTerminal">
          <div class="waiting" aria-live="polite">
            <span class="spinner" aria-hidden="true"></span>
            <strong v-if="tStatus === 'PENDING'">Sent to {{ terminalName }}</strong>
            <strong v-else-if="tStatus === 'IN_PROGRESS'">The customer is paying on {{ terminalName }}…</strong>
            <strong v-else>Canceling on the Terminal…</strong>
            <span class="muted" v-if="tStatus === 'PENDING'">Ask the customer to tap, insert or swipe their card.</span>
          </div>
          <button
            class="ghost"
            type="button"
            :disabled="!!busy || tStatus === 'CANCEL_REQUESTED'"
            @click="cancelOnTerminal"
          >
            {{ busy === 'cancel-terminal' ? 'Canceling…' : 'Cancel on the Terminal' }}
          </button>
        </template>
        <div class="waiting" v-else-if="tStatus === 'COMPLETED'" aria-live="polite">
          <span class="spinner" aria-hidden="true"></span>
          <strong>Approved — finishing up…</strong>
        </div>
        <template v-else>
          <p>{{ CANCEL_REASONS[cart.terminal_cancel_reason] || 'The Terminal checkout ended without a payment.' }}</p>
          <button class="primary" type="button" :disabled="!!busy" @click="chargeTerminal">
            {{ busy === 'terminal' ? 'Sending…' : 'Send to the Terminal again' }}
          </button>
        </template>
      </template>

      <!-- Payment link -->
      <template v-else>
        <p class="muted">The customer scans this with their phone camera and pays on Square's page.</p>
        <div class="qr">
          <img :src="`/api/carts/${cart.id}/checkout.svg`" alt="Payment QR code" />
        </div>
        <div class="row pay-actions">
          <a v-if="linkUrl" class="ghost" :href="linkUrl" target="_blank" rel="noopener">Open link</a>
          <button class="ghost" type="button" :disabled="!!busy" @click="checkSquare">
            {{ busy === 'refresh' ? 'Checking…' : 'Check Square' }}
          </button>
        </div>
        <p class="muted">This updates on its own when the payment lands.</p>
      </template>

      <button
        v-if="status === 'open' || (status === 'pending_payment' && !onTerminal)"
        class="ghost danger-text cancel-sale"
        type="button"
        :disabled="!!busy"
        @click="cancelSale"
      >
        Cancel sale
      </button>
    </template>
  </div>
</template>

<style scoped>
.pay {
  text-align: center;
}

.pay-total {
  font-size: 2rem;
  font-weight: 600;
  margin: 0 0 0.4rem;
}

.pay-lines {
  list-style: none;
  padding: 0;
  margin: 0 auto 1rem;
  max-width: 26rem;
  font-size: 0.92rem;
}

.pay-lines li {
  display: flex;
  justify-content: space-between;
  gap: 1rem;
  padding: 0.2rem 0;
}

.pay .primary,
.pay .ghost {
  width: 100%;
  max-width: 26rem;
  margin: 0.35rem auto 0;
  display: block;
}

.pay .field {
  max-width: 26rem;
  margin: 0 auto 0.5rem;
  text-align: left;
}

.paid-head {
  color: var(--ok);
}

.waiting {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 0.4rem;
  margin: 0.6rem 0 1rem;
}

.spinner {
  width: 2.2rem;
  height: 2.2rem;
  border: 3px solid var(--border);
  border-top-color: var(--accent);
  border-radius: 50%;
  animation: spin 0.9s linear infinite;
}

@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}

@media (prefers-reduced-motion: reduce) {
  .spinner {
    animation: none;
  }
}

.qr {
  display: flex;
  justify-content: center;
  margin: 1rem 0;
}

.qr img {
  width: 240px;
  height: 240px;
  background: #fff;
  padding: 0.75rem;
  border-radius: 4px;
}

.pay-actions {
  justify-content: center;
  gap: 0.5rem;
}

.pay-actions .ghost {
  width: auto;
  margin: 0;
}

.cancel-sale {
  margin-top: 1rem !important;
}
</style>
