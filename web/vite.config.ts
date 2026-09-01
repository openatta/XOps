import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// 构建产物随二进制发行（D55）：xops-web 从 web/dist 托管它，部署方不需要 Node。
export default defineConfig({
  plugins: [react()],
  build: { outDir: 'dist', emptyOutDir: true },
  server: {
    // 开发时把只读 API 与会话面代理到后端。**代理表里只有这两条**——
    // 前端没有第二条数据通路（RP-05 的读模型是它唯一能看见的东西）。
    proxy: {
      '/api': 'http://127.0.0.1:8080',
      '/session': 'http://127.0.0.1:8080',
    },
  },
  test: { environment: 'node' },
})
