#!/usr/bin/env bash
# 只格式化 **XOps 自己的** crate。
#
# ⚠️ **不要用 `cargo fmt --all`。** 它会连 `vendor/attacore` 一起格式化——
# 那是一个只读的子模块，改那边的代码是明令禁止的（改动会被上游清理掉，
# 而一次被清理掉的修改是查不出来的）。`[workspace] exclude` 拦不住 `cargo fmt`：
# 它顺着 path 依赖走，不看 workspace 成员表。**这一条踩过一次，75 个文件。**
#
# 用法：`./scripts/fmt.sh` 或 `./scripts/fmt.sh --check`
set -euo pipefail
cd "$(dirname "$0")/.."
args=()
for name in $(cargo metadata --no-deps --format-version 1 \
    | python3 -c 'import json,sys
for p in json.load(sys.stdin)["packages"]:
    print(p["name"])'); do
  args+=(-p "$name")
done
exec cargo fmt "${args[@]}" "$@"
