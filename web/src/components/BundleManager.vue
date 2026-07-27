<script setup>
/**
 * Package deals — a named set of bottles sold for one headline price.
 *
 * The price sits on the package, not on its parts: the whole point is that it
 * differs from the sum. At intake the package price is split across its bottles,
 * weighted by what each is worth on its own, so order history still adds up.
 */
import { computed, reactive, ref } from 'vue'

import { api } from '../lib/api.js'
import { BOTTLE_SIZES, ORDER_TYPES, bottleLabel } from '../lib/bottle.js'

const props = defineProps({
  bundles: { type: Array, default: () => [] },
  scents: { type: Array, default: () => [] },
})
const emit = defineEmits(['changed'])

const busy = ref('')
const error = ref('')
const editing = ref(null)

const blank = () => ({
  name: '',
  description: '',
  price: '',
  active: true,
  items: [{ type: 'set_perfume', size: 'oz3_4', scent_id: '', quantity: 1 }],
})
const draft = reactive(blank())

const activeScents = computed(() => props.scents.filter((s) => s.active))

function reset() {
  Object.assign(draft, blank())
  editing.value = null
}

function edit(bundle) {
  editing.value = bundle.id
  Object.assign(draft, {
    name: bundle.name,
    description: bundle.description || '',
    price: bundle.price,
    active: bundle.active,
    items: bundle.items.map((i) => ({
      type: i.type,
      size: i.size,
      scent_id: i.scent_id || '',
      quantity: i.quantity,
    })),
  })
}

function addItem() {
  draft.items.push({ type: 'set_perfume', size: 'oz3_4', scent_id: '', quantity: 1 })
}

function removeItem(i) {
  draft.items.splice(i, 1)
}

/** What the bottles would cost bought separately — the number that makes a package look like a deal. */
function itemsAtFullPrice() {
  let total = 0
  for (const item of draft.items) {
    if (item.type !== 'set_perfume') continue
    const scent = props.scents.find((s) => s.id === item.scent_id)
    const price = scent ? Number(scent[`price_${item.size}`] ?? 0) : 0
    total += price * (Number(item.quantity) || 1)
  }
  return total
}

const savings = computed(() => {
  const full = itemsAtFullPrice()
  const price = Number(draft.price)
  if (!full || !Number.isFinite(price)) return null
  return { full, saving: full - price }
})

async function save() {
  if (!draft.name.trim() || draft.price === '') {
    error.value = 'A package needs a name and a price.'
    return
  }
  busy.value = 'save'
  error.value = ''
  try {
    const body = {
      name: draft.name.trim(),
      description: draft.description.trim() || null,
      price: Number(draft.price),
      active: draft.active,
      items: draft.items.map((i) => ({
        type: i.type,
        size: i.size,
        scent_id: i.type === 'set_perfume' ? i.scent_id || null : null,
        quantity: Number(i.quantity) || 1,
      })),
    }
    if (editing.value) await api.updateBundle(editing.value, body)
    else await api.createBundle(body)
    reset()
    emit('changed')
  } catch (err) {
    error.value = err.message
  } finally {
    busy.value = ''
  }
}

async function toggleActive(bundle) {
  busy.value = bundle.id
  error.value = ''
  try {
    await api.updateBundle(bundle.id, {
      name: bundle.name,
      description: bundle.description,
      price: Number(bundle.price),
      active: !bundle.active,
      items: bundle.items.map((i) => ({
        type: i.type,
        size: i.size,
        scent_id: i.scent_id,
        quantity: i.quantity,
      })),
    })
    emit('changed')
  } catch (err) {
    error.value = err.message
  } finally {
    busy.value = ''
  }
}

async function remove(bundle) {
  busy.value = bundle.id
  error.value = ''
  try {
    await api.deleteBundle(bundle.id)
    emit('changed')
  } catch (err) {
    // Sold packages refuse deletion so their orders keep their history.
    error.value = err.message
  } finally {
    busy.value = ''
  }
}

function describe(bundle) {
  return bundle.items
    .map((i) => {
      const what =
        i.type === 'custom_mix'
          ? 'Custom blend'
          : props.scents.find((s) => s.id === i.scent_id)?.name || 'Scent'
      const qty = i.quantity > 1 ? `${i.quantity} × ` : ''
      return `${qty}${what} (${bottleLabel(i.size)})`
    })
    .join(' + ')
}
</script>

