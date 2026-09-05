#!/usr/bin/env bash
# 只读 Web 面，从**使用者的方式**走一遍：登录 → 项目 → 表 → 看板 → 翻页 → 个人看板。
#
# ⚠️ **这个场景一次模型调用都没有。** 它跑得快，该是回归里跑得最勤的之一。
#
# # 它挡的是哪一类
#
# 前三条场景全程只说 MCP —— `/api/` 在它们里面出现零次。于是只读面上的东西
# **结构性地没有任何探针**，而最近两处缺陷正好都在那一面：
#
# ```text
# 装配层从来没接过身份提供方   Web 上一个人都登不进来，日志里一个字都没有
# 查询串被 split('?') 扔掉      看板没有第二页，201 行的表少一行
# ```
#
# ⚠️ **所以第 ① 段必须走真的 `POST /session`**。拿 MCP 令牌当会话绕过去，
# 就永远撞不到第一处——而它的症状是"页面在、路由在、就是进不去"。
#
# ⚠️ **凡是要传带转义双引号的 JSON，一律先赋值给变量再用**——见 `03-流程.sh` 顶上那段。
source "$(dirname "${BASH_SOURCE[0]}")/lib/mcp.sh"
source "$(dirname "${BASH_SOURCE[0]}")/lib/web.sh"
SCENE="看板"

A="$XOPS_TOKEN"
B="${TOKEN_B:?}"
甲() { XOPS_TOKEN="$A" mcp "$@"; }
乙() { XOPS_TOKEN="$B" mcp "$@"; }

节 "① 登录 —— 真的那一次 POST /session"
# ⚠️ 这两条要**回同一个东西**（`IDN-001`）：区分"账号不存在"与"口令不对"
# 是给探测者的，不是给运维的。
要 "口令打错登不进来" "$(登录状态 "alice@scenarios" "错的口令")" "401"
要 "账号不存在也是同一个错" "$(登录状态 "mallory@nowhere" "随便")" "401"

# ⚠️ **不能写成 `$(登录 …)`** —— 子 shell 里的赋值出不来，见 lib/web.sh。
登录 "alice@scenarios" "$WEB_PW"
要 "配了预置账号就登得进来" "$LOGIN" "ok"
含 "下发的是会话不是令牌（I-L）" "$SESSION" "xsess_"

# `I-L` / `BRD-007`：两套凭据**互不通用**，而且是靠"两边根本不认识对方"兑现的。
SAVED="$SESSION"
SESSION="$XOPS_TOKEN"
要 "MCP 令牌当不了 Web 会话（I-L）" "$(web状态 /api/me)" "401"
SESSION="$SAVED"

节 "② 没有会话什么都读不到"
SESSION=""
for P in /api/me /api/projects /api/me/notices; do
  要 "没会话读不到 $P" "$(web状态 "$P")" "401"
done
SESSION="$SAVED"

节 "③ 我是谁 · 我的项目 · 成员 · 表"
R=$(甲 project.create '{"slug":"t04","displayName":"看板场景"}')
PROJ=$(echo "$R" | 取 project)
要 "建得出项目" "$(echo "$PROJ" | cut -c1-2)" "01"

# ⚠️ **签令牌那个账号与登录那个账号必须是同一个人。** 不一致的话下面这条会红——
# 而在真实部署里它的症状是"我建的项目在页面上看不见"，不报错。
R=$(web /api/me)
要 "Web 上认得出我是谁（BRD-011）" "$(echo "$R" | 深取 account)" "alice@scenarios"

R=$(web /api/projects)
含 "刚建的项目在只读面上看得见" "$R" "$PROJ"

R=$(web "/api/projects/$PROJ/members")
要 "成员就我一个" "$(echo "$R" | 数 members)" "1"
要 "角色是所有者（PRJ-007）" "$(echo "$R" | 深取 members.0.role)" "owner"
要 "显示名在后端解出来" "$(echo "$R" | 深取 members.0.display_name)" "alice@scenarios"

ARGS="{\"project\":\"$PROJ\",\"table\":\"bugs\",\"columns\":[{\"name\":\"title\",\"type\":\"text\",\"maxLen\":64,\"required\":true},{\"name\":\"seq\",\"type\":\"integer\"}]}"
R=$(甲 table.create "$ARGS")
要 "建得出表" "$(echo "$R" | 取 table)" "bugs"

