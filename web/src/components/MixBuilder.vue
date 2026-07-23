<script setup>
import { computed, ref } from 'vue'

import { INGREDIENT_TYPES, bottleLabel, formatMl, scaleMl, totalMl } from '../lib/bottle.js'

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

/// Splits a list of ingredients into the perfumery type groups (in the order
/// defined by INGREDIENT_TYPES), dropping empties. Anything with an unexpected
/// type lands in a trailing "Other" group so it can't vanish from the picker.
function groupByType(list) {
  const known = new Set(INGREDIENT_TYPES.map((t) => t.value))
  const groups = INGREDIENT_TYPES.map((t) => ({
    label: t.label,
    items: list.filter((i) => i.type === t.value),
  }))
  const other = list.filter((i) => !known.has(i.type))
  if (other.length) groups.push({ label: 'Other', items: other })
  return groups.filter((g) => g.items.length)
}

/// The ingredients a given row may switch to: every active ingredient except
/// the ones other rows already use. Its own current ingredient stays in the
/// list so the select shows and keeps the selection. A row's ingredient may
/// have since been deactivated (an old mix reordered), so we fold it back in
/// too, otherwise the select would silently drop to a different value.
function optionsFor(index) {
  const usedByOthers = new Set(
    props.modelValue.filter((_, i) => i !== index).map((item) => item.ingredient_id),
  )
  const current = props.modelValue[index]?.ingredient_id
  return props.ingredients.filter(
    (i) => !usedByOthers.has(i.id) && (i.active || i.id === current),
  )
}

function addIngredient() {
  if (!picking.value || atCap.value) return
  emit('update:modelValue', [...props.modelValue, { ingredient_id: picking.value, amount_ml: 1 }])
  picking.value = ''
}

function setIngredient(index, value) {
  const next = props.modelValue.map((item, i) =>
    i === index ? { ...item, ingredient_id: value } : item,
  )
  emit('update:modelValue', next)
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

    <p class="muted" style="margin-top: -0.4rem">
      Amounts are in millilitres (ml), measured at the 3.4 oz base formula.
    </p>

    <p class="muted" v-if="!activeIngredients.length">
      No active ingredients yet. Add some before building a mix.
    </p>

    <div class="mix-row" v-for="(item, index) in modelValue" :key="index">
      <select
        class="name"
        :value="item.ingredient_id"
        :aria-label="`Ingredient ${index + 1}`"
        @change="setIngredient(index, $event.target.value)"
      >
        <optgroup v-for="group in groupByType(optionsFor(index))" :key="group.label" :label="group.label">
          <option v-for="option in group.items" :key="option.id" :value="option.id">
            {{ option.name }}{{ option.active ? '' : ' (inactive)' }}
          </option>
        </optgroup>
      </select>
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
      <span class="unit" aria-hidden="true">ml</span>
      <button class="icon" type="button" :aria-label="`Remove ${nameFor(item.ingredient_id)}`" @click="removeAt(index)">
        ×
      </button>
    </div>

    <div class="row" v-if="available.length && !atCap">
      <div>
        <label for="add-ingredient">Add ingredient</label>
        <select id="add-ingredient" v-model="picking" @change="addIngredient">
          <option value="">Choose…</option>
          <optgroup v-for="group in groupByType(available)" :key="group.label" :label="group.label">
            <option v-for="ingredient in group.items" :key="ingredient.id" :value="ingredient.id">
              {{ ingredient.name }}
            </option>
          </optgroup>
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
