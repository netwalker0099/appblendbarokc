<script setup>
import { onMounted, ref } from 'vue'

import MixBuilder from '../components/MixBuilder.vue'
import { api } from '../lib/api.js'
import { bottleLabel, formatMl, totalMl } from '../lib/bottle.js'
import { isAdmin } from '../lib/auth.js'

const query = ref('')
const customers = ref([])
const selected = ref(null)
const mixes = ref([])
const orders = ref([])
const ingredients = ref([])
const scents = ref([])

const searching = ref(false)
const loadingDetail = ref(false)
const error = ref('')
const notice = ref('')

// Editing a saved blend. Open to any employee — correcting a formula at the bar
// is ordinary work, not an admin privilege.
const editingMix = ref(null)
const editName = ref('')
const editItems = ref([])
const savingMix = ref(false)

function startEdit(mix) {
  editingMix.value = mix.id
  editName.value = mix.name || ''
  // Deactivated ingredients would be rejected wholesale by the API, so drop them
  // here and say so, rather than failing on save with nothing to point at.
  const activeIds = new Set(ingredients.value.filter((i) => i.active).map((i) => i.id))
  const usable = mix.items.filter((i) => activeIds.has(i.ingredient_id))
  if (usable.length !== mix.items.length) {
    notice.value = 'Some ingredients in this blend are no longer active and were left out.'
  }
  editItems.value = usable.map((i) => ({
    ingredient_id: i.ingredient_id,
    amount_ml: Number(i.amount_ml),
  }))
}

function cancelEdit() {
  editingMix.value = null
  editItems.value = []
}

async function saveMix() {
  if (!editName.value.trim()) {
    error.value = 'A blend needs a name.'
    return
  }
  savingMix.value = true
  error.value = ''
  try {
    await api.updateMix(editingMix.value, {
      name: editName.value.trim(),
      items: editItems.value.map((i) => ({ ...i, amount_ml: Number(i.amount_ml) })),
    })
    cancelEdit()
    notice.value = 'Blend updated.'
    await select(selected.value)
  } catch (err) {
    error.value = err.message
  } finally {
    savingMix.value = false
  }
}

async function removeMix(mix) {
  error.value = ''
  notice.value = ''
  try {
    await api.deleteMix(mix.id)
    notice.value = 'Blend deleted.'
    await select(selected.value)
  } catch (err) {
    // Blends attached to an order refuse deletion so history stays intact.
    error.value = err.message
  }
}

async function removeOrder(order) {
  error.value = ''
  notice.value = ''
  try {
    await api.deleteOrder(order.id)
    notice.value = 'Order deleted.'
    await select(selected.value)
  } catch (err) {
    error.value = err.message
  }
}

async function removeCustomer() {
  error.value = ''
  notice.value = ''
  try {
    const impact = await api.customerDeletionImpact(selected.value.id)
    if (!impact.can_delete) {
      error.value = impact.reason
      return
    }
    const ok = window.confirm(
      `Delete ${impact.email}?\n\n` +
        `This removes ${impact.orders} order(s) and ${impact.mixes} blend(s). ` +
        `It cannot be undone.`,
    )
    if (!ok) return
    await api.deleteCustomer(selected.value.id)
    notice.value = `Deleted ${impact.email}.`
    back()
    await search()
  } catch (err) {
    error.value = err.message
  }
}

onMounted(async () => {
  try {
    const [ing, sc] = await Promise.all([api.listIngredients(), api.listScents()])
    ingredients.value = ing
    scents.value = sc
    await search()
  } catch (err) {
    error.value = err.message
  }
})

async function search() {
  searching.value = true
  error.value = ''
  try {
    customers.value = await api.listCustomers(query.value.trim())
  } catch (err) {
    error.value = err.message
  } finally {
    searching.value = false
  }
}

async function select(customer) {
  selected.value = customer
  loadingDetail.value = true
  error.value = ''
  try {
    // Single call returns the customer's mixes (with items) and orders — no
    // more per-mix fan-out over the stand's connection.
    const detail = await api.getReorder(customer.id)
    selected.value = detail.customer
    mixes.value = detail.mixes
    orders.value = detail.orders
  } catch (err) {
    error.value = err.message
  } finally {
    loadingDetail.value = false
  }
}

function back() {
  selected.value = null
  mixes.value = []
  orders.value = []
}

function ingredientName(id) {
  return ingredients.value.find((i) => i.id === id)?.name ?? 'Unknown'
}

function scentName(id) {
  return scents.value.find((s) => s.id === id)?.name ?? '—'
}

function describeMix(mix) {
  return mix.items.map((i) => `${ingredientName(i.ingredient_id)} ${formatMl(i.amount_ml)}ml`).join(' · ')
}

function describeScent(id) {
  const scent = scents.value.find((s) => s.id === id)
  if (!scent || !scent.items?.length) return ''
  return scent.items.map((i) => `${ingredientName(i.ingredient_id)} ${formatMl(i.amount_ml)}ml`).join(' · ')
}

function formatDate(value) {
  return new Date(value).toLocaleDateString(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
  })
}
</script>

