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

### Element: sql:table.kv
- module: xops-store
- consumers: [xops-store]
- 物理 schema **只有一张表**：`kv(space TEXT, key BLOB, value BLOB, PRIMARY KEY(space, key)) WITHOUT ROWID`。
- 业务上的"表"不是数据库里的表——它是键的前缀。换库时要建的东西因此只有这一张。
- `WITHOUT ROWID` 是**性能选择不是能力依赖**：去掉它一切照常工作。

### Element: sql:table.kv.column.space
- module: xops-store
- consumers: [xops-store]

### Element: sql:table.kv.column.key
- module: xops-store
- consumers: [xops-store]

### Element: sql:table.kv.column.value
- module: xops-store
- consumers: [xops-store]

### Element: sql:layout.event-key
- module: xops-store
- consumers: [全部]
- `event` 空间：`表名 \0 序号(8 字节大端)`。按键升序扫就是按序号升序读。

### Element: sql:layout.row-key
- module: xops-store
- consumers: [全部]
- `row` 空间：`表名 \0 行 ID(16 字节)`。存的是投影，**软删是墓碑不是删键**。

### Element: sql:layout.watermark
- module: xops-store
- consumers: [xops-store]
- `meta` 空间两个水位：`seq`（事件写到第几条）与 `applied`（投影放到第几条）。
- **`applied` 可以落后于 `seq`，那正是它存在的理由**：没有事务，事件与投影之间会崩。
  补法是重放不是回滚——**事件是真相，投影是它的缓存**。区间开始时对锁集合里每张表修一次，
  正常情况下只多花一次 `get`。

### Element: sql:meta.column-type.text
- module: xops-table
- 短文本，默认上限 512 字符。

### Element: sql:meta.column-type.long-text
- module: xops-table
- Markdown 正文这类，默认上限 256 KiB。**超限拒绝，不截断**（`MCP-014`）。

### Element: sql:meta.column-type.integer
- module: xops-table

### Element: sql:meta.column-type.decimal
- module: xops-table

### Element: sql:meta.column-type.bool
- module: xops-table

### Element: sql:meta.column-type.timestamp
- module: xops-table
- UTC 毫秒。**存的是数字**，不是日期字符串——换库时不必关心日期方言。

### Element: sql:meta.column-type.enum
- module: xops-table
- 取值集合**由用户声明**。

### Element: sql:meta.column-type.sequence
- module: xops-table
- 自增序号：**项目内、每表独立，不跨项目共享计数器**（`TBL-018`）。
- 在写入区间内取号（`PreWrite`），所以两个并发写不会撞。

### Element: sql:meta.column-type.row-ref
- module: xops-table
- 存另一张表的行 ID。**平台不校验、不级联**（`TBL-019`、`TBL-023`）。

### Element: sql:meta.column-type.blob
- module: xops-table
- 二进制，存 base64 文本，默认上限 4 MiB。

### Element: sql:meta.column-type.derived
- module: xops-table
- 派生文本：模板只认 `{project.slug}` 与 `{<同一行的列>}`。
  **insert 时生成一次、之后不变**（`TBL-020`）；派生列不能引用另一个派生列。

### Element: sql:meta.auto-column.writtenBy
- module: xops-table
- 四种自包含取值（`TBL-015`）。**任何列声明都不能覆盖它**；参数里带的一律被盖掉（`I-B`）。

### Element: sql:meta.auto-column.at
- module: xops-table
- 写入时刻，从 `Clock` 来。

### Element: sql:meta.auto-column.revision
- module: xops-table
- 读的哪个代码修订。**跟着 `writtenBy` 的执行那一类进来**，不单独声明。

### Element: sql:meta.auto-column._instance
- module: xops-table
- 这一行属于哪个流程实例。**位在这里，值由 RP-15 填**。

### Element: sql:meta.auto-column.retainUntil
- module: xops-table
- 这一行什么时候到期。**位在这里，值由 RP-12 填**。

### Element: sql:meta.projection.physical-table-name
- module: xops-table
- 业务表 `(项目, 名字)` → 物理表名 `p<项目>.<名字>`；全局表就是它自己的名字。
- **业务上的"表"不是数据库里的表**——它是键的一段前缀，所以两个项目各建一张 `bugs`
  互不相干，而 RP-01 的表锁照样是"一张业务表一把锁"。

### Element: sql:meta.projection.sequence-counter
- module: xops-table
- 空间 `table-seq`，键 `物理表名 \0 列名` → i64。**只在写入区间内读改**。

### Element: sql:table._tables
- module: xops-table
- 表目录落在这张平台表上。行标识由 `(项目, 表名)` 定死，所以**同一张表的每次 schema 变更
  都落在同一行上**——它的单行历史就是这张表的 schema 变更史。
- 它**不是那五张系统表之一**，用户看不到它，也不参与建表、看板与专属 tool 的派发。

### Element: sql:table._plugins
- module: xops-table
- 每个插件的每个版本一行（`TBL-009`）。项目级系统表，**只有平台能写**。
- 列：`plugin` · `version` · `state`（候选/已安装/已停用）· `position`（流转/输出）·
  `entry` · `source` · `capabilities` · `tests` · `testResult` · `generatedBy` ·
  `installedBy` · `installedAt`。
- **`position` `entry` `capabilities` 三列是 D59 补的**：`TBL-009` 原来的列表写在 D52 之前，
  那时插件"与平台同权限"，既没有能力声明也没有"载体入口"这个概念。D52 之后这三样
  都成了这一行必须自己回答的事——`PLG-009`（能力声明是版本的一部分）·
  `I-T`（它对成员可读）· `RET-009`（`generatedBy` 指向的 `_runs` 行会到期被清理，
  所以这一行必须自包含）。**少一列就有一条要求落不了地。**
- ⚠️ **插件配置不在这张表里，所以它没有对应的列元素**（`PLG-015`）。
  它加密后落在 KV 的一个非行空间上，**是全系统唯一一份不以表的形式存在的状态**（`I-A`）。
