<script setup>
import { computed, onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'

import CatalogManager from '../components/CatalogManager.vue'
import ScentManager from '../components/ScentManager.vue'
import { api } from '../lib/api.js'

const router = useRouter()

const ingredients = ref([])
const scents = ref([])
const sync = ref(null)
const webhooks = ref([])

const loading = ref(true)
const error = ref('')
const notice = ref('')

onMounted(load)

async function load() {
  loading.value = true
  error.value = ''
  try {
    const [ing, sc, st, wh] = await Promise.all([
      api.listIngredients(),
      api.listScents(),
      api.getSyncStatus(),
      api.listWebhooks(),
    ])
    ingredients.value = ing
    scents.value = sc
    sync.value = st
    webhooks.value = wh
  } catch (err) {
    handle(err)
  } finally {
    loading.value = false
  }
}

function handle(err) {
  error.value = err.message
  if (err.status === 401) router.push({ name: 'pair' })
}

async function refreshIntegration() {
  try {
    ;[sync.value, webhooks.value] = await Promise.all([api.getSyncStatus(), api.listWebhooks()])
  } catch (err) {
    handle(err)
  }
}

async function addIngredient(name) {
  try {
    ingredients.value = [...ingredients.value, await api.createIngredient(name)]
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
async function saveScentFormula(scent, items) {
  try {
    const payload = items.map((i) => ({ ingredient_id: i.ingredient_id, amount_ml: Number(i.amount_ml) }))
    const updated = await api.updateScent(scent.id, { items: payload })
    scents.value = scents.value.map((s) => (s.id === updated.id ? updated : s))
    flash(`Saved formula for “${scent.name}”.`)
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
      @add="addIngredient"
      @toggle="toggleIngredient"
    />

    <ScentManager
      :scents="scents"
      :ingredients="ingredients"
      @add="addScent"
      @toggle="toggleScent"
      @save="saveScentFormula"
    />

    <div class="card">
      <h2>Squarespace integration</h2>

      <dl class="summary" v-if="sync">
        <dt>Push backend</dt>
        <dd>
          <span class="badge" :class="sync.backend === 'mock' ? '' : 'ok-badge'">
            {{ sync.backend === 'mock' ? 'Mock (no API key)' : 'Live' }}
          </span>
        </dd>
        <dt>Webhook receiver</dt>
        <dd>
          <span class="badge" :class="sync.webhook_receiver_enabled ? 'ok-badge' : ''">
            {{ sync.webhook_receiver_enabled ? 'Enabled' : 'Disabled (no secret)' }}
          </span>
        </dd>
        <dt>Sync jobs</dt>
        <dd>
          {{ sync.counts.pending }} pending · {{ sync.counts.succeeded }} done ·
          <strong v-if="failedCount" class="danger-text">{{ failedCount }} failed</strong>
          <template v-else>0 failed</template>
        </dd>
      </dl>

      <p class="muted" v-if="sync && sync.backend === 'mock'">
        Set <code>SQUARESPACE_API_KEY</code> (push) and
        <code>SQUARESPACE_WEBHOOK_SECRET</code> (webhooks) in the server’s
        <code>.env</code> and restart to go live. These are server-side secrets and
        can’t be set from here.
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
          Retry failed syncs
        </button>
      </div>
    </div>

    <div class="card">
      <h2>Recent webhooks</h2>
      <p class="muted" v-if="!webhooks.length">No webhooks received yet.</p>
      <div v-for="event in webhooks" :key="event.id" class="list-item" style="cursor: default">
        <span class="grow">
          <strong>{{ event.topic }}</strong>
          <span class="muted">{{ event.squarespace_order_id || '—' }} · {{ formatTime(event.received_at) }}</span>
          <span class="muted danger-text" v-if="event.error">{{ event.error }}</span>
        </span>
        <span class="badge" :class="event.status === 'processed' ? 'ok-badge' : ''">
          {{ event.status }}
        </span>
      </div>
    </div>
  </template>
</template>
