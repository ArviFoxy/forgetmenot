import { defineConfig } from 'vitest/config';

const serverTarget = process.env.FORGETMENOT_URL ?? 'http://127.0.0.1:7373';

export default defineConfig({
  build: { outDir: 'dist', emptyOutDir: true },
  server: {
    proxy: {
      '/api': { target: serverTarget, changeOrigin: true },
      '/hook': { target: serverTarget, changeOrigin: true },
      '/mcp': { target: serverTarget, changeOrigin: true },
    },
  },
  test: {
    environment: 'node',
    include: ['tests/**/*.test.ts'],
  },
});
