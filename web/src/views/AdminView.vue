<script setup>
import { computed, onMounted, reactive, ref } from 'vue'
import { useRouter } from 'vue-router'

import CatalogManager from '../components/CatalogManager.vue'
import ScentManager from '../components/ScentManager.vue'
import TeamManager from '../components/TeamManager.vue'
import { INGREDIENT_TYPES } from '../lib/bottle.js'
import { api, downloadBackup } from '../lib/api.js'
import { currentUser } from '../lib/auth.js'

const router = useRouter()

const ingredients = ref([])
const scents = ref([])
const employees = ref([])
const sync = ref(null)
const square = ref(null)
const squareEvents = ref([])
const report = ref(null)
const reconciling = ref(false)
// Default the reconciliation window to the last 7 days, as YYYY-MM-DD.
const range = reactive({ from: isoDaysAgo(7), to: isoDaysAgo(0) })

function isoDaysAgo(n) {
  const d = new Date()
  d.setDate(d.getDate() - n)
  return d.toISOString().slice(0, 10)
}
const CUSTOM_PRICE_KEYS = [
  'custom_price_oz3_4',
  'custom_price_oz1_7',
  'custom_price_roller',
  'custom_price_spray',
]
const customPrices = reactive(Object.fromEntries(CUSTOM_PRICE_KEYS.map((k) => [k, ''])))
const savingPrices = ref(false)

const loading = ref(true)
const error = ref('')
const notice = ref('')

onMounted(load)

async function load() {
  loading.value = true
  error.value = ''
  try {
    const [ing, sc, emp, set, st, sq, ev] = await Promise.all([
      api.listIngredients(),
      api.listScents(),
      api.listEmployees(),
      api.getSettings(),
      api.getSyncStatus(),
      api.getSquareStatus(),
      api.listSquareEvents(),
    ])
    ingredients.value = ing
    scents.value = sc
    employees.value = emp
    sync.value = st
    square.value = sq
    squareEvents.value = ev
    for (const k of CUSTOM_PRICE_KEYS) {
      customPrices[k] = set[k] ?? ''
    }
  } catch (err) {
    handle(err)
  } finally {
    loading.value = false
  }
}

function handle(err) {
  error.value = err.message
  if (err.status === 401) router.push({ name: 'login' })
}

async function reloadTeam() {
  try {
    employees.value = await api.listEmployees()
  } catch (err) {
    handle(err)
  }
}

async function saveCustomPrices() {
  savingPrices.value = true
  try {
    const num = (v) => (v === '' || v === null ? null : Number(v))
    await api.updateSettings(
      Object.fromEntries(CUSTOM_PRICE_KEYS.map((k) => [k, num(customPrices[k])])),
    )
    flash('Saved custom-blend prices.')
  } catch (err) {
    handle(err)
  } finally {
    savingPrices.value = false
  }
}

async function refreshIntegration() {
  try {
    ;[sync.value, square.value, squareEvents.value] = await Promise.all([
      api.getSyncStatus(),
      api.getSquareStatus(),
      api.listSquareEvents(),
    ])
  } catch (err) {
    handle(err)
  }
}

/**
 * Run the reconciliation report.
 *
 * `save` persists a snapshot; the plain refresh doesn't, so this screen can be
 * reloaded freely without filling the audit table with duplicates.
 */
async function runReconcile(save = false) {
  reconciling.value = true
  error.value = ''
  try {
    report.value = await api.reconcile({
      // Cover whole days in local time: from 00:00 on `from` to 23:59:59 on `to`.
      from: new Date(`${range.from}T00:00:00`).toISOString(),
      to: new Date(`${range.to}T23:59:59`).toISOString(),
      save,
    })
    if (save) flash('Saved this reconciliation run.')
  } catch (err) {
    handle(err)
  } finally {
    reconciling.value = false
  }
}

function money(cents) {
  const sign = cents < 0 ? '-' : ''
  return `${sign}$${(Math.abs(cents) / 100).toFixed(2)}`
}

function shortId(id) {
  return id ? String(id).slice(0, 8) : '—'
}

