/**
 * 几个到处都用的小件。
 */

import { intercept } from './router'

/**
 * 站内链接。**是 `<a href>`，不是 `<button onClick>`**——理由写在 `router.ts` 的
 * `intercept` 上：中键、复制链接、发给别人，是看板这种东西被用的常态。
 */
export function Link({
  to,
  children,
  className,
}: {
  to: string
  children: React.ReactNode
  className?: string
}) {
  return (
    <a href={to} className={className} onClick={(event) => intercept(event, to)}>
      {children}
    </a>
  )
}

/**
 * 需要动手的地方给命令，不给按钮（`BRD-005`）。
 *
 * ⚠️ **这不是一种委婉的说法。** 前端里根本不存在能发出写请求的代码路径，
 * `scripts/frontend-discipline.mjs` 枚举全部源码盯着这件事。
 */
export function AgentHint({ command, what }: { command: string; what?: string }) {
  return (
    <aside className="hint">
      <p>{what ?? '看板是只读的。要改，在你的 Agent 里跑：'}</p>
      <pre>
        <code>{command}</code>
      </pre>
    </aside>
  )
}

/** `TBL-016`：来源标识读的就是 `writtenBy`。 */
export function describeWriter(written: Record<string, unknown> | null | undefined): string {
  if (!written) return '未知'
  const kind = written['kind']
  switch (kind) {
    case 'person':
      return `人 ${String(written['user'] ?? '')}`
    case 'execution':
      // 不可信内容不是一个额外的标记位，是 writtenBy 的自然结果。
      return `执行（模型产出，内容不可信）任务所有者 ${String(written['task_owner'] ?? '')}`
    case 'plugin':
      return `插件 ${String(written['plugin'] ?? '')}`
    case 'platform':
      return '平台'
    default:
      return String(kind ?? '未知')
  }
}

export function Loading() {
  return <p className="empty">读取中…</p>
}

export function Problem({ error }: { error: string }) {
  return <p className="error">{error}</p>
}
