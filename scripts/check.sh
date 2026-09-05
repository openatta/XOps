#!/usr/bin/env bash
# 提交前的四道门，按正确的顺序跑一遍。**和 CI 是同一份清单。**
#
# ⚠️ **顺序不是随便排的**：前端产物要在 `cargo build` 之前就位——
# 它在编译期被嵌进二进制（`D55`），而 `web/dist` 是 gitignore 的。
# 反了的话 `assets.rs` 只打一条 warning，二进制里没有页面，**不报错**。
#
# `scenarios/run.sh` **不在这里**：它要真的模型 key，而且是花钱的。
# 改了装配层、执行链或任何一处注入位，再单独跑它一遍。
set -uo pipefail
cd "$(dirname "$0")/.."

BAD=0
步() {
  printf '\n\033[1;36m━━ %s ━━\033[0m\n' "$1"; shift
  if "$@"; then printf '   \033[32m✓\033[0m\n'; else printf '   \033[31m✗ 没过\033[0m\n'; BAD=1; fi
}

步 "前端 · 类型与只读纪律"  bash -c 'cd web && npm run check'
步 "前端 · 测试"            bash -c 'cd web && npm test'
步 "前端 · 构建（产物要嵌进二进制）" bash -c 'cd web && npm run build'
步 "Rust · 格式"            ./scripts/fmt.sh --check
步 "Rust · clippy"          cargo clippy --workspace --all-targets -- -D warnings
步 "Rust · 测试"            cargo test --workspace
步 "契约 · 问实现"          bash -c 'cargo build -p xopsd && node scripts/contracts.mjs dump'
步 "契约 · 比基线"          node scripts/contracts.mjs check
步 "页面嵌进去了没有（D55）" bash -c '
  n=$(./target/debug/xopsd --dump-contracts | python3 -c "import json,sys; print(json.load(sys.stdin)[\"embeddedAssets\"])")
  echo "   嵌进去 $n 个文件"; [ "$n" -gt 0 ]'

printf '\n'
if [ $BAD -eq 0 ]; then
  printf '\033[32m四道门都过了\033[0m —— 改了执行链的话别忘了 scenarios/run.sh\n'
else
  printf '\033[31m有门没过\033[0m\n'
fi
exit $BAD
