import js from "@eslint/js";
import tseslint from "typescript-eslint";

// ignores 里必须包含 3rds —— 那是第三方 clone（deepseek-harness，7800+ 文件），
// 不是本仓库的代码，扫它既慢又会报出与我们无关的问题。
export default tseslint.config(
  {
    ignores: [
      "**/node_modules/**",
      "**/dist/**",
      "**/coverage/**",
      "3rds/**",
      "xforge/**",
      ".claude/**",
    ],
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
);
