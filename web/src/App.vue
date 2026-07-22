<script setup>
import { ref, onMounted } from 'vue'

const status = ref('checking...')

onMounted(async () => {
  try {
    const res = await fetch('/api/health')
    const data = await res.json()
    status.value = data.status
  } catch (err) {
    status.value = `error: ${err.message}`
  }
})
</script>

<template>
  <main>
    <h1>Blend Bar</h1>
    <p>API status: {{ status }}</p>
  </main>
</template>
