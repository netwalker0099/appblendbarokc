<script setup>
import { computed, ref } from 'vue'

const props = defineProps({
  title: { type: String, required: true },
  noun: { type: String, required: true }, // e.g. "ingredient"
  items: { type: Array, required: true },
})

const emit = defineEmits(['add', 'toggle'])

const draft = ref('')
const busy = ref(false)

// Active first, then inactive; each group alphabetical (the API already sorts by
// name, so a stable sort on `active` is enough).
const sorted = computed(() =>
  [...props.items].sort((a, b) => Number(b.active) - Number(a.active)),
)
const activeCount = computed(() => props.items.filter((i) => i.active).length)

async function add() {
  const name = draft.value.trim()
  if (!name || busy.value) return
  busy.value = true
  try {
    await emit('add', name)
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
      </span>
      <span class="badge" v-if="!item.active">inactive</span>
      <button
        class="ghost"
        type="button"
        style="flex: none"
        @click="emit('toggle', item)"
      >
        {{ item.active ? 'Deactivate' : 'Activate' }}
      </button>
    </div>
  </div>
</template>
