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
  test: {
    // ⚠️ **以前是 `'node'`，于是任何带 JSX 或 hook 的文件根本渲染不起来。**
    // 13 个源文件里有测试的只有 2 个（都是纯函数），页面那一层是**测不了**，
    // 不是"没写"。`BRD-009` 说恶意内容"**必须实际构造验证，不能只做代码审查**"——
    // 而断言 AST 与断言渲染之后的 DOM 不是一回事。
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./src/test-setup.ts'],
  },
})
