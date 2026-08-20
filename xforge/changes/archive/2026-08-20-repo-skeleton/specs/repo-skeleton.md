# 仓库骨架

## ADDED Requirements

### Requirement: SKEL-001 Workspace 容纳约定的模块布局

仓库以 pnpm workspace 组织，声明 `apps/*` 与 `packages/*` 两个包位置。新增一个符合该布局的包时，无需修改任何仓库根配置即被纳入工程基线的全部命令。

#### Scenario: 新增包被自动纳入

- **WHEN** 在 `packages/` 下新增一个仅含 `package.json` 与一个源文件的最小包，且不修改仓库根的任何文件
- **THEN** 在仓库根执行类型检查、lint 与测试三条命令时，该包的文件均被处理，且三条命令均成功

#### Scenario: 布局之外的目录不被纳入

- **WHEN** 在仓库根新增一个既不在 `apps/` 也不在 `packages/` 下的目录，其中含有一个源文件
- **THEN** 仓库根的三条命令不因该目录的存在而失败，也不把它当作 workspace 包处理

### Requirement: SKEL-002 三条统一命令

仓库根提供类型检查、lint、测试三条命令。每条都能在无任何交互、无本地预置状态的前提下执行完毕，并以退出码表达成败。

#### Scenario: 干净 clone 上三条命令通过

- **WHEN** 在一个全新 clone 的仓库中安装依赖后，依次执行类型检查、lint、测试三条命令
- **THEN** 三条命令均以退出码 0 结束

#### Scenario: 类型错误使命令失败

- **WHEN** 在任一 workspace 包中引入一处类型错误后执行类型检查命令
- **THEN** 该命令以非 0 退出码结束，并指出出错的文件与位置

#### Scenario: 失败的测试使命令失败

- **WHEN** 在任一 workspace 包中加入一个必然失败的测试后执行测试命令
- **THEN** 该命令以非 0 退出码结束，并指出失败的测试

#### Scenario: 命令不依赖交互或本地状态

- **WHEN** 在非交互式环境（无 TTY）中执行三条命令
- **THEN** 三条命令的行为与在交互式终端中一致，不等待任何输入

### Requirement: SKEL-003 Node 版本显式声明

仓库显式声明所需的 Node 主版本，且该声明与 XForge CLI 要求的运行时（Node 20 或更高）一致。

#### Scenario: 版本声明可被读取

- **WHEN** 检查仓库根的运行时版本声明
- **THEN** 声明存在，且其允许的最低 Node 主版本不低于 20

### Requirement: SKEL-004 CI 执行同一套命令

CI 在推送与 Pull Request 上执行与本地相同的三条命令。任一命令失败即判定该次 CI 失败。

#### Scenario: CI 在推送时运行

- **WHEN** 向仓库推送一个提交
- **THEN** CI 被触发，并执行类型检查、lint、测试三条命令

#### Scenario: CI 在 Pull Request 上运行

- **WHEN** opened 一个针对默认分支的 Pull Request
- **THEN** CI 被触发，并执行同样的三条命令

#### Scenario: 任一命令失败即 CI 失败

- **WHEN** 推送一个含有类型错误的提交
- **THEN** CI 判定为失败，且失败原因指向类型检查命令

### Requirement: SKEL-005 骨架不含业务内容

骨架不引入任何业务代码，也不引入任何产品运行时依赖。

#### Scenario: 不存在业务模块

- **WHEN** 检查本 Change 交付后的仓库内容
- **THEN** 不存在任何 `apps/*` 或 `packages/*` 包的实现，也不存在任何 MCP tool、HTTP 端点或数据模型定义

#### Scenario: 不引入运行时依赖

- **WHEN** 检查仓库根声明的依赖
- **THEN** 其中不含数据库客户端、对象存储客户端、容器运行时客户端，也不含 deepseek-harness

### Requirement: SKEL-006 构建与依赖产物不入库

依赖目录与构建产物被版本控制忽略。

#### Scenario: 安装与构建后工作区仍然干净

- **WHEN** 在干净 clone 上安装依赖并执行三条命令后检查版本控制状态
- **THEN** 没有未跟踪或已修改的文件
