import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    // include 用 glob 覆盖两个 workspace 位置，新增包无需修改本文件即被测试覆盖（SKEL-001）。
    include: [
      "apps/**/*.{test,spec}.ts",
      "packages/**/*.{test,spec}.ts",
      "test/**/*.{test,spec}.ts",
    ],
    exclude: ["**/node_modules/**", "**/dist/**", "3rds/**"],
    // 骨架阶段仓库里还没有任何包，空测试集不应判定为失败（SKEL-002）。
    passWithNoTests: true,
  },
});