async function addIngredient(name, type) {
  try {
    ingredients.value = [...ingredients.value, await api.createIngredient(name, type)]
    flash(`Added ingredient “${name}”.`)
  } catch (err) {
    handle(err)
  }
}
async function toggleIngredient(item) {
  try {
    const updated = await api.updateIngredient(item.id, { active: !item.active })
    ingredients.value = ingredients.value.map((i) => (i.id === updated.id ? updated : i))
  } catch (err) {
    handle(err)
  }
}
async function setIngredientType(item, type) {
  try {
    const updated = await api.updateIngredient(item.id, { type })
    ingredients.value = ingredients.value.map((i) => (i.id === updated.id ? updated : i))
  } catch (err) {
    handle(err)
  }
}
async function addScent(name) {
  try {
    scents.value = [...scents.value, await api.createScent(name)]
    flash(`Added scent “${name}”. Open “Formula” to set its ingredients.`)
  } catch (err) {
    handle(err)
  }
}
async function toggleScent(item) {
  try {
    const updated = await api.updateScent(item.id, { active: !item.active })
    scents.value = scents.value.map((s) => (s.id === updated.id ? updated : s))
  } catch (err) {
    handle(err)
  }
}
async function saveScentFormula(scent, items, prices) {
  try {
    const payload = items.map((i) => ({ ingredient_id: i.ingredient_id, amount_ml: Number(i.amount_ml) }))
    const updated = await api.updateScent(scent.id, { items: payload, prices })
    scents.value = scents.value.map((s) => (s.id === updated.id ? updated : s))
    flash(`Saved “${scent.name}”.`)
  } catch (err) {
    handle(err)
  }
}

async function retrySync() {
  try {
    const { requeued } = await api.retrySync()
    flash(requeued ? `Requeued ${requeued} failed sync(s).` : 'No failed syncs to retry.')
    await refreshIntegration()
  } catch (err) {
    handle(err)
  }
}

let flashTimer
function flash(msg) {
  notice.value = msg
  clearTimeout(flashTimer)
  flashTimer = setTimeout(() => (notice.value = ''), 4000)
}

const backingUp = ref(false)
async function backup() {
  backingUp.value = true
  try {
    const { blob, filename } = await downloadBackup()
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = filename
    document.body.appendChild(a)
    a.click()
    a.remove()
    URL.revokeObjectURL(url)
    flash(`Downloaded ${filename}.`)
  } catch (err) {
    handle(err)
  } finally {
    backingUp.value = false
  }
}

const failedCount = computed(() => sync.value?.counts?.failed ?? 0)

function formatTime(value) {
  return value ? new Date(value).toLocaleString() : '—'
}
</script>

