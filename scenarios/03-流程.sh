#!/usr/bin/env bash
# 一条全程经 MCP 定义的两步流程，第二步是并行组。
#
# ⚠️ 这个场景**一次模型调用都没有**：流程是人与任务在表上表态，平台只做判定。
# 它跑得快，也就该是回归里跑得最勤的那一个。
#
# ⚠️ **凡是要传带转义双引号的 JSON，一律先赋值给变量再用。**
# macOS 自带 bash 3.2 在 `cmd "$(f "{\"k\":\"v\"}")"` 这种写法里会丢掉引号上下文，
# 把参数从逗号处切开——`{"project":"01AAA"` 这样半截的 JSON 发出去，
# 服务端回的是"请求体不是 JSON"，而调用点看着完全正常。踩过一次，别再踩。
source "$(dirname "${BASH_SOURCE[0]}")/lib/mcp.sh"
SCENE="流程"

# 甲是发起人，乙是另一个人。**职责分离要的就是这个"另一个"。**
A="$XOPS_TOKEN"
B="${TOKEN_B:?}"
甲() { XOPS_TOKEN="$A" mcp "$@"; }
乙() { XOPS_TOKEN="$B" mcp "$@"; }

节 "① 建项目，把乙拉进来"
R=$(甲 project.create '{"slug":"t03","displayName":"流程场景"}')
PROJ=$(echo "$R" | 取 project)
要 "建得出项目" "$(echo "$PROJ" | cut -c1-2)" "01"

R=$(乙 identity.whoami '{}')
BOB=$(echo "$R" | 取 user)
ARGS="{\"project\":\"$PROJ\",\"user\":\"$BOB\",\"role\":\"member\"}"
R=$(甲 member.set "$ARGS")
要 "乙加进来了" "$(echo "$R" | 取 role)" "member"
R=$(乙 project.mine '{}')
含 "乙自己也看得见这个项目" "$R" "$PROJ"

节 "② 建结算表"
# 结算表放"谁对它做了什么表态"（FLW-005）。
# stage 这一列是为了让两个节点的筛选**互斥得证**——见第 ③ 段。
ARGS="{\"project\":\"$PROJ\",\"table\":\"reviews\",\"columns\":[{\"name\":\"stage\",\"type\":\"enum\",\"enumValues\":[\"初审\",\"复审甲\",\"复审乙\"],\"required\":true},{\"name\":\"verdict\",\"type\":\"enum\",\"enumValues\":[\"过\",\"不过\"],\"required\":true},{\"name\":\"note\",\"type\":\"text\",\"maxLen\":200}]}"
R=$(甲 table.create "$ARGS")
要 "建得出结算表" "$(echo "$R" | 取 table)" "reviews"

节 "③ 筛选证不出互斥的定义要被挡下（FLW-008③）"
# 保守口径：**宁可误拒**。误放的后果是运行时一行同时结算两个节点，
# 而那是事后查不出来的。
同筛选节点() {
  printf '{"name":"%s","pass":[{"op":"equals","column":"verdict","value":"过"}],"writerRoles":["member"]}' "$1"
}
X1=$(同筛选节点 一审); X2=$(同筛选节点 二审)
BAD="{\"project\":\"$PROJ\",\"name\":\"坏的\",\"settlementTable\":\"reviews\",\"steps\":[[$X1],[$X2]]}"
R=$(甲 flow.define "$BAD")
含 "证不出互斥就拒绝定义" "$R" "FLW-008"

节 "④ 打错一个键名要被拒（MCP-004）"
# 流程定义里最怕被静默丢掉的是 separationOfDuties——
# **少了它没有任何症状，只是审批不再需要第二个人。**
TYPO="{\"project\":\"$PROJ\",\"name\":\"打错\",\"settlementTable\":\"reviews\",\"steps\":[[{\"name\":\"n\",\"pass\":[{\"op\":\"present\",\"column\":\"verdict\"}],\"writerRoles\":[\"member\"],\"separationOfDudies\":true}]]}"
R=$(甲 flow.define "$TYPO")
含 "打错的键名被指出来" "$R" "separationOfDudies"
含 "而且指得出在哪一层" "$R" "steps[0][0]"

节 "⑤ 定义一条两步流程，第二步是并行组（FLW-001 / FLW-002）"
节点() {
  printf '{"name":"%s","pass":[{"op":"equals","column":"stage","value":"%s"},{"op":"equals","column":"verdict","value":"过"}],"reject":[{"op":"equals","column":"stage","value":"%s"},{"op":"equals","column":"verdict","value":"不过"}],"writerRoles":["member","maintainer","owner"],"separationOfDuties":true}' "$1" "$1" "$1"
}
N1=$(节点 初审); N2=$(节点 复审甲); N3=$(节点 复审乙)
DEF="{\"project\":\"$PROJ\",\"name\":\"两步评审\",\"settlementTable\":\"reviews\",\"steps\":[[$N1],[$N2,$N3]]}"
R=$(甲 flow.define "$DEF")
FLOW=$(echo "$R" | 取 flow)
要 "定义得出来" "$(echo "$FLOW" | cut -c1-2)" "01"
要 "版本号由平台排" "$(echo "$R" | 取 version)" "1"
要 "发布态" "$(echo "$R" | 取 state)" "published"

