import { defineConfig } from 'vitest/config';
import vue from '@vitejs/plugin-vue';

export default defineConfig({
  plugins: [vue()],
  clearScreen: false,
  envPrefix: ['VITE_'],
  test: {
    environment: 'jsdom',
    include: ['src/**/*.spec.ts'],
    globals: false,
    // css:true 才能让 `?raw` 拿到真内容；css:false 会把整个 CSS 模块 stub 成空串，
    // 于是 token 契约测试全部拿到 ''（假绿/假红都可能出现）。
    css: true,
    restoreMocks: true,
  },
});
