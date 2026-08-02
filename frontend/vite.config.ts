import { defineConfig, loadEnv } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig(({ mode }) => {
  const apiProxyTarget =
    loadEnv(mode, '.', 'RAIN_').RAIN_DEV_API_PROXY_TARGET || 'http://localhost:8080';

  return {
    plugins: [react()],
    test: {
      environment: 'jsdom',
      setupFiles: './tests/setup.ts',
      globals: true
    },
    server: {
      port: 5173,
      host: '0.0.0.0',
      proxy: {
        '/api': {
          target: apiProxyTarget,
          changeOrigin: true
        },
        '/readyz': {
          target: apiProxyTarget,
          changeOrigin: true
        }
      }
    }
  };
});
