<script setup>
import { onMounted, ref } from 'vue'

import { api } from '../lib/api.js'
import { bottleLabel, formatMl, totalMl } from '../lib/bottle.js'

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
        <div v-for="mix in mixes" :key="mix.id" class="list-item" style="cursor: default">
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
      </div>

      <div class="card">
        <h2>Orders</h2>
        <p class="muted" v-if="!orders.length">No orders yet.</p>
        <div v-for="order in orders" :key="order.id" class="list-item" style="cursor: default">
          <span class="grow">
            <strong>
              {{ order.order_type === 'custom_mix' ? 'Custom mix' : scentName(order.scent_id) }}
            </strong>
            <span class="muted">
              {{ bottleLabel(order.size) }}
              <template v-if="order.amount"> · ${{ order.amount }}</template>
              · {{ formatDate(order.created_at) }}
            </span>
          </span>
          <span class="badge">{{ order.status }}</span>
        </div>
      </div>

      <RouterLink class="primary" :to="{ name: 'intake', query: { customer: selected.id } }">
        New order for this customer
      </RouterLink>
    </template>
  </template>
</template>