<template>
  <p class="error" v-if="error">{{ error }}</p>
  <p class="notice" v-if="notice">{{ notice }}</p>

  <template v-if="!selected">
    <div class="card">
      <h2>Find a customer</h2>
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
        <button class="ghost" type="submit" style="flex: none" :disabled="searching">
          {{ searching ? '…' : 'Search' }}
        </button>
      </form>
    </div>

    <div class="card">
      <h2>{{ query ? 'Matches' : 'Recent customers' }}</h2>
      <p class="muted" v-if="!customers.length">No customers found.</p>
      <button
        v-for="customer in customers"
        :key="customer.id"
        class="list-item"
        type="button"
        @click="select(customer)"
      >
        <span class="grow">
          <strong>{{ customer.name || customer.email }}</strong>
          <span class="muted">{{ customer.email }}</span>
        </span>
        <span class="badge" v-if="customer.marketing_consent">opted in</span>
      </button>
    </div>
  </template>

  <template v-else>
    <button class="ghost" type="button" @click="back">← All customers</button>

    <div class="card" style="margin-top: 1rem">
      <h2>Customer</h2>
      <dl class="summary">
        <dt>Name</dt>
        <dd>{{ selected.name || '—' }}</dd>
        <dt>Email</dt>
        <dd>{{ selected.email }}</dd>
        <dt>Marketing</dt>
        <dd>{{ selected.marketing_consent ? 'Opted in' : 'Not opted in' }}</dd>
        <dt>Since</dt>
        <dd>{{ formatDate(selected.created_at) }}</dd>
      </dl>
    </div>

    <p class="muted" v-if="loadingDetail">Loading history…</p>

    <template v-else>
      <div class="card">
        <h2>Saved mixes</h2>
        <p class="muted" v-if="!mixes.length">No custom mixes yet.</p>
        <template v-for="mix in mixes" :key="mix.id">
          <div class="list-item" style="cursor: default">
            <span class="grow">
              <strong>{{ mix.name || 'Unnamed mix' }}</strong>
              <span class="muted">{{ describeMix(mix) }}</span>
              <span class="muted">{{ formatMl(totalMl(mix.items)) }} ml base · {{ formatDate(mix.created_at) }}</span>
            </span>
            <RouterLink
              class="ghost"
              style="flex: none"
              :to="{ name: 'intake', query: { mix: mix.id, customer: selected.id } }"
            >
              Reorder
            </RouterLink>
          </div>
          <div class="row" style="gap: 0.4rem; margin-bottom: 0.6rem">
            <button
              class="ghost"
              type="button"
              style="flex: none"
              @click="editingMix === mix.id ? cancelEdit() : startEdit(mix)"
            >
              {{ editingMix === mix.id ? 'Cancel' : 'Edit blend' }}
            </button>
            <button
              v-if="isAdmin"
              class="ghost"
              type="button"
              style="flex: none"
              @click="removeMix(mix)"
            >
              Delete
            </button>
          </div>

          <div v-if="editingMix === mix.id" class="card" style="margin-bottom: 0.8rem">
            <div class="field">
              <label>Blend name</label>
              <input v-model="editName" type="text" required />
            </div>
            <MixBuilder v-model="editItems" :ingredients="ingredients" />
            <button
              class="primary"
              type="button"
              style="margin-top: 0.7rem"
              :disabled="savingMix"
              @click="saveMix"
            >
              {{ savingMix ? 'Saving…' : 'Save blend' }}
            </button>
          </div>
        </template>
      </div>

      <div class="card">
        <h2>Orders</h2>
        <p class="muted" v-if="!orders.length">No orders yet.</p>
        <div v-for="order in orders" :key="order.id" class="list-item" style="cursor: default">
          <span class="grow">
            <strong>
              {{ order.order_type === 'custom_mix' ? 'Custom mix' : scentName(order.scent_id) }}
            </strong>
            <span
              class="muted"
              v-if="order.order_type === 'set_perfume' && describeScent(order.scent_id)"
            >
              {{ describeScent(order.scent_id) }}
            </span>
            <span class="muted">
              <template v-if="order.quantity > 1">{{ order.quantity }} × </template>
              {{ bottleLabel(order.size) }}
              <template v-if="order.amount"> · ${{ order.amount }}</template>
              · {{ formatDate(order.created_at) }}
            </span>
          </span>
          <span class="badge">{{ order.status }}</span>
          <button
            v-if="isAdmin && order.status === 'lead'"
            class="icon"
            type="button"
            aria-label="Delete order"
            title="Delete this unsold order"
            @click="removeOrder(order)"
          >
            ✕
          </button>
        </div>
      </div>

      <div class="row" style="gap: 0.5rem">
        <RouterLink class="primary" :to="{ name: 'intake', query: { customer: selected.id } }">
          New order for this customer
        </RouterLink>
        <RouterLink class="ghost" :to="{ name: 'checkout', query: { customer: selected.id } }">
          Take payment
        </RouterLink>
      </div>

      <div class="card" v-if="isAdmin" style="margin-top: 1rem">
        <h2>Danger zone</h2>
        <p class="muted">
          Deleting a customer removes their blends and unsold orders. Anyone with a
          paid or refunded cart cannot be deleted — that history has to reconcile
          against Square, which keeps its own record.
        </p>
        <button class="ghost" type="button" @click="removeCustomer">
          Delete this customer
        </button>
      </div>
    </template>
  </template>
</template>
