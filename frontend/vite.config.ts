import { defineConfig, loadEnv } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig(({ mode }) => {
  const apiProxyTarget =
    loadEnv(mode, '.', 'RAIN_').RAIN_DEV_API_PROXY_TARGET || 'http://localhost:8080';

  return {
    plugins: [react()],
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