<template>
  <p class="error" v-if="error">{{ error }}</p>
  <p class="notice" v-if="notice">{{ notice }}</p>
  <p class="muted" v-if="loading">Loading…</p>

  <template v-else>
    <CatalogManager
      title="Ingredients"
      noun="ingredient"
      :items="ingredients"
      :types="INGREDIENT_TYPES"
      @add="addIngredient"
      @toggle="toggleIngredient"
      @set-type="setIngredientType"
    />

    <ScentManager
      :scents="scents"
      :ingredients="ingredients"
      @add="addScent"
      @toggle="toggleScent"
      @save="saveScentFormula"
    />

    <div class="card">
      <h2>Custom blend pricing</h2>
      <p class="muted">Retail price per size for bespoke custom blends — applies to every custom blend.</p>
      <div class="row">
        <div>
          <label>3.4 oz</label>
          <input type="number" inputmode="decimal" min="0" step="0.01" v-model="customPrices.custom_price_oz3_4" />
        </div>
        <div>
          <label>1.7 oz</label>
          <input type="number" inputmode="decimal" min="0" step="0.01" v-model="customPrices.custom_price_oz1_7" />
        </div>
        <div>
          <label>Roller</label>
          <input type="number" inputmode="decimal" min="0" step="0.01" v-model="customPrices.custom_price_roller" />
        </div>
        <div>
          <label>Spray (10 ml)</label>
          <input type="number" inputmode="decimal" min="0" step="0.01" v-model="customPrices.custom_price_spray" />
        </div>
      </div>
      <button class="ghost" type="button" :disabled="savingPrices" style="margin-top: 0.8rem" @click="saveCustomPrices">
        {{ savingPrices ? 'Saving…' : 'Save custom-blend prices' }}
      </button>
    </div>

    <TeamManager
      :employees="employees"
      :current-email="currentUser?.email || ''"
      @changed="reloadTeam"
    />

    <div class="card">
      <h2>Square billing</h2>

      <dl class="summary" v-if="square">
        <dt>Payments</dt>
        <dd>
          <span class="badge" :class="square.live ? 'ok-badge' : 'danger-badge'">
            {{ square.live ? `Live — ${square.backend}` : 'Mock — no cards are charged' }}
          </span>
        </dd>
        <dt>Webhook receiver</dt>
        <dd>
          <span class="badge" :class="square.webhook_receiver_enabled ? 'ok-badge' : ''">
            {{ square.webhook_receiver_enabled ? 'Enabled' : 'Disabled (no signature key)' }}
          </span>
        </dd>
        <dt>Carts</dt>
        <dd>
          {{ square.cart_counts.paid }} paid · {{ square.cart_counts.pending_payment }} awaiting
          · {{ square.cart_counts.open }} open · {{ square.cart_counts.canceled }} canceled
          <template v-if="square.cart_counts.refunded">
            · <strong class="danger-text">{{ square.cart_counts.refunded }} refunded</strong>
          </template>
        </dd>
        <dt>Awaiting payment</dt>
        <dd>{{ money(square.pending_payment_cents) }}</dd>
        <dt>Contact sync</dt>
        <dd v-if="sync">
          {{ sync.counts.pending }} pending · {{ sync.counts.succeeded }} done ·
          <strong v-if="failedCount" class="danger-text">{{ failedCount }} failed</strong>
          <template v-else>0 failed</template>
        </dd>
      </dl>

      <p class="muted" v-if="square && !square.live">
        Set <code>SQUARE_ACCESS_TOKEN</code> and <code>SQUARE_LOCATION_ID</code> in the
        server’s <code>.env</code> (plus <code>SQUARE_ENV=sandbox</code> while testing)
        and restart to take real payments. Add
        <code>SQUARE_WEBHOOK_SIGNATURE_KEY</code> and <code>SQUARE_WEBHOOK_URL</code>
        so Square can report payments back automatically. These are server-side
        secrets and can’t be set from here.
      </p>

      <div class="row" style="margin-top: 0.75rem">
        <button class="ghost" type="button" style="flex: none" @click="refreshIntegration">
          Refresh
        </button>
        <button
          class="ghost"
          type="button"
          style="flex: none"
          :disabled="!failedCount"
          @click="retrySync"
        >
          Retry failed contact syncs
        </button>
      </div>
    </div>

    <div class="card">
      <h2>Reconciliation</h2>
      <p class="muted">
        Compares what this app recorded selling against what Square actually
        collected. Every sale on both sides lands in exactly one bucket below.
      </p>

      <div class="row" style="gap: 0.5rem; align-items: flex-end; flex-wrap: wrap">
        <label style="flex: none">
          <span class="muted">From</span>
          <input v-model="range.from" type="date" />
        </label>
        <label style="flex: none">
          <span class="muted">To</span>
          <input v-model="range.to" type="date" />
        </label>
        <button
          class="ghost"
          type="button"
          style="flex: none"
          :disabled="reconciling"
          @click="runReconcile(false)"
        >
          {{ reconciling ? 'Checking…' : 'Run' }}
        </button>
        <button
          class="ghost"
          type="button"
          style="flex: none"
          :disabled="reconciling || !report"
          @click="runReconcile(true)"
        >
          Run &amp; save
        </button>
      </div>

      <template v-if="report">
        <p
          class="recon-summary"
          :class="report.balanced ? 'ok-text' : 'danger-text'"
          style="margin-top: 1rem"
        >
          {{ report.summary }}
        </p>

        <p class="muted danger-text" v-if="!report.live">
          These figures came from the mock backend — they are a check of the logic,
          not of your books.
        </p>

        <dl class="summary">
          <dt>Square collected</dt>
          <dd>{{ money(report.square_total_cents) }}</dd>
          <dt>This app recorded</dt>
          <dd>{{ money(report.local_total_cents) }}</dd>
          <dt>Difference</dt>
          <dd :class="report.difference_cents === 0 ? '' : 'danger-text'">
            {{ money(report.difference_cents) }}
          </dd>
        </dl>

        <h3 v-if="report.amount_mismatch.length" class="danger-text">
          Amount mismatch ({{ report.amount_mismatch.length }})
        </h3>
        <p class="muted" v-if="report.amount_mismatch.length">
          Both sides have the sale, but the totals differ — a tip, a discount, or the
          price edited in Square after the link was made.
        </p>
        <div
          v-for="row in report.amount_mismatch"
          :key="row.cart_id"
          class="list-item"
          style="cursor: default"
        >
          <span class="grow">
            <strong>Cart {{ shortId(row.cart_id) }}</strong>
            <span class="muted">Square order {{ row.square_order_id }}</span>
          </span>
          <span class="badge danger-badge">
            {{ money(row.square_cents) }} vs {{ money(row.local_cents) }}
          </span>
        </div>

        <h3 v-if="report.missing_in_square.length" class="danger-text">
          Missing in Square ({{ report.missing_in_square.length }})
        </h3>
        <p class="muted" v-if="report.missing_in_square.length">
          Marked paid here, but Square has no matching payment in this window. Worth
          investigating — this is the bucket that means money may not have been taken.
        </p>
        <div
          v-for="row in report.missing_in_square"
          :key="row.cart_id"
          class="list-item"
          style="cursor: default"
        >
          <span class="grow">
            <strong>Cart {{ shortId(row.cart_id) }}</strong>
            <span class="muted">
              {{ row.square_order_id || 'never sent to Square' }} ·
              {{ row.paid_at ? formatTime(row.paid_at) : '—' }}
            </span>
          </span>
          <span class="badge danger-badge">{{ money(row.cents) }}</span>
        </div>

        <h3 v-if="report.unrecorded_payment.length" class="danger-text">
          Paid but unrecorded ({{ report.unrecorded_payment.length }})
        </h3>
        <p class="muted" v-if="report.unrecorded_payment.length">
          Square collected on these carts but this app never marked them paid —
          almost always a webhook that didn’t arrive. Open the cart and press “Check
          Square” to settle it, then check the webhook subscription.
        </p>
        <div
          v-for="row in report.unrecorded_payment"
          :key="row.cart_id"
          class="list-item"
          style="cursor: default"
        >
          <span class="grow">
            <strong>Cart {{ shortId(row.cart_id) }}</strong>
            <span class="muted">
              Square order {{ row.square_order_id }} ·
              {{ row.paid_at ? formatTime(row.paid_at) : '—' }}
            </span>
          </span>
          <span class="badge danger-badge">{{ money(row.square_cents) }} collected</span>
        </div>

        <h3 v-if="report.missing_locally.length">
          Only in Square ({{ report.missing_locally.length }})
        </h3>
        <p class="muted" v-if="report.missing_locally.length">
          Square took money with no cart here — usually a sale rung up directly in the
          Square POS. Not an error, but it should have an explanation.
        </p>
        <div
          v-for="row in report.missing_locally"
          :key="row.square_payment_id"
          class="list-item"
          style="cursor: default"
        >
          <span class="grow">
            <strong>{{ row.square_payment_id }}</strong>
            <span class="muted">{{ formatTime(row.created_at) }}</span>
          </span>
          <span class="badge">{{ money(row.cents) }}</span>
        </div>

        <h3 v-if="report.awaiting_payment.length">
          Awaiting payment ({{ report.awaiting_payment.length }})
        </h3>
        <p class="muted" v-if="report.awaiting_payment.length">
          Links issued but not paid. Not a discrepancy; they expire after 24 hours.
        </p>
        <div
          v-for="row in report.awaiting_payment"
          :key="row.cart_id"
          class="list-item"
          style="cursor: default"
        >
          <span class="grow">
            <strong>Cart {{ shortId(row.cart_id) }}</strong>
            <span class="muted">{{ formatTime(row.created_at) }}</span>
          </span>
          <span class="badge">{{ money(row.cents) }}</span>
        </div>

        <p class="muted" style="margin-top: 1rem">
          {{ report.matched.length }} matched exactly.
        </p>
      </template>
    </div>

    <div class="card">
      <h2>Recent Square events</h2>
      <p class="muted" v-if="!squareEvents.length">
        No webhooks received yet. Until the receiver is configured, use “Check Square”
        on a checkout to pull the result instead.
      </p>
      <div v-for="event in squareEvents" :key="event.id" class="list-item" style="cursor: default">
        <span class="grow">
          <strong>{{ event.event_type }}</strong>
          <span class="muted">
            {{ event.square_payment_id || event.square_order_id || '—' }} ·
            {{ formatTime(event.received_at) }}
          </span>
          <span class="muted danger-text" v-if="event.error">{{ event.error }}</span>
        </span>
        <span class="badge" :class="event.status === 'processed' ? 'ok-badge' : ''">
          {{ event.status }}
        </span>
      </div>
    </div>

    <div class="card">
      <h2>Backup</h2>
      <p class="muted">
        Download a full database backup (SQL) — restorable into a fresh Postgres if
        this box is ever lost. It contains all customer data, so store it somewhere
        safe.
      </p>
      <button class="ghost" type="button" :disabled="backingUp" @click="backup">
        {{ backingUp ? 'Preparing…' : 'Download database backup' }}
      </button>
    </div>
  </template>
</template>
