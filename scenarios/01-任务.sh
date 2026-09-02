#!/usr/bin/env bash
# 使用者的六步，代码仓那步先不进来（那是 02）。
#
# ⚠️ 变量名一律 ASCII —— 见 lib/mcp.sh 顶上那段。
source "$(dirname "${BASH_SOURCE[0]}")/lib/mcp.sh"
SCENE="任务"

节 "① 建项目"
PROJ=$(mcp project.create '{"slug":"t01","displayName":"任务场景"}' | 取 project)
要 "建得出项目" "$(echo "$PROJ" | cut -c1-2)" "01"

节 "② 建技能"
SKILL=$(mcp skill.create "$(python3 - "$PROJ" <<'PY'
import json, sys
print(json.dumps({"project": sys.argv[1], "name": "加法",
  "content": "把输入里的 a 和 b 相加。只回复那个数字，不要任何解释、不要标点。",
  "declaration": {"inputs": [{"name": "a", "type": "integer", "required": True},
                             {"name": "b", "type": "integer", "required": True}],
                  "output": "report", "maxDurationMillis": 60000}}, ensure_ascii=False))
PY
)" | 取 skill)
要 "建得出技能" "$(echo "$SKILL" | cut -c1-2)" "01"

# SKL-003：没有一次成功的测试执行就发布不了。**这条挡的是"上传即可用"。**
EARLY=$(mcp skill.publish "{\"project\":\"$PROJ\",\"skill\":\"$SKILL\",\"version\":1}")
含 "没测过就发不了（SKL-003）" "$EARLY" "error"

节 "③ 试跑 —— 真的模型调用"
TESTED=$(mcp skill.test "{\"project\":\"$PROJ\",\"skill\":\"$SKILL\",\"version\":1,\"inputs\":\"{\\\"a\\\":17,\\\"b\\\":25}\"}")
要 "试跑成功" "$(echo "$TESTED" | 取 succeeded)" "True"
含 "真模型算对了 17+25" "$(echo "$TESTED" | 取 output)" "42"

PUBLISHED=$(mcp skill.publish "{\"project\":\"$PROJ\",\"skill\":\"$SKILL\",\"version\":1}")
要 "测过之后发得出去" "$(echo "$PUBLISHED" | 取 state)" "published"

节 "④ 建任务并触发"
TASK=$(mcp task.create "{\"project\":\"$PROJ\",\"name\":\"日常加法\",\"skill\":\"$SKILL\",\"skillVersion\":1,\"inputs\":\"{\\\"a\\\":100,\\\"b\\\":23}\"}" | 取 task)
要 "建得出任务" "$(echo "$TASK" | cut -c1-2)" "01"

TRIG=$(mcp run.trigger "{\"project\":\"$PROJ\",\"task\":\"$TASK\"}")
RUN=$(echo "$TRIG" | 取 run)
# EXE-021：返回的是"进了队列"，不是"跑完了"。
要 "触发立刻返回（EXE-021）" "$(echo "$TRIG" | 取 accepted)" "True"
要 "回话里有执行标识" "$(echo "$RUN" | cut -c1-2)" "01"

节 "⑤ 看结果"
ST=""
i=0; while [ $i -lt 90 ]; do
  ST=$(mcp run.status "{\"project\":\"$PROJ\",\"run\":\"$RUN\"}" | 取 status)
  [ "$ST" = "succeeded" ] || [ "$ST" = "failed" ] && break
  sleep 1; i=$((i+1))
done
要 "执行成功了" "$ST" "succeeded"

# ⚠️ **落账**：执行跑完之后，谁把 `_runs` 那一行写下来。触发那条路非阻塞，
# 没有别人在等着做这件事 —— 这条断过一次，表现是"执行成功了，账上什么也没有"。
ROWS='{"rows":[]}'
i=0; while [ $i -lt 40 ]; do
  ROWS=$(mcp row.sys-runs.select "{\"project\":\"$PROJ\"}")
  N=$(echo "$ROWS" | python3 -c 'import sys,json;print(len(json.load(sys.stdin).get("rows",[])))')
  [ "$N" -gt 0 ] && break
  sleep 1; i=$((i+1))
done
LANDED=$(echo "$ROWS" | python3 -c '
import sys, json
rows = json.load(sys.stdin).get("rows", [])
print(json.dumps(rows[0]["values"], ensure_ascii=False) if rows else "{}")')
含 "_runs 上有这一行（EXE-026）" "$LANDED" '"status": "succeeded"'
含 "产出正文落下来了" "$LANDED" "123"

TRACE=$(echo "$LANDED" | 取 trace)
# 过程记录曾经是七十几行字面量 `event` —— 不报错、不为空、什么也没说。
不含 "过程记录不是一串 event（EXE-022）" "$TRACE" "event
event"
含 "过程记录里有回合的结论" "$TRACE" "turn-complete"

# TSK-016：一个静默被跳过的任务，会让人以为它在跑。
HIST=$(mcp run.trigger-history "{\"project\":\"$PROJ\",\"task\":\"$TASK\"}")
含 "触发留了痕（TSK-016）" "$HIST" "$RUN"

节 "⑥ 并发上限（EXE-027）"
SLOW=$(mcp skill.create "$(python3 - "$PROJ" <<'PY'
import json, sys
print(json.dumps({"project": sys.argv[1], "name": "数数",
  "content": "把 1 到 40 每个数字单独占一行输出，什么都不要多说。",
  "declaration": {"output": "report", "maxDurationMillis": 120000}}, ensure_ascii=False))
PY
)" | 取 skill)
mcp skill.test "{\"project\":\"$PROJ\",\"skill\":\"$SLOW\",\"version\":1,\"inputs\":\"{}\"}" >/dev/null
mcp skill.publish "{\"project\":\"$PROJ\",\"skill\":\"$SLOW\",\"version\":1}" >/dev/null
PRESS=$(mcp task.create "{\"project\":\"$PROJ\",\"name\":\"压\",\"skill\":\"$SLOW\",\"skillVersion\":1,\"overlap\":\"queue\"}" | 取 task)

RES=$(for _ in 1 2 3 4 5 6 7 8; do
  ( mcp run.trigger "{\"project\":\"$PROJ\",\"task\":\"$PRESS\"}" | 取 accepted ) &
done; wait)
要 "项目级上限放行 4 个" "$(echo "$RES" | grep -c True)" "4"
要 "其余 4 个被挡（不是报错，是跳过）" "$(echo "$RES" | grep -c False)" "4"

# 名额**析构即归还**。忘了归还是这类代码最常见的漏，
# 表现是"第一批跑完之后平台就再也接不了活"。
i=0; while [ $i -lt 120 ]; do
  N=$(mcp row.sys-runs.select "{\"project\":\"$PROJ\"}" | python3 -c 'import sys,json;print(len(json.load(sys.stdin).get("rows",[])))')
  [ "$N" -ge 5 ] && break
  sleep 1; i=$((i+1))
done
要 "落账之后名额还回来了" "$(mcp run.trigger "{\"project\":\"$PROJ\",\"task\":\"$PRESS\"}" | 取 accepted)" "True"

收工
