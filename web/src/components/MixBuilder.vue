<script setup>
import { computed, reactive, ref, watch } from 'vue'

import {
  BOTTLE_SIZES,
  INGREDIENT_TYPES,
  bottleLabel,
  bottleMl,
  formatMl,
  formulaCheck,
  fromBaseMl,
  toBaseMl,
  totalMl,
} from '../lib/bottle.js'

/// Mirrors MAX_MIX_INGREDIENTS in the API. The server is still the authority —
/// this only keeps the operator from building a mix that would be rejected.
const MAX_INGREDIENTS = 8

const props = defineProps({
  modelValue: { type: Array, required: true },
  ingredients: { type: Array, required: true },
  /// The bottle being filled. Amounts are typed in this bottle's ml and must
  /// add up to its volume; v-model still carries 3.4 oz base amounts, which is
  /// what the API stores.
  size: { type: String, default: 'oz3_4' },
})

const emit = defineEmits(['update:modelValue'])

const picking = ref('')

const activeIngredients = computed(() => props.ingredients.filter((i) => i.active))

const chosenIds = computed(() => new Set(props.modelValue.map((item) => item.ingredient_id)))

const available = computed(() => activeIngredients.value.filter((i) => !chosenIds.value.has(i.id)))

const atCap = computed(() => props.modelValue.length >= MAX_INGREDIENTS)

/// The bottle's volume rule, shown live. The screens that save a formula
/// disable their button on the same check; the API refuses it regardless.
const status = computed(() => formulaCheck(props.modelValue, props.size))

const targetMl = computed(() => bottleMl(props.size))

/// What a row's box shows while it is being typed in. Without this, typing
/// "3." would round-trip through the base amount and come back as "3",
/// eating the decimal point. Cleared on blur and when the size changes.
const drafts = reactive({})
watch(
  () => props.size,
  () => Object.keys(drafts).forEach((k) => delete drafts[k]),
)

/// Rows whose typed amount is finer than 0.01 ml — what can be measured.
const tooFine = reactive({})

function shownAmount(index, item) {
  if (drafts[index] !== undefined) return drafts[index]
  if (item.amount_ml === '' || item.amount_ml == null) return ''
  return fromBaseMl(item.amount_ml, props.size)
}

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
  // 1 ml in the bottle being filled.
  emit('update:modelValue', [
    ...props.modelValue,
    { ingredient_id: picking.value, amount_ml: toBaseMl(1, props.size) },
  ])
  picking.value = ''
}

function setIngredient(index, value) {
  const next = props.modelValue.map((item, i) =>
    i === index ? { ...item, ingredient_id: value } : item,
  )
  emit('update:modelValue', next)
}

/// `value` is in the bottle's ml; it is stored as the base amount.
function setAmount(index, value) {
  drafts[index] = value
  const fine = value !== '' && /\.\d{3,}/.test(String(value))
  if (fine) tooFine[index] = true
  else delete tooFine[index]
  const next = props.modelValue.map((item, i) =>
    i === index
      ? { ...item, amount_ml: value === '' || fine ? '' : toBaseMl(value, props.size) }
      : item,
  )
  emit('update:modelValue', next)
}

function endEdit(index) {
  if (!tooFine[index]) delete drafts[index]
}

function removeAt(index) {
  // Row positions shift; drop per-row typing state rather than misattribute it.
  Object.keys(drafts).forEach((k) => delete drafts[k])
  Object.keys(tooFine).forEach((k) => delete tooFine[k])
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
      Amounts are what you pour for the <strong>{{ bottleLabel(size) }}</strong>, in
      millilitres, and must add up to exactly {{ formatMl(targetMl) }} ml. The pour
      chart below shows the same blend in every size.
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
        step="0.01"
        :value="shownAmount(index, item)"
        :aria-label="`${nameFor(item.ingredient_id)} amount in millilitres for the ${bottleLabel(size)}`"
        :aria-invalid="tooFine[index] ? 'true' : undefined"
        @input="setAmount(index, $event.target.value)"
        @blur="endEdit(index)"
      />
      <span class="unit" aria-hidden="true">ml</span>
      <button class="icon" type="button" :aria-label="`Remove ${nameFor(item.ingredient_id)}`" @click="removeAt(index)">
        ×
      </button>
    </div>

    <p v-if="Object.keys(tooFine).length" class="formula-status bad" role="alert">
      Amounts can be measured to 0.01 ml at most.
    </p>
    <p
      v-if="modelValue.length"
      class="formula-status"
      :class="status.ok ? 'ok' : 'bad'"
      role="status"
    >
      {{ status.ok ? '✓ ' : '' }}{{ status.message }}
    </p>

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

    <!-- Pour chart: the blend is stored once at the 3.4oz base and every bottle
         size is derived here (1.7oz is half; roller and spray are a tenth — both
         are 10 ml, differing only in the closure). The size being filled is
         highlighted. -->
    <template v-if="modelValue.length">
      <p class="muted pour-caption">Pour chart — amounts per bottle size (ml)</p>
      <div class="pour-chart">
        <table>
          <thead>
            <tr>
              <th>Ingredient</th>
              <th v-for="s in BOTTLE_SIZES" :key="s.value" :class="{ current: s.value === size }">
                {{ s.label }}
              </th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="item in modelValue" :key="item.ingredient_id">
              <td class="ing">{{ nameFor(item.ingredient_id) }}</td>
              <td v-for="s in BOTTLE_SIZES" :key="s.value" :class="{ current: s.value === size }">
                {{ formatMl(fromBaseMl(item.amount_ml, s.value)) }}
              </td>
            </tr>
          </tbody>
          <tfoot>
            <tr>
              <td class="ing">Total</td>
              <td v-for="s in BOTTLE_SIZES" :key="s.value" :class="{ current: s.value === size }">
                {{ formatMl(Math.round(totalMl(modelValue, s.value) * 100) / 100) }}
              </td>
            </tr>
          </tfoot>
        </table>
      </div>
    </template>
  </div>
</template>

<style scoped>
.formula-status {
  margin: 0.4rem 0 0.8rem;
  font-size: 0.92rem;
  font-weight: 600;
}

.formula-status.ok {
  color: var(--ok);
}

.formula-status.bad {
  color: var(--danger);
}
</style>
