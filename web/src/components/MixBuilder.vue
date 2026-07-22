<script setup>
import { computed, ref } from 'vue'

import { bottleLabel, formatMl, scaleMl, totalMl } from '../lib/bottle.js'

/// Mirrors MAX_MIX_INGREDIENTS in the API. The server is still the authority —
/// this only keeps the operator from building a mix that would be rejected.
const MAX_INGREDIENTS = 8

const props = defineProps({
  modelValue: { type: Array, required: true },
  ingredients: { type: Array, required: true },
  size: { type: String, default: 'oz3_4' },
})

const emit = defineEmits(['update:modelValue'])

const picking = ref('')

const activeIngredients = computed(() => props.ingredients.filter((i) => i.active))

const chosenIds = computed(() => new Set(props.modelValue.map((item) => item.ingredient_id)))

const available = computed(() => activeIngredients.value.filter((i) => !chosenIds.value.has(i.id)))

const atCap = computed(() => props.modelValue.length >= MAX_INGREDIENTS)

function nameFor(id) {
  return props.ingredients.find((i) => i.id === id)?.name ?? 'Unknown ingredient'
}

function addIngredient() {
  if (!picking.value || atCap.value) return
  emit('update:modelValue', [...props.modelValue, { ingredient_id: picking.value, amount_ml: 1 }])
  picking.value = ''
}

function setAmount(index, value) {
  const next = props.modelValue.map((item, i) =>
    i === index ? { ...item, amount_ml: value === '' ? '' : Number(value) } : item,
  )
  emit('update:modelValue', next)
}

function removeAt(index) {
  emit(
    'update:modelValue',
    props.modelValue.filter((_, i) => i !== index),
  )
}
</script>

<template>
  <div class="card">
    <h2>Mix builder — {{ modelValue.length }}/{{ MAX_INGREDIENTS }}</h2>

    <p class="muted" v-if="!activeIngredients.length">
      No active ingredients yet. Add some before building a mix.
    </p>

    <div class="mix-row" v-for="(item, index) in modelValue" :key="item.ingredient_id">
      <span class="name">{{ nameFor(item.ingredient_id) }}</span>
      <input
        class="amount"
        type="number"
        inputmode="decimal"
        min="0.01"
        step="any"
        :value="item.amount_ml"
        :aria-label="`${nameFor(item.ingredient_id)} amount in millilitres`"
        @input="setAmount(index, $event.target.value)"
      />
      <button class="icon" type="button" :aria-label="`Remove ${nameFor(item.ingredient_id)}`" @click="removeAt(index)">
        ×
      </button>
    </div>

    <div class="row" v-if="available.length && !atCap">
      <div>
        <label for="add-ingredient">Add ingredient</label>
        <select id="add-ingredient" v-model="picking" @change="addIngredient">
          <option value="">Choose…</option>
          <option v-for="ingredient in available" :key="ingredient.id" :value="ingredient.id">
            {{ ingredient.name }}
          </option>
        </select>
      </div>
    </div>

    <p class="muted" v-else-if="atCap">
      Maximum of {{ MAX_INGREDIENTS }} ingredients reached.
    </p>

    <template v-if="modelValue.length">
      <div class="mix-total">
        <span>Total (3.4 oz base)</span>
        <span>{{ formatMl(totalMl(modelValue)) }} ml</span>
      </div>
      <div class="mix-total" v-if="size !== 'oz3_4'">
        <span>Poured for {{ bottleLabel(size) }}</span>
        <span>{{ formatMl(totalMl(modelValue, size)) }} ml</span>
      </div>
      <p class="muted" v-if="size !== 'oz3_4'">
        {{
          modelValue
            .map((i) => `${nameFor(i.ingredient_id)} ${formatMl(scaleMl(i.amount_ml, size))}ml`)
            .join(' · ')
        }}
      </p>
    </template>
  </div>
</template>
