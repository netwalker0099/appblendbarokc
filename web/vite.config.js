import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

export default defineConfig({
  plugins: [vue()],
  server: {
    // `npm run dev` serves the SPA itself; /api/* goes to the API container so
    // the dev server behaves like Caddy does in the deployed stack.
    proxy: {
      '/api': process.env.API_ORIGIN || 'http://localhost:8080',
    },
  },
})
