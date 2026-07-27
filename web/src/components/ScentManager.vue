<script setup>
import { computed, reactive, ref, watch } from 'vue'

import { formatMl } from '../lib/bottle.js'
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
      prices[scent.id] = {
        oz3_4: scent.price_oz3_4 ?? '',
        oz1_7: scent.price_oz1_7 ?? '',
        roller: scent.price_roller ?? '',
        spray: scent.price_spray ?? '',
      }
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
  if (!scent.items.length) return 'No formula yet'
  return scent.items.map((i) => `${ingredientName(i.ingredient_id)} ${formatMl(i.amount_ml)}ml`).join(' · ')
}

function priceSummary(scent) {
  const parts = [
    ['3.4oz', scent.price_oz3_4],
    ['1.7oz', scent.price_oz1_7],
    ['roller', scent.price_roller],
    ['spray', scent.price_spray],
  ]
    .filter(([, v]) => v != null)
    .map(([label, v]) => `${label} $${v}`)
  return parts.length ? parts.join(' · ') : 'No prices set'
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
    await emit('save', scent, forms[scent.id], {
      oz3_4: priceVal(p.oz3_4),
      oz1_7: priceVal(p.oz1_7),
      roller: priceVal(p.roller),
      spray: priceVal(p.spray),
    })
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
        <button class="ghost" type="button" style="flex: none" @click="emit('toggle', scent)">
          {{ scent.active ? 'Deactivate' : 'Activate' }}
        </button>
      </div>

      <template v-if="expanded[scent.id]">
        <MixBuilder v-model="forms[scent.id]" :ingredients="ingredients" />

        <div class="card" style="margin: 0.6rem 0">
          <h2>Prices ($)</h2>
          <div class="row">
            <div>
              <label>3.4 oz</label>
              <input type="number" inputmode="decimal" min="0" step="0.01" v-model="prices[scent.id].oz3_4" />
            </div>
            <div>
              <label>1.7 oz</label>
              <input type="number" inputmode="decimal" min="0" step="0.01" v-model="prices[scent.id].oz1_7" />
            </div>
            <div>
              <label>Roller</label>
              <input type="number" inputmode="decimal" min="0" step="0.01" v-model="prices[scent.id].roller" />
            </div>
            <div>
              <label>Spray (10 ml)</label>
              <input type="number" inputmode="decimal" min="0" step="0.01" v-model="prices[scent.id].spray" />
            </div>
          </div>
        </div>

        <button class="ghost" type="button" :disabled="saving[scent.id]" @click="save(scent)">
          {{ saving[scent.id] ? 'Saving…' : 'Save formula & prices' }}
        </button>
      </template>
    </div>
  </div>
</template>
