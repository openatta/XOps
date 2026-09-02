# `TurnOutcome.usage` 只有最后一次 API 调用的用量

**状态**：待投递

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