# ⚠️ 这一条挡的是"**一张还没建看板的表在页面上完全不存在**"：
# 在表清单这条路由出现之前，前端只知道有哪些**看板**，而没有任何地方会说这件事。
R=$(web "/api/projects/$PROJ/tables")
含 "还没建看板的表也看得见" "$R" '"bugs"'
含 "平台自己那几张表也在" "$R" '"_runs"'
不含 "但表清单里不回任何一行数据" "$R" '"rows"'

节 "④ 看板 —— 一张表的一个视图（BRD-001）"
# ⚠️ **表专属 tool 是逐字段声明的，不收一整份 JSON**（`MCP-004`）——
# 列是顶层参数，不是一个 `values` 对象。写错了回的是"多了一个键 values"，
# 而**行一条都没进去，看板上是空的**。
for I in 0 1 2 3 4; do
  ARGS="{\"project\":\"$PROJ\",\"title\":\"第${I}条\",\"seq\":${I}}"
  R=$(甲 row.bugs.insert "$ARGS")
  case "$R" in *'"error"'*) printf '     写不进去：%s\n' "$R" ;; esac
done

ARGS="{\"project\":\"$PROJ\",\"table\":\"bugs\",\"name\":\"全部缺陷\",\"sort\":\"seq\",\"direction\":\"desc\"}"
R=$(甲 board.define "$ARGS")
BOARD=$(echo "$R" | 取 board)
要 "建得出看板" "$(echo "$BOARD" | cut -c1-2)" "01"

R=$(web "/api/projects/$PROJ/boards")
含 "看板清单上有它" "$R" "$BOARD"

R=$(web "/api/projects/$PROJ/boards/$BOARD")
要 "五行都在" "$(echo "$R" | 数 rows)" "5"
要 "来源标识留着（TBL-016）" "$(echo "$R" | 深取 rows.0.values.writtenBy.kind)" "person"

节 "⑤ 翻页 —— 查询串以前被直接扔掉"
# ⚠️ 这一段盯的是一个**不报错的缺陷**：一次给死 200 行、没有第二页，
# 一张 201 行的表在页面上就是少一行，看的人不会知道。
R=$(web "/api/projects/$PROJ/boards/$BOARD?limit=2")
要 "一页两行" "$(echo "$R" | 数 rows)" "2"
要 "说得出后面还有" "$(echo "$R" | 深取 has_more)" "true"
# **先排完序再切。** 先切再排会稳定地显示最老的那一批，而它同样不报错。
要 "倒序的第一页是最新的那几条" "$(echo "$R" | 深取 rows.0.values.title)" "第4条"

R=$(web "/api/projects/$PROJ/boards/$BOARD?limit=2&offset=4")
要 "最后一页只剩一行" "$(echo "$R" | 数 rows)" "1"
要 "到头了" "$(echo "$R" | 深取 has_more)" "false"
要 "说得出这一页从第几行起" "$(echo "$R" | 深取 offset)" "4"

# 上限是**平台的**，不是调用方的：没有它，?limit=99999999 是一次谁都发得出的自助拒绝服务。
R=$(web "/api/projects/$PROJ/boards/$BOARD?limit=99999999")
要 "要多少都给不动平台上限" "$(echo "$R" | 数 rows)" "5"
# 手打错的参数该看到第一页，不该看到一屏错误。
要 "打错的参数不炸" "$(web状态 "/api/projects/$PROJ/boards/$BOARD?limit=abc")" "200"

节 "⑥ 个人看板（NTF-001）—— 那条以前不存在的路由"
# 通知**只从事件派生**（`NTF-002`），所以这里造的是一个事件：
# 发起人自己表态不算数（`FLW-026③`）→ 那一行没被采纳 → 写的人收到通知。
ARGS="{\"project\":\"$PROJ\",\"table\":\"reviews\",\"columns\":[{\"name\":\"verdict\",\"type\":\"enum\",\"enumValues\":[\"过\",\"不过\"],\"required\":true}]}"
甲 table.create "$ARGS" > /dev/null
NODE='{"name":"初审","pass":[{"op":"equals","column":"verdict","value":"过"}],"writerRoles":["member","maintainer","owner"],"separationOfDuties":true}'
DEF="{\"project\":\"$PROJ\",\"name\":\"一步评审\",\"settlementTable\":\"reviews\",\"steps\":[[$NODE]]}"
FLOW=$(甲 flow.define "$DEF" | 取 flow)
ARGS="{\"project\":\"$PROJ\",\"flow\":\"$FLOW\",\"version\":1,\"subjectKind\":\"pr\",\"subjectId\":\"#4\"}"
INST=$(甲 flow.start "$ARGS" | 取 instance)
甲 flow.settle "{\"project\":\"$PROJ\",\"instance\":\"$INST\",\"values\":\"{\\\"verdict\\\":\\\"过\\\"}\"}" > /dev/null
sleep 1

