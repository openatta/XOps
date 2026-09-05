#!/usr/bin/env bash
# 起一个真的 xopsd，把场景跑一遍，跑完把痕迹清干净。
#
# 用法：scenarios/run.sh [场景前缀 ...]

set -uo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT" || exit 1

[ -f .env ] && . ./.env
: "${ANTHROPIC_AUTH_TOKEN:=}" "${ANTHROPIC_BASE_URL:=}" "${ANTHROPIC_DEFAULT_HAIKU_MODEL:=}"

export XOPS_MODEL_KEY="${XOPS_MODEL_KEY:-$ANTHROPIC_AUTH_TOKEN}"
export XOPS_MODEL_BASE_URL="${XOPS_MODEL_BASE_URL:-$ANTHROPIC_BASE_URL}"
export XOPS_MODEL="${XOPS_MODEL:-${ANTHROPIC_DEFAULT_HAIKU_MODEL:-claude-sonnet-4-6}}"

if [ -z "$XOPS_MODEL_KEY" ]; then
  echo "没有 XOPS_MODEL_KEY —— 场景里有真的模型调用，桩引擎证明不了任何事。" >&2
  exit 2
fi

# ⚠️ **不要放在 /tmp 的固定路径下**：外部清理会整批抹掉它（踩过，一次抹掉 19 个工作树）。
# mktemp 的目录带随机名，而且这里退出时自己删。
NEST=$(mktemp -d "${TMPDIR:-/tmp}/xops-scenarios-XXXXXX")
清场() {
  [ -n "${DAEMON:-}" ] && kill "$DAEMON" 2>/dev/null
  # 工作区与只读本地仓都去掉了写位（RPO-009），要先放开才删得掉。
  chmod -R u+w "$NEST" 2>/dev/null
  rm -rf "$NEST"
}
trap 清场 EXIT

空端口() { python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()'; }
MCP_PORT=$(空端口); WEB_PORT=$(空端口)

export XOPS_DB="$NEST/xops.db"
XOPS_SECRET_KEY=$(openssl rand -hex 32); export XOPS_SECRET_KEY
export XOPS_MCP_ADDR="127.0.0.1:$MCP_PORT"
export XOPS_WEB_ADDR="127.0.0.1:$WEB_PORT"
export XOPS_WORKSPACES="$NEST/workspaces"
# 前端产物：有 web/dist 就用它，没有就用编译期嵌进二进制的那一份（`D55`）。
# ⚠️ **两个都没有的话，深链回落那条断言会红**——那正是它该做的：
# `assets.rs` 在 `web/dist` 不在时只打一条 warning 就过去了，
# 于是"二进制里没有页面"这件事没有任何人会说。
[ -d "$ROOT/web/dist" ] && export XOPS_ASSETS="$ROOT/web/dist"
export XOPS_LOG=info
export SCENE_NEST="$NEST"

echo "编译…"
if ! cargo build --release -p xopsd 2>&1 | tail -1 | grep -q Finished; then
  cargo build --release -p xopsd 2>&1 | tail -20; exit 1
fi

# Web 会话与 MCP 令牌是**两套凭据**（`I-L` / `BRD-007`），所以两样都要备。
#
# ⚠️ **账号名要和签令牌那个一模一样。** `--issue-token` 把账号建在 `builtin` 上
# （`Directory::bootstrap_token`），预置账号也在 `builtin` 上——名字一致，
# 两条路才落在同一个用户身上。不一致的话场景照样跑得通，
# **只是 Web 上看到的是另一个人的空项目列表**，而那不报错。
export WEB_PW="场景口令"
export XOPS_LOGIN="alice@scenarios:${WEB_PW},bob@scenarios:${WEB_PW}"

# 第一把令牌只能从命令行来（MCP-002：每次调用都要带令牌，握手也不例外）。
XOPS_TOKEN=$(./target/release/xopsd --issue-token alice@scenarios 2>/dev/null | tail -1); export XOPS_TOKEN
TOKEN_B=$(./target/release/xopsd --issue-token bob@scenarios 2>/dev/null | tail -1); export TOKEN_B
case "$XOPS_TOKEN" in xops_*) ;; *) echo "签不出令牌"; exit 1 ;; esac

./target/release/xopsd > "$NEST/xopsd.log" 2>&1 &
DAEMON=$!
i=0
while [ $i -lt 100 ]; do
  grep -q "xopsd.started" "$NEST/xopsd.log" 2>/dev/null && break
  sleep 0.2; i=$((i+1))
done
grep -q "xopsd.started" "$NEST/xopsd.log" || { echo "起不来："; cat "$NEST/xopsd.log"; exit 1; }
echo "xopsd 起来了  MCP=$XOPS_MCP_ADDR  窝=$NEST"

BAD=0
for SCRIPT in "$ROOT"/scenarios/[0-9]*.sh; do
  NAME=$(basename "$SCRIPT")
  if [ $# -gt 0 ]; then
    HIT=0
    for PREFIX in "$@"; do case "$NAME" in "$PREFIX"*) HIT=1 ;; esac; done
    [ $HIT -eq 1 ] || continue
  fi
  printf '\n\033[1;36m━━ %s ━━\033[0m\n' "$NAME"
  bash "$SCRIPT" || BAD=1
done

printf '\n'
if [ $BAD -eq 0 ]; then
  printf '\033[32m全部通过\033[0m\n'
else
  printf '\033[31m有场景没过\033[0m  日志留在 %s\n' "$NEST/xopsd.log"
  cp "$NEST/xopsd.log" "${TMPDIR:-/tmp}/xops-scenarios-last.log" 2>/dev/null
  printf '（已复制一份到 %s）\n' "${TMPDIR:-/tmp}/xops-scenarios-last.log"
fi
exit $BAD
