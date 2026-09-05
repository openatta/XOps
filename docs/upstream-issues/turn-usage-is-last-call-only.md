# `TurnOutcome.usage` 只有最后一次 API 调用的用量

**状态**：**已解决**（上游 `v0.2.5`，2026-09-04 跟版）。
投递日 2026-09-02，openatta/AttaCore。链接待补——
拿到之后补在这一行，**不要删这个文件**：删掉之后下一个人会重新发现同一件事，
而这一份现在还多担一件事——它记着 `usage` 那个字段**至今仍然是最后一次**。

上游选了下面「想要的」里的第一条：**加一个累计字段**，
`TurnOutcome.total_usage`，`usage` 一个字没改（上游的原话：
"It is not what its name suggests and never was"，保留它只为不打断已经在读它的宿主）。
⚠️ **所以这个坑还在**——按名字读 `usage` 的人照样会掉进去。

XOps 这边的三步都做完了：

```text
子模块跟版本                      v0.2.0 → v0.2.5
tokens 改成读累计字段             xops_exec::embedded::tokens，读 total_usage
                                  ⚠️ 而且四项都加：input 不含命中缓存的部分，
                                  而 SkillScene 的系统提示是 cached 发的
删掉横幅里 engine_gaps 那一条      IsolationLevel::engine_gaps() 现在是空表
                                  （有测试往里塞一条假的，验这条路没烂掉）
```

有两条测试盯着它不再退回去：`一个回合来回几趟就把几趟都算进用量`（读 `usage`
会得到 18，读累计得到 138）与 `命中缓存的那部分也算进用量`。

---

以下是当初投出去的那份，**原样保留**。

## 现象

一个回合里模型可能来回好几趟（`TurnOutcome.api_calls` 会是 3、4），
但 `TurnOutcome.usage` 拿到的是 `turn.rs` 里那个每轮被重新赋值的
`let usage = stream_result.usage;`——**最后一趟的用量**，不是累计。

实测：同一个技能对同一个仓跑两次，一次报 2817 token，一次报 365。
两次做的是同样的活。

## 为什么它不只是个显示问题

XOps 用这个数做**单次执行的 token 预算**（`TSK-005`）。少算意味着预算咬不住：
一次跑了 50k 的执行可能报 400，而上限是按报出来的数比的。
**一个看着像真数、实际少算的预算，比没有预算更糟**——
没有预算至少不会有人以为它在管事。

## 宿主这一侧为什么补不上

- `AgentEvent` 里只有 `TurnComplete` 带 `usage`，而它带的就是同一个"最后一次"。
  没有"每次 API 调用完成"这样的事件。
- `ModelInterceptor::on_message` 拿到的是 `ModelMessage`（role + content），
  里面没有 usage。
- 包一层 `Model` 装饰器能数到每次调用，但**归不到某一次执行头上**：
  一个引擎实例服务多个并发会话，而模型调用那一层看不到 session id。

`turn.rs` 里其实已经有一个累计量（`total_tokens_used += usage.input_tokens as u64
+ usage.output_tokens as u64`），只是没有出现在 `TurnOutcome` 上。

## 想要的

`TurnOutcome` 上多一个累计字段，或者把现在这个字段改成累计。

前者不打断任何人；后者更符合这个字段名字给人的印象，
但会**悄悄改变**已经在读它的宿主的行为——那种改动最好有个明确的版本边界。
