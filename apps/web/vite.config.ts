import { fileURLToPath } from 'node:url'
import { DEFAULT_PORT } from '@lila/shared'
import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

const server = `http://127.0.0.1:${process.env.LILA_PORT || DEFAULT_PORT}`

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: { alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) } },
  server: { proxy: { '/api': { target: server, ws: true } } },
})
