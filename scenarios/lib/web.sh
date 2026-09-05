#!/usr/bin/env bash
# 只读 Web 面这一侧的家当。**与 `mcp.sh` 并列，不是它的一部分。**
#
# # 为什么要有这个文件
#
# `scenarios/` 挡的是一类具体的缺陷：**单元全绿、装配也过，但那条链在运行时是断的，
# 而且不报错**。到今天为止用这个办法撞出十四处。
#
# ⚠️ **可是在这个文件出现之前，探针只会说 MCP。** 三条场景 539 行里 `/api/` 出现零次——
# 只读面上的任何东西，这张网**结构性地看不见**。而最近两处正好落在那一面：
#
# ```text
# 装配层从来没接过身份提供方   Web 上一个人都登不进来，日志里一个字都没有
# 查询串被 split('?') 扔掉      看板没有第二页，201 行的表少一行
# ```
#
# 两处都是单元全绿、装配也过。**加这个文件补的就是那张网上的洞。**
#
# # 一条纪律
#
# ⚠️ **登录要走真的 `POST /session`，不要拿 MCP 令牌绕过去。** 上面第一处洞
# 卡的正是这一步：用令牌当会话，就永远撞不到"没有身份提供方"。
# `I-L` / `BRD-007` 说两套凭据互不通用——**场景也要按这条走**。
#
# ⚠️ 变量名一律 ASCII —— 见 `lib/mcp.sh` 顶上那段。

set -uo pipefail

: "${XOPS_WEB_ADDR:?}"

# 当前会话 cookie。**空的时候 web() 不带 Cookie 头**——那正好用来验"没会话读不到"。
SESSION="${SESSION:-}"
# 上一次 `登录` 的结果。见那个函数上面的 ⚠️。
LOGIN=""

# 登录 <账号> <口令> —— 把会话存进 $SESSION，把结果存进 $LOGIN。
#
# ⚠️ **不要写成 `$(登录 …)`。** 命令替换开的是一个子 shell，里面对 `SESSION`
# 的赋值出不来——外面看到的还是空串，于是后面每一条读都是 401，
# 而"登录"那一条**照样是绿的**。踩过一次：20 通过 27 失败，头一条还写着"登得进来"。
# 所以结果放在 `$LOGIN` 里，调用方 `登录 a b` 之后再断言它。
#
# ⚠️ **这是唯一一处非 GET，和前端那条纪律同一个理由**（`BRD-005`：
# 写操作的唯一出口是凭据类的会话面）。
登录() {
  local account=$1 secret=$2 headers
  SESSION=""
  headers=$(curl -s --noproxy '*' --max-time 30 -D - -o /dev/null \
    -X POST "http://${XOPS_WEB_ADDR}/session" \
    -H 'Content-Type: application/json' \
    -d "{\"provider\":\"builtin\",\"account\":\"${account}\",\"secret\":\"${secret}\"}")
  case "$headers" in
    *" 200 "*)
      SESSION=$(printf '%s' "$headers" | sed -n 's/.*xops_session=\([^;[:space:]]*\).*/\1/p' | head -1)
      if [ -n "$SESSION" ]; then LOGIN="ok"; else LOGIN="登录 200 了但没下发会话"; fi
      ;;
    *) LOGIN=$(printf '%s' "$headers" | head -1 | tr -d '\r') ;;
  esac
}

# 登录失败该回什么 —— 只要状态码。
登录状态() {
  curl -s --noproxy '*' --max-time 30 -o /dev/null -w '%{http_code}' \
    -X POST "http://${XOPS_WEB_ADDR}/session" \
    -H 'Content-Type: application/json' \
    -d "{\"provider\":\"builtin\",\"account\":\"$1\",\"secret\":\"$2\"}"
}

# web <路径> —— 回话体。带着当前会话。
web() {
  if [ -n "$SESSION" ]; then
    curl -s --noproxy '*' --max-time 30 -H "Cookie: xops_session=${SESSION}" \
      "http://${XOPS_WEB_ADDR}$1"
  else
    curl -s --noproxy '*' --max-time 30 "http://${XOPS_WEB_ADDR}$1"
  fi
}

# web状态 <路径> —— 只要状态码。
web状态() {
  if [ -n "$SESSION" ]; then
    curl -s --noproxy '*' --max-time 30 -o /dev/null -w '%{http_code}' \
      -H "Cookie: xops_session=${SESSION}" "http://${XOPS_WEB_ADDR}$1"
  else
    curl -s --noproxy '*' --max-time 30 -o /dev/null -w '%{http_code}' \
      "http://${XOPS_WEB_ADDR}$1"
  fi
}

# web发 <方法> <路径> —— 带着会话试着写。**用来证明没有地方可发**（`BRD-005` ①）。
web发() {
  curl -s --noproxy '*' --max-time 30 -o /dev/null -w '%{http_code}' \
    -X "$1" -H "Cookie: xops_session=${SESSION}" -H 'Content-Type: application/json' \
    -d '{}' "http://${XOPS_WEB_ADDR}$2"
}

# web类型 <路径> —— Content-Type。深链回落验的是它。
web类型() {
  curl -s --noproxy '*' --max-time 30 -o /dev/null -w '%{content_type}' \
    -H "Cookie: xops_session=${SESSION}" "http://${XOPS_WEB_ADDR}$1"
}

# 深取 <点分路径> —— 从 stdin 的 JSON 里取一个值。取不到就是空串。
#
# `mcp.sh` 的 `取` 只认顶层键；只读面回的是 `{"notices":[{...}]}` 这种，
# 要能写 `notices.0.kind`。数组用下标，其余用键名。
深取() {
  python3 -c '
import sys, json
try:
    node = json.load(sys.stdin)
except Exception:
    print(""); raise SystemExit
for part in sys.argv[1].split("."):
    if part == "":
        continue
    try:
        node = node[int(part)] if isinstance(node, list) else node[part]
    except Exception:
        print(""); raise SystemExit
print(node if isinstance(node, str) else json.dumps(node, ensure_ascii=False))' "$1"
}

# 数 <点分路径> —— 那个数组有几项。不是数组就回空串。
数() {
  python3 -c '
import sys, json
try:
    node = json.load(sys.stdin)
except Exception:
    print(""); raise SystemExit
for part in sys.argv[1].split("."):
    if part == "":
        continue
    try:
        node = node[int(part)] if isinstance(node, list) else node[part]
    except Exception:
        print(""); raise SystemExit
print(len(node) if isinstance(node, (list, dict)) else "")' "$1"
}
