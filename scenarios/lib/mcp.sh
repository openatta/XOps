#!/usr/bin/env bash
# 场景脚本的全部家当：一次调用、几个断言。
#
# ⚠️ **变量名一律 ASCII。** macOS 自带 bash 3.2，多字节变量名会被当成一条命令
# （`root=x` → `command not found`）。函数名它认，所以断言这一侧留中文——
# 那里才是要读的地方。
#
# ⚠️ **失败时要把整个回话打出来。** 场景跑在一个已经起来的进程上，
# 没有 backtrace 可贴；"断言失败"四个字对排查毫无帮助。

set -uo pipefail

: "${XOPS_MCP_ADDR:?}" "${XOPS_TOKEN:?}"

PASS=0
FAIL=0
SCENE="${SCENE:-场景}"

# mcp <tool> <arguments-json>  —— 回话的 structuredContent，或者 {"error":...}
mcp() {
  local tool=$1 args=${2:-'{}'}
  curl -s --noproxy '*' --max-time 180 \
    -X POST "http://${XOPS_MCP_ADDR}/mcp" \
    -H "Authorization: Bearer ${XOPS_TOKEN}" \
    -H 'Content-Type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"${tool}\",\"arguments\":${args}}}" \
  | python3 -c '
import sys, json
try:
    d = json.load(sys.stdin)
except Exception as error:
    print(json.dumps({"error": f"回话不是 JSON：{error}"}, ensure_ascii=False)); sys.exit(0)
if "error" in d:
    print(json.dumps({"error": d["error"].get("data", d["error"])}, ensure_ascii=False)); sys.exit(0)
r = d.get("result", {})
print(json.dumps(r.get("structuredContent", r), ensure_ascii=False))'
}

# 取一个字段。取不到就是空串 —— 断言那一侧会说话。
取() { python3 -c 'import sys,json
try: d = json.load(sys.stdin)
except Exception: print(""); raise SystemExit
print(d.get(sys.argv[1], "") if isinstance(d, dict) else "")' "$1"; }

要() {  # 要 <说明> <实际> <期望>
  if [ "$2" = "$3" ]; then
    printf '   \033[32m✓\033[0m %s\n' "$1"; PASS=$((PASS+1))
  else
    printf '   \033[31m✗\033[0m %s\n     期望 %s\n     实际 %s\n' "$1" "$3" "$2"; FAIL=$((FAIL+1))
  fi
}

含() {  # 含 <说明> <实际> <该出现的子串>
  case "$2" in
    *"$3"*) printf '   \033[32m✓\033[0m %s\n' "$1"; PASS=$((PASS+1)) ;;
    *) printf '   \033[31m✗\033[0m %s\n     该含 %s\n     实际 %s\n' "$1" "$3" "$2"; FAIL=$((FAIL+1)) ;;
  esac
}

不含() {
  case "$2" in
    *"$3"*) printf '   \033[31m✗\033[0m %s\n     不该含 %s\n     实际 %s\n' "$1" "$3" "$2"; FAIL=$((FAIL+1)) ;;
    *) printf '   \033[32m✓\033[0m %s\n' "$1"; PASS=$((PASS+1)) ;;
  esac
}

节() { printf '\n\033[1m%s\033[0m\n' "$1"; }

# 模型这一次没照做，再试一遍。
#
# ⚠️ **重试挡的是模型的服从性，不是链路的对错。** 实测同一条断言 3 遍里红 1 遍——
# 而同一遍里正式触发那条是通过的，也就是说链路好的，只是那一次模型没调工具。
# **"经常红但不用管"的测试等于没有测试**：下次真断了也会被当成又一次模型抽风。
#
# 所以重试**必须打出来**：链断 = 连红到用光次数，照样红；
# 模型抽了 = 这行出现在输出里，人一眼看得见"它试了几次才过"。
再试一遍() { printf '   \033[33m↻\033[0m %s（第 %s 次）\n' "$1" "$2"; }

收工() {
  printf '\n  %s：\033[32m%d 通过\033[0m' "$SCENE" "$PASS"
  [ "$FAIL" -gt 0 ] && printf ' \033[31m%d 失败\033[0m' "$FAIL"
  printf '\n'
  [ "$FAIL" -eq 0 ]
}
