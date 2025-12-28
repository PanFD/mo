import { defineConfig } from 'vite';
import { resolve } from 'path';

export default defineConfig(async () => ({
  plugins: [],
  clearScreen: false,
  root: 'src',
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ['**/src-tauri/**'],
    },
  },
  build: {
    outDir: '../dist',
    emptyOutDir: true,
  },
}));
