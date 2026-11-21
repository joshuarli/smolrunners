import { defineConfig } from 'vite';
import solid from 'vite-plugin-solid';

const target = process.env.SMOLRUNNERS_API_URL || 'http://127.0.0.1:8791';
const proxy = { '/api': { target, changeOrigin: true } };

export default defineConfig({
  plugins: [solid()],
  server: { host: '127.0.0.1', port: 5173, proxy },
  preview: { host: '127.0.0.1', port: 4173, proxy },
  build: { outDir: 'dist' },
});
