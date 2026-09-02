#!/usr/bin/env bash
# 本地仓（file://），全程不出网。
#
# ⚠️ **这个场景的重点是最后一段。** 绑定成功不说明什么 ——
# 备好的工作区曾经根本没交给引擎，agent 在 xopsd 自己的 cwd 里跑、
# 读的是 XOps 的源码。**它读到了东西，只是读错了地方，而且不报错。**
# 所以这里让技能去找一个只有那个仓里才有的字符串。
source "$(dirname "${BASH_SOURCE[0]}")/lib/mcp.sh"
SCENE="代码仓"

NEST="${SCENE_NEST:?}"
BARE="$NEST/仓/口令仓.git"
WORK="$NEST/仓/work"
# 只有那个仓里才有的字符串。**不能是任何一个会出现在 XOps 源码里的词。**
TOKENWORD="鲸鱼吃了七枚橄榄"

节 "① 造一个本地仓，推一笔提交，然后把它设成只读"
mkdir -p "$BARE" "$WORK"
git init -q --bare "$BARE"
git init -q "$WORK"
(
  cd "$WORK" || exit 1
  git config user.email scenarios@xops; git config user.name scenarios
  printf '# 口令\n\n%s\n' "$TOKENWORD" > 口令.md
  git add -A && git commit -qm "第一笔"
  git remote add origin "$BARE" && git push -q origin HEAD:refs/heads/main
)
REV=$(git -C "$WORK" rev-parse HEAD)
要 "推上去了" "$(echo "$REV" | wc -c | tr -d ' ')" "41"

# ⚠️ **本地仓的只读证明是问操作系统，不是推一次。**
# 实测：`git push --dry-run` 走 file:// 时，目标目录只读也返回 0 ——
# 远端那条路在本地是**静默失效**的。见 crates/xops-repo/src/local.rs。
PROJ=$(mcp project.create '{"slug":"t02","displayName":"代码仓场景"}' | 取 project)

节 "② 可写的仓绑不上（RPO-013）"
BAD=$(mcp repo.bind "{\"project\":\"$PROJ\",\"remote\":\"file://$BARE\"}")
含 "还能写就拒绝" "$BAD" "写得进去"

# 本地仓没有凭据可给：往一个专放密钥的字段里塞占位串，
# `repo.rotate` 会把那串垃圾当成一把真凭据去换。**在绑成功之前试**——
# 绑成功之后撞的是"已经绑过了"，那证明不了这一条。
WITHCRED=$(mcp repo.bind "{\"project\":\"$PROJ\",\"remote\":\"file://$BARE\",\"credential\":\"占位\"}")
含 "本地仓给了凭据要被拒" "$WITHCRED" "不要给凭据"

chmod -R a-w "$BARE"

节 "③ 只读之后绑得上"
BOUND=$(mcp repo.bind "{\"project\":\"$PROJ\",\"remote\":\"file://$BARE\"}")
要 "绑上了" "$(echo "$BOUND" | 取 remote)" "file://$BARE"
要 "记的平台是 local，不是 github" "$(echo "$BOUND" | 取 platform)" "local"

含 "本地仓没有凭据可换" "$(mcp repo.rotate "{\"project\":\"$PROJ\",\"credential\":\"新的\"}")" "没有凭据可换"

STATUS=$(mcp repo.status "{\"project\":\"$PROJ\"}")
要 "状态说得出绑了" "$(echo "$STATUS" | 取 bound)" "True"
不含 "状态里没有凭据的任何形态（RPO-003）" "$STATUS" "credential"

节 "④ 一个要读代码仓的技能"
SKILL=$(mcp skill.create "$(python3 - "$PROJ" <<'EOP'
import json, sys
print(json.dumps({"project": sys.argv[1], "name": "报工作目录",
  # 故意不让它调工具:这条要证的是「工作目录是不是那份工作区」，
  # 而不是「这个模型会不会用工具」——后者不该决定前者的成败。
  "content": "不要调用任何工具。把系统提示里 Working directory 那个路径原样写出来，"
             "只写这一行路径，别的什么都不要写。",
  "declaration": {"output": "report", "needsRepository": True,
                  "maxDurationMillis": 120000}}, ensure_ascii=False))
EOP
)" | 取 skill)
要 "建得出技能" "$(echo "$SKILL" | cut -c1-2)" "01"

# 备工作区那条链**两条路都要接**:正式触发一条，技能试跑一条。
# 少接一条的表现是"技能发布不了"——发布要一次成功的试跑，而试跑拿不到工作区。
TESTED=$(mcp skill.test "{\"project\":\"$PROJ\",\"skill\":\"$SKILL\",\"version\":1,\"inputs\":\"{}\"}")
要 "试跑也备得出工作区" "$(echo "$TESTED" | 取 succeeded)" "True"
[ "$(echo "$TESTED" | 取 succeeded)" = "True" ] || printf '     回话：%s\n' "$TESTED"

节 "⑤ 执行的工作目录是那份工作区 —— 这条断过"
# ⚠️ 断掉的表现**不是报错**:agent 照样跑，只是在 xopsd 自己的 cwd 里跑，
# 读的是 XOps 的源码。「读错了地方」和「读不到」是两件事，后者会喊，前者不会。
OUT=$(echo "$TESTED" | 取 output | tr -d ' ')
含 "工作目录在 XOPS_WORKSPACES 底下" "$OUT" "$XOPS_WORKSPACES/ws-"
不含 "不是 xopsd 自己的 cwd（XOps 的源码目录）" "$OUT" "Workspace/XOps"
case "$OUT" in *"$XOPS_WORKSPACES/ws-"*) ;; *) printf '     产出：%s\n' "$OUT" ;; esac

节 "⑥ 技能真的读得到仓里的文件 —— 这条也断过"
# ⚠️ 断掉的**同样不报错**:`Builder` 拿不到工具注册表时用一个空的，
# 于是每次请求里的 tools 是 `[]`，模型只好用自己的方式凑合——
# 把工具调用当文本吐出来，或者绕道解释"我没有 shell 工具"。
# **一次执行看着是成功的，产出里一个字有用的都没有。**
SKILL=$(mcp skill.create "$(python3 - "$PROJ" <<'EOP'
import json, sys
print(json.dumps({"project": sys.argv[1], "name": "找口令",
  "content": "用 Glob 看清楚有哪些文件，再用 Read 读 口令.md，"
             "把文件里那一行中文原样写出来，只写那一行。读不到就写「读不到」。",
  "declaration": {"output": "report", "needsRepository": True,
                  "maxDurationMillis": 180000}}, ensure_ascii=False))
EOP
)" | 取 skill)
READRUN=$(mcp skill.test "{\"project\":\"$PROJ\",\"skill\":\"$SKILL\",\"version\":1,\"inputs\":\"{}\"}")
要 "跑成了" "$(echo "$READRUN" | 取 succeeded)" "True"
OUT=$(echo "$READRUN" | 取 output)
含 "产出里有只有那个仓才有的口令" "$OUT" "$TOKENWORD"
不含 "工具调用没被当成文本吐出来" "$OUT" "DSML"
不含 "引擎的回合旁白不进产出" "$OUT" "used tools:"
case "$OUT" in *"$TOKENWORD"*) ;; *) printf '     产出：%s\n' "$OUT" ;; esac

收工
