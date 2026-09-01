# 上游依赖：AttaCore 的 `exec` 宿主注入位

> 状态：**已起草，未提交**。裸跑的决定（2026-09-01）把它的紧迫性拿掉了——
> 现在提，等于往对方的 tracker 上放一条我们自己不打算用的东西。
> **接容器后端那一天，它是第一件要做的事。**
>
> 上游仓：`github.com/openatta/AttaCore` · 本仓落点：`crates/xops-exec` · 需求：`EXE-029`（D51）

## 为什么记在这里

**我们不改 AttaCore 的代码。** 需求变更走 ISSUE 同步过去，直接改会被那边清理掉。
所以这份东西的正确形态是"一份随时可以提出去的草稿"，而不是一个补丁。

## 上游现状（读代码得到的，不是猜的）

AttaCore 的四个执行契约已经完备，且登记进了它的可替换扩展点索引：

```text
exec.process      声明用途原文：which machine the work happens on
exec.filesystem
exec.network
exec.sandbox
```

信任级别 **host-only**，全部内建工具都经它们碰机器。

**缺的只有一样：Builder 上没有 `exec` 这个宿主注入位。** 那类注入位它已经有十六个，
加一个是有先例的改动。

## 要提的那条 ISSUE（草稿）

> **标题**：Builder 缺少 `exec` 宿主注入位，四个 `exec.*` 扩展点因此无法从宿主侧替换
>
> **正文**：
>
> `exec.process` / `exec.filesystem` / `exec.network` / `exec.sandbox` 四个契约已经完备，
> 也已登记为 host-only 扩展点，但 Builder 上没有对应的注入位——宿主没有办法把自己的
> 实现装进去。目前那十六个注入位覆盖了别的扩展点，`exec` 这一族是空的。
>
> 需要的形状与既有注入位一致：Builder 上一个 `with_exec(...)`（或同族命名），
> 接收四个契约的实现，daemon 在装配 session 时使用它。
>
> **用途**：XOps 想用一次性容器承载每一次执行——容器创建、资源限制、网络策略、挂载、
> 完整销毁——而这四件事正好是那四个契约的内容。没有注入位，容器后端就只能靠 fork
> 或者绕过引擎，两条都会让"XOps 与引擎只有一条执行契约"这个接缝消失。
>
> **不需要的**：不需要 AttaCore 自己实现容器，也不需要它知道容器是什么。
> 只要一个注入位。

## 现在为什么可以先不提

XOps 这一侧当前是**裸跑**（`crates/xops-exec/src/provider.rs` 的 `IsolationLevel::Bare`），
容器后端没有实现，因此这个注入位暂时用不上。

裸跑没兑现的需求**逐条列在代码里**（`IsolationLevel::unsatisfied`），并且有测试盯着那张表——
这是 `EXE-029` 那句"沙箱兑现不了的逐条如实上报，绝不当作已兑现"的落法。
**接容器那天，那张表要缩短，而缩短之前的第一步就是把上面这条 ISSUE 提出去。**
