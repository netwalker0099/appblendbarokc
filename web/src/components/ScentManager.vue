<script setup>
import { computed, reactive, ref, watch } from 'vue'

import { BOTTLE_SIZES, formatMl, formulaCheck } from '../lib/bottle.js'
import MixBuilder from './MixBuilder.vue'

const props = defineProps({
  scents: { type: Array, required: true }, // each: { id, name, active, items, price_* }
  ingredients: { type: Array, required: true },
})

const emit = defineEmits(['add', 'toggle', 'save'])

const draft = ref('')
const adding = ref(false)

// Per-scent editable copies of the formula + prices, and which rows are expanded.
const forms = reactive({})
const prices = reactive({})
const expanded = reactive({})
const saving = reactive({})

watch(
  () => props.scents,
  (scents) => {
    for (const scent of scents) {
      forms[scent.id] = scent.items.map((i) => ({
        ingredient_id: i.ingredient_id,
        amount_ml: Number(i.amount_ml),
      }))
      // Keyed off BOTTLE_SIZES so a new size picks up its own price box here
      // without this component needing to know the list.
      prices[scent.id] = Object.fromEntries(
        BOTTLE_SIZES.map((size) => [size.value, scent[`price_${size.value}`] ?? '']),
      )
    }
  },
  { immediate: true, deep: true },
)

const sorted = computed(() =>
  [...props.scents].sort((a, b) => Number(b.active) - Number(a.active)),
)

function ingredientName(id) {
  return props.ingredients.find((i) => i.id === id)?.name ?? 'Unknown'
}

function summarize(scent) {
  if (!scent.items.length) return 'No formula yet — can’t be sold at intake'
  return scent.items.map((i) => `${ingredientName(i.ingredient_id)} ${formatMl(i.amount_ml)}ml`).join(' · ')
}

function priceSummary(scent) {
  const parts = BOTTLE_SIZES.map((size) => [size.label, scent[`price_${size.value}`]])
    .filter(([, v]) => v != null)
    .map(([label, v]) => `${label} $${v}`)
  return parts.length ? parts.join(' · ') : 'No prices set'
}

/// An empty formula is allowed on an inactive scent (the recipe isn't written
/// yet, but prices can be); anything entered must add up to 100 ml before it
/// can be saved. An active scent can't have its formula emptied: intake needs
/// it to pour the bottle, and the API refuses it.
const formulaBlocked = (id) => {
  if (forms[id].length > 0) return !formulaCheck(forms[id]).ok
  const scent = props.scents.find((s) => s.id === id)
  return Boolean(scent?.active && scent.items.length)
}

const priceVal = (v) => (v === '' || v === null ? null : Number(v))

async function add() {
  const name = draft.value.trim()
  if (!name || adding.value) return
  adding.value = true
  try {
    await emit('add', name)
    draft.value = ''
  } finally {
    adding.value = false
  }
}

async function save(scent) {
  saving[scent.id] = true
  try {
    const p = prices[scent.id]
    await emit(
      'save',
      scent,
      forms[scent.id],
      Object.fromEntries(BOTTLE_SIZES.map((size) => [size.value, priceVal(p[size.value])])),
    )
  } finally {
    saving[scent.id] = false
  }
}
</script>

<template>
  <div class="card">
    <h2>Scents — {{ scents.filter((s) => s.active).length }} active / {{ scents.length }} total</h2>

    <form class="row" @submit.prevent="add">
      <div>
        <input
          v-model="draft"
          type="text"
          placeholder="Add scent…"
          aria-label="New scent name"
          autocapitalize="words"
        />
      </div>
      <button class="ghost" type="submit" style="flex: none" :disabled="adding || !draft.trim()">
        {{ adding ? '…' : 'Add' }}
      </button>
    </form>

    <p class="muted" v-if="!scents.length">No scents yet.</p>

    <div v-for="scent in sorted" :key="scent.id" class="scent" :class="{ inactive: !scent.active }">
      <div class="list-item" style="cursor: default; margin-bottom: 0">
        <span class="grow">
          <strong>{{ scent.name }}</strong>
          <span class="muted">{{ summarize(scent) }}</span>
          <span class="muted">{{ priceSummary(scent) }}</span>
        </span>
        <span class="badge" v-if="!scent.active">inactive</span>
        <button class="ghost" type="button" style="flex: none" @click="expanded[scent.id] = !expanded[scent.id]">
          {{ expanded[scent.id] ? 'Close' : 'Edit' }}
        </button>
        <button
          class="ghost"
          type="button"
          style="flex: none"
          :disabled="!scent.active && !scent.items.length"
          :title="!scent.active && !scent.items.length ? 'Add a formula totalling 100 ml first' : ''"
          @click="emit('toggle', scent)"
        >
          {{ scent.active ? 'Deactivate' : 'Activate' }}
        </button>
      </div>

      <template v-if="expanded[scent.id]">
        <MixBuilder v-model="forms[scent.id]" :ingredients="ingredients" />

        <div class="card" style="margin: 0.6rem 0">
          <h2>Prices ($)</h2>
          <div class="row">
            <div v-for="size in BOTTLE_SIZES" :key="size.value">
              <label :for="`price-${scent.id}-${size.value}`">{{ size.label }}</label>
              <input
                :id="`price-${scent.id}-${size.value}`"
                v-model="prices[scent.id][size.value]"
                type="number"
                inputmode="decimal"
                min="0"
                step="0.01"
              />
            </div>
          </div>
          <p class="muted">A blank price means this scent is not sold in that size.</p>
        </div>

        <button class="ghost" type="button" :disabled="saving[scent.id] || formulaBlocked(scent.id)" @click="save(scent)">
          {{ saving[scent.id] ? 'Saving…' : 'Save formula & prices' }}
        </button>
        <p class="muted field-help" v-if="formulaBlocked(scent.id)">
          <template v-if="forms[scent.id].length">
            The formula has to add up to 100 ml before this scent can be saved.
          </template>
          <template v-else>
            An active scent needs its formula — deactivate it first to remove it.
          </template>
        </p>
      </template>
    </div>
  </div>
</template>