<template>
  <div class="card">
    <h2>Package deals</h2>
    <p class="muted">
      A named set of bottles sold for one price. At intake the package price is
      split across its bottles, weighted by what each is worth on its own, so the
      order history still adds up.
    </p>

    <p class="error" v-if="error">{{ error }}</p>

    <div v-for="b in props.bundles" :key="b.id" class="list-item" style="cursor: default">
      <span class="grow">
        <strong>{{ b.name }} — ${{ b.price }}</strong>
        <span class="muted">{{ describe(b) }}</span>
        <span class="muted" v-if="b.description">{{ b.description }}</span>
      </span>
      <span class="badge" :class="b.active ? 'ok-badge' : ''">
        {{ b.active ? 'Offered' : 'Hidden' }}
      </span>
    </div>

    <div v-for="b in props.bundles" :key="`c-${b.id}`" class="row bundle-controls">
      <span class="muted grow">{{ b.name }}</span>
      <button class="ghost" type="button" :disabled="busy === b.id" @click="edit(b)">Edit</button>
      <button class="ghost" type="button" :disabled="busy === b.id" @click="toggleActive(b)">
        {{ b.active ? 'Hide' : 'Offer' }}
      </button>
      <button class="ghost" type="button" :disabled="busy === b.id" @click="remove(b)">
        Delete
      </button>
    </div>

    <p class="muted" v-if="!props.bundles.length">No packages yet.</p>

    <hr style="border: 0; border-top: 1px solid var(--border); margin: 1.2rem 0" />

    <h3 style="margin-bottom: 0.6rem">{{ editing ? 'Edit package' : 'New package' }}</h3>

    <div class="row">
      <div class="field grow">
        <label>Name</label>
        <input v-model="draft.name" type="text" placeholder="e.g. Date Night" />
      </div>
      <div class="field" style="flex: none; width: 8rem">
        <label>Price ($)</label>
        <input v-model="draft.price" type="number" inputmode="decimal" min="0" step="0.01" />
      </div>
    </div>

    <div class="field">
      <label>Description (optional)</label>
      <input v-model="draft.description" type="text" placeholder="Shown to staff at intake" />
    </div>

    <p class="muted" style="margin: 0.2rem 0 0.6rem" v-if="savings && savings.full > 0">
      Bought separately: <strong>${{ savings.full.toFixed(2) }}</strong>
      <template v-if="savings.saving > 0">
        · customer saves <strong>${{ savings.saving.toFixed(2) }}</strong>
      </template>
      <template v-else-if="savings.saving < 0">
        · <span class="danger-text">costs ${{ (-savings.saving).toFixed(2) }} more than separately</span>
      </template>
    </p>

    <label class="muted" style="display: block; margin-bottom: 0.4rem">What's in it</label>
    <div v-for="(item, i) in draft.items" :key="i" class="row bundle-item">
      <select v-model="item.type" aria-label="Item type" style="flex: none; width: 9rem">
        <option v-for="t in ORDER_TYPES" :key="t.value" :value="t.value">{{ t.label }}</option>
      </select>
      <select v-model="item.size" aria-label="Bottle size" style="flex: none; width: 9rem">
        <option v-for="s in BOTTLE_SIZES" :key="s.value" :value="s.value">{{ s.label }}</option>
      </select>
      <select
        v-if="item.type === 'set_perfume'"
        v-model="item.scent_id"
        aria-label="Scent"
        class="grow"
      >
        <option value="">Choose a scent…</option>
        <option v-for="s in activeScents" :key="s.id" :value="s.id">{{ s.name }}</option>
      </select>
      <span v-else class="muted grow" style="font-size: 0.85rem">
        Built at the bar — staff add the blend at intake
      </span>
      <input
        v-model="item.quantity"
        type="number"
        min="1"
        step="1"
        aria-label="Quantity"
        style="flex: none; width: 4.5rem"
      />
      <button
        v-if="draft.items.length > 1"
        class="icon"
        type="button"
        aria-label="Remove item"
        @click="removeItem(i)"
      >
        ✕
      </button>
    </div>

    <div class="row" style="gap: 0.5rem; margin-top: 0.7rem">
      <button class="ghost" type="button" style="flex: none" @click="addItem">+ Add item</button>
      <button class="ghost" type="button" style="flex: none" :disabled="busy === 'save'" @click="save">
        {{ busy === 'save' ? 'Saving…' : editing ? 'Save changes' : 'Create package' }}
      </button>
      <button v-if="editing" class="ghost" type="button" style="flex: none" @click="reset">
        Cancel
      </button>
    </div>
  </div>
</template>

<style scoped>
.bundle-controls {
  gap: 0.4rem;
  align-items: center;
  margin-bottom: 0.4rem;
  flex-wrap: wrap;
}

.bundle-item {
  gap: 0.4rem;
  align-items: center;
  margin-bottom: 0.5rem;
  flex-wrap: wrap;
}
</style>
