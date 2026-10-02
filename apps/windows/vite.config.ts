import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';
import { canonicalFerretPlugin } from './scripts/prepare-ferret.mjs';

export default defineConfig({
  plugins: [react(), canonicalFerretPlugin()],
  clearScreen: false,
  server: { port: 1420, strictPort: true, host: '127.0.0.1', watch: { ignored: ['**/src-tauri/**', '**/dist/**'] } },
  build: { target: 'es2022', sourcemap: false },
  test: { environment: 'jsdom', include: ['tests/**/*.test.{ts,tsx}'], restoreMocks: true },
});