节 "⑥ 发起实例"
ARGS="{\"project\":\"$PROJ\",\"flow\":\"$FLOW\",\"version\":1,\"subjectKind\":\"pr\",\"subjectId\":\"#7\"}"
R=$(甲 flow.start "$ARGS")
INST=$(echo "$R" | 取 instance)

激活() { python3 -c 'import sys, json
try:
    d = json.load(sys.stdin)
except Exception:
    print("读不出"); raise SystemExit
print(",".join(sorted(d.get("active", ["没有 active 字段"]))))'; }
状态() { 甲 flow.status "{\"project\":\"$PROJ\",\"instance\":\"$INST\"}"; }
表态() {
  local WHO=$1 VALUES=$2 OUT
  OUT=$($WHO flow.settle "{\"project\":\"$PROJ\",\"instance\":\"$INST\",\"values\":\"$VALUES\"}")
  case "$OUT" in *'"error"'*) printf '     写不进去：%s\n' "$OUT" ;; esac
}
R=$(状态)
要 "第一个节点随即激活（FLW-011）" "$(echo "$R" | 激活)" "初审"

节 "⑦ 发起人自己表态不算数（FLW-026③）"
# 挡的是**闭环自批**：甲发起 → 甲通过，全程一个人，
# 审批唯一的价值（多一个人）当场归零。
表态 甲 '{\"stage\":\"初审\",\"verdict\":\"过\"}'
sleep 1
R=$(状态)
要 "还停在初审" "$(echo "$R" | 激活)" "初审"

# FLW-027：**行照常留在表里**，只是不结算 —— 而且写的人要收到通知。
R=$(甲 row.reviews.select "{\"project\":\"$PROJ\"}")
要 "那一行照常在表里（FLW-027）" "$(echo "$R" | python3 -c 'import sys,json;print(len(json.load(sys.stdin).get("rows",[])))')" "1"
R=$(甲 notice.unread '{}')
含 "写的人收到「没被采纳」" "$R" "row-not-settled"

节 "⑧ 另一个人表态才推进"
表态 乙 '{\"stage\":\"初审\",\"verdict\":\"过\"}'
sleep 1
R=$(状态)
要 "推进到并行组，两个节点同时激活（FLW-002）" "$(echo "$R" | 激活)" "复审乙,复审甲"

表态 乙 '{\"stage\":\"复审甲\",\"verdict\":\"过\"}'
sleep 1
R=$(状态)
要 "并行组过了一个，还差一个" "$(echo "$R" | 激活)" "复审乙"

表态 乙 '{\"stage\":\"复审乙\",\"verdict\":\"过\"}'
sleep 1
R=$(状态)
要 "全部通过才推进，实例进终态" "$(echo "$R" | 取 state)" "approved"

节 "⑨ 拒绝让整个实例立即进终态（FLW-034）"
ARGS="{\"project\":\"$PROJ\",\"flow\":\"$FLOW\",\"version\":1,\"subjectKind\":\"pr\",\"subjectId\":\"#8\"}"
R=$(甲 flow.start "$ARGS")
I2=$(echo "$R" | 取 instance)
ARGS="{\"project\":\"$PROJ\",\"instance\":\"$I2\",\"values\":\"{\\\"stage\\\":\\\"初审\\\",\\\"verdict\\\":\\\"不过\\\"}\"}"
R=$(乙 flow.settle "$ARGS")
不含 "写得进去" "$R" '"error"'
sleep 1
R=$(甲 flow.status "{\"project\":\"$PROJ\",\"instance\":\"$I2\"}")
要 "一票拒绝，整个实例拒绝" "$(echo "$R" | 取 state)" "rejected"

节 "⑩ 停用（FLW-006）"
ARGS="{\"project\":\"$PROJ\",\"flow\":\"$FLOW\",\"version\":1}"
R=$(甲 flow.disable "$ARGS")
要 "停得掉" "$(echo "$R" | 取 state)" "disabled"
ARGS="{\"project\":\"$PROJ\",\"flow\":\"$FLOW\",\"version\":1,\"subjectKind\":\"pr\",\"subjectId\":\"#9\"}"
R=$(甲 flow.start "$ARGS")
含 "停用之后发不起新实例" "$R" "已停用"
# 在途的继续走完 —— 上面那个已经 approved 的实例没被停用波及。
R=$(状态)
要 "在途实例不受影响" "$(echo "$R" | 取 state)" "approved"

收工