# ⚠️ **不要假设收件箱是干净的。** 四条场景共用一个 daemon 与**同一个 alice**
# （`--issue-token alice@scenarios` 与 `XOPS_LOGIN` 是同一个账号，那是刻意的），
# 于是前面几条场景留下的通知也在这里。单跑 04 时是 1 条，连跑时是 8 条——
# **一条"期望 1"的断言会在单跑时绿、连跑时红**，而红的原因跟被测的东西无关。
# 按内容断言，不按条数。
R=$(web /api/me/notices)
要 "个人看板上最新的一条就是刚才那次（跨项目一起排，NTF-014）" \
   "$(echo "$R" | 深取 notices.0.project)" "$PROJ"
要 "是「我写的行未被采纳」那一类（NTF-007）" "$(echo "$R" | 深取 notices.0.kind)" "row-not-settled"
要 "没到上限就不说截断" "$(echo "$R" | 深取 truncated)" "false"

# `NTF-010`：读被**硬限定为 `user = 令牌持有人`**——行级可见性的唯一例外。
# ⚠️ 路径上根本没有 user 参数：**表达不出**"看别人的"，不是"表达得出但被拒绝"。
SAVED_A="$SESSION"
登录 "bob@scenarios" "$WEB_PW"
要 "换个人登得进来" "$LOGIN" "ok"
# ⚠️ 乙**自己也可能有通知**（03 那条场景里他就有）。要验的是
# "**看不见甲的那些**"，不是"一条都没有"——后者在连跑时是假的。
R=$(web /api/me/notices)
不含 "乙看不见甲这个项目的通知（NTF-010）" "$R" "$PROJ"
R=$(web /api/projects)
不含 "乙也看不见甲的项目（BRD-011）" "$R" "$PROJ"
要 "乙读甲的看板与不存在一致（PRJ-008）" "$(web状态 "/api/projects/$PROJ/boards/$BOARD")" "404"
SESSION="$SAVED_A"

节 "⑦ 深链回落到 index.html"
# 路由在前端那一侧，所以后端对认不出的路径要交回页面而不是 404。
# ⚠️ 这一条也顺带盯着"**二进制里到底有没有页面**"：`web/dist` 不在时
# `assets.rs` 只打一条 warning 就过去了，而那时深链回的是 404。
含 "/me 回的是页面" "$(web类型 /me)" "text/html"
含 "看板深链回的也是页面" "$(web类型 "/projects/$PROJ/boards/$BOARD")" "text/html"
# ⚠️ **`/api/` 下面认不出的路径也回落到页面**，因为回落是路由未命中之后的兜底，
# 不分前缀。它不是洞（读不到任何东西），但**客户端拿到的是 HTML 不是 JSON 错误**——
# 记在这里，免得下一个人把它当成"接口挂了"。
含 "认不出的 /api 路径也回落（不是 JSON 错误）" "$(web类型 /api/nope)" "text/html"

节 "⑧ 只读面上没有地方可写（BRD-005 ①）"
# **不是"有但不给 Web 用"，是不存在**——这一道是结构性的。
for M in POST PUT PATCH DELETE; do
  要 "$M 到看板上没有地方可发" "$(web发 "$M" "/api/projects/$PROJ/boards")" "404"
done
要 "POST /api/me 也一样" "$(web发 POST /api/me)" "404"
# 查询串**不参与路由匹配**——否则等于凭空多出一组没被 `ROUTES` 枚举过的路径，
# 而 `BRD-005` 第 ① 道靠的正是枚举那张表。
含 "带查询串照样命中原来那条" "$(web "/api/me?anything=1")" "alice@scenarios"
不含 "查询串救不回一条不存在的路由" "$(web "/api/nope?x=/api/me")" "alice@scenarios"

收工
