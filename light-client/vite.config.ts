import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

const lightServerProxy = process.env.LIGHT_SERVER_PROXY ?? 'http://127.0.0.1:3000';
const viteHost = process.env.VITE_HOST ?? '127.0.0.1';
const vitePort = Number.parseInt(process.env.VITE_PORT ?? '5173', 10);

export default defineConfig({
  plugins: [react()],
  optimizeDeps: {
    // tiny-secp256k1 ships a browser WASM loader. Keep it out of Vite's
    // dependency pre-bundling so the package browser mapping can resolve
    // the WASM loader correctly in dev and build.
    exclude: ['tiny-secp256k1'],
  },
  server: {
    host: viteHost,
    port: Number.isFinite(vitePort) ? vitePort : 5173,
    strictPort: true,
    proxy: {
      '/api': {
        target: lightServerProxy,
        changeOrigin: true,
        rewrite: (path) => path.replace(/^\/api/, ''),
      },
    },
  },
});
