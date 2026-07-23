<script setup>
import { computed, ref } from 'vue'

const props = defineProps({
  title: { type: String, required: true },
  noun: { type: String, required: true }, // e.g. "ingredient"
  items: { type: Array, required: true },
  // Optional: enables a "type" selector on add and per row. Each entry
  // { value, label }; the order also defines the list's grouping order.
  types: { type: Array, default: () => [] },
})

const emit = defineEmits(['add', 'toggle', 'set-type'])

const draft = ref('')
const draftType = ref(props.types[0]?.value ?? '')
const busy = ref(false)

const typeRank = (value) => {
  const i = props.types.findIndex((t) => t.value === value)
  return i === -1 ? props.types.length : i
}
const typeLabel = (value) => props.types.find((t) => t.value === value)?.label ?? value

// Active first; within that, by type order (when typed) then name — the API
// already returns rows name-sorted, so this stable sort just layers on top.
const sorted = computed(() =>
  [...props.items].sort(
    (a, b) => Number(b.active) - Number(a.active) || typeRank(a.type) - typeRank(b.type),
  ),
)
const activeCount = computed(() => props.items.filter((i) => i.active).length)

async function add() {
  const name = draft.value.trim()
  if (!name || busy.value) return
  busy.value = true
  try {
    await emit('add', name, draftType.value)
    draft.value = ''
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <div class="card">
    <h2>{{ title }} — {{ activeCount }} active / {{ items.length }} total</h2>

    <form class="row" @submit.prevent="add">
      <div>
        <input
          v-model="draft"
          type="text"
          :placeholder="`Add ${noun}…`"
          :aria-label="`New ${noun} name`"
          autocapitalize="words"
        />
      </div>
      <div v-if="types.length" style="flex: none">
        <select v-model="draftType" :aria-label="`New ${noun} type`">
          <option v-for="t in types" :key="t.value" :value="t.value">{{ t.label }}</option>
        </select>
      </div>
      <button class="ghost" type="submit" style="flex: none" :disabled="busy || !draft.trim()">
        {{ busy ? '…' : 'Add' }}
      </button>
    </form>

    <p class="muted" v-if="!items.length">No {{ noun }}s yet.</p>

    <div
      v-for="item in sorted"
      :key="item.id"
      class="list-item"
      :class="{ inactive: !item.active }"
      style="cursor: default"
    >
      <span class="grow">
        <strong>{{ item.name }}</strong>
        <span class="muted" v-if="types.length">{{ typeLabel(item.type) }}</span>
      </span>
      <select
        v-if="types.length"
        style="flex: none; width: 9.5rem"
        :value="item.type"
        :aria-label="`${item.name} type`"
        @change="emit('set-type', item, $event.target.value)"
      >
        <option v-for="t in types" :key="t.value" :value="t.value">{{ t.label }}</option>
      </select>
      <span class="badge" v-if="!item.active">inactive</span>
      <button class="ghost" type="button" style="flex: none" @click="emit('toggle', item)">
        {{ item.active ? 'Deactivate' : 'Activate' }}
      </button>
    </div>
  </div>
</template>
