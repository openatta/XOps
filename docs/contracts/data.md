# 数据库

## Purpose

两截东西，缺一截就漏：

```
物理 schema   固定、很小：**一张键值表**，事件 · 投影 · 水位各占一个键空间
              也是 CON-012 唯一可被枚举检查的地方 ——
              没有外键、没有触发器、没有 JSON 列、没有存储过程，看得见就能查
元 schema     用户表的规则：11 种列类型 → 物理映射 · TBL-014 的自动补列 ·
              加列可以、改类型删列不做（TBL-022）
```

⚠️ **业务上的"表"不是数据库里的表。** 物理 schema 只有一张键值表 `kv`，业务表是键的前缀 ——
所以 `sql:table.bugs` 这种东西不存在；`sql:table._runs` 记的是那张系统表**作为业务表**的列，
不是一句 `CREATE TABLE`。

⚠️ **用户建的表一张都不该出现在这份记录里。** 它们是运行时产物，被批准的是
"用户能造出什么形状的表"，不是"现在有哪些表"。

## 谁往这里加元素

| 前缀 | 归属 |
|---|---|
| `sql:table.kv*` `sql:layout.*`（物理 schema：唯一那张键值表 · 键编码 · 水位） | RP-01 |
| `sql:table._*`（五张系统表**作为业务表**的形状） | 各系统表的主包，见下 |
| `sql:meta.column-type.*` `sql:meta.auto-column.*` `sql:meta.projection.*` | RP-04 |
| `sql:table._runs.column.retainUntil` 等保留期列位 | RP-12 |
| `sql:table._flows.*` `sql:table._flow_nodes.*` | RP-14 |
| `sql:table._plugins.*` | RP-16 |
| `sql:table._notices.*` | RP-17 |

## 命名

```
sql:table.<表>                       一张表
sql:table.<表>.column.<列>           一列
sql:layout.<名>                      键空间的编码约定（RP-01：event-key · row-key · watermark）
sql:meta.column-type.<类型>          用户表可用的一种列类型（TBL-018 那一组）
sql:meta.auto-column.<列位>          平台自动补的列位（TBL-014：writtenBy / at /
                                     revision / _instance / retainUntil）
sql:meta.projection.<规则>           事件到当前视图的投影规则
```

方言文件（有实现之后）：`data/schema.sql`（DDL，由 sqlite schema dump 自证）
与 `data/meta-schema.md`（元 schema，散文 + 映射表）。

## Elements

(none)
