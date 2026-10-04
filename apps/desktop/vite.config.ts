import { defineConfig } from 'vite';
import vue from '@vitejs/plugin-vue';

// Noto 前端构建配置。dev 端口固定 5173（Playwright / 文档均以该端口为准）。
export default defineConfig({
  plugins: [vue()],
  clearScreen: false,
  envPrefix: ['VITE_'],
  server: {
    port: 5173,
    strictPort: true,
    host: '127.0.0.1',
    watch: {
      // src-tauri 与 Rust 侧产物不属于前端编译输入，忽略以免无谓重启
      ignored: ['**/src-tauri/**', '**/dist/**', '**/node_modules/**'],
    },
  },
  build: {
    target: 'es2022',
    outDir: 'dist',
    sourcemap: true,
  },
});
