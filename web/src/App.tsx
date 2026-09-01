/**
 * XOps 的只读前端。
 *
 * **看板上没有任何直接写库的按钮**（`BRD-005`）。要写就发一次 MCP 调用——
 * 所以需要动手的地方，页面给的是"在你的 Agent 里跑这条命令"，而不是一个按钮。
 */

import { useCallback, useEffect, useState } from 'react'

import { MarkdownView } from './MarkdownView'
import { api, ApiError } from './api'
import type { BoardSummary, BoardView, Identity, Project, RowHistory, Settlement } from './api'
import { login, logout } from './session'

function useAsync<T>(load: () => Promise<T>, deps: unknown[]): [T | null, string | null] {
  const [value, setValue] = useState<T | null>(null)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    let alive = true
    setError(null)
    load()
      .then((result) => alive && setValue(result))
      .catch((cause: unknown) => {
        if (!alive) return
        setValue(null)
        setError(cause instanceof ApiError ? cause.message : '读不到')
      })
    return () => {
      alive = false
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps)
  return [value, error]
}

function Login({ onDone }: { onDone: () => void }) {
  const [account, setAccount] = useState('')
  const [secret, setSecret] = useState('')
  const [failed, setFailed] = useState(false)

  return (
    <form
      className="login"
      onSubmit={(event) => {
        event.preventDefault()
        login(account, secret).then(onDone).catch(() => setFailed(true))
      }}
    >
      <h1>XOps</h1>
      <label>
        账号
        <input value={account} onChange={(event) => setAccount(event.target.value)} />
      </label>
      <label>
        口令
        <input
          type="password"
          value={secret}
          onChange={(event) => setSecret(event.target.value)}
        />
      </label>
      <button type="submit">登录</button>
      {failed && <p className="error">凭证不对</p>}
    </form>
  )
}

/** 需要动手的地方给命令，不给按钮。 */
function AgentHint({ command }: { command: string }) {
  return (
    <aside className="hint">
      <p>看板是只读的。要改，在你的 Agent 里跑：</p>
      <pre>
        <code>{command}</code>
      </pre>
    </aside>
  )
}

function RowDetail({
  project,
  table,
  row,
  onClose,
}: {
  project: string
  table: string
  row: string
  onClose: () => void
}) {
  const [history] = useAsync<RowHistory>(
    () => api.history(project, table, row),
    [project, table, row],
  )
  const instance = history?.versions.at(-1)?.values['_instance']
  const [settlements] = useAsync<{ settlements: Settlement[] }>(
    () =>
      typeof instance === 'string'
        ? api.settlements(project, table, instance)
        : Promise.resolve({ settlements: [] }),
    [project, table, instance],
  )

  return (
    <section className="detail">
      <header>
        <h2>{row}</h2>
        <button type="button" onClick={onClose}>
          关闭
        </button>
      </header>

      {/* BRD-006 的前一半：状态怎么变的、谁改的、什么时候。 */}
      <h3>单行历史</h3>
      <ol className="history">
        {history?.versions.map((version) => (
          <li key={version.seq}>
            <span className="op">{version.op}</span>
            <time>{new Date(version.at).toLocaleString()}</time>
            <span className="who">{describeWriter(version.written_by)}</span>
            <pre>
              <code>{JSON.stringify(version.values, null, 2)}</code>
            </pre>
          </li>
        ))}
      </ol>

      {/* BRD-006 的后一半：为什么这么变、谁表的态。**两次查询，前端也不 join。** */}
      <h3>结算行</h3>
      {settlements?.settlements.length ? (
        <ol className="settlements">
          {settlements.settlements.map((settlement) => (
            <li key={settlement.row}>
              <span className="who">{describeWriter(settlement.written_by)}</span>
              <pre>
                <code>{JSON.stringify(settlement.values, null, 2)}</code>
              </pre>
            </li>
          ))}
        </ol>
      ) : (
        <p className="empty">这一行还不属于任何流程实例。</p>
      )}
    </section>
  )
}

/** `TBL-016`：来源标识读的就是 `writtenBy`。 */
function describeWriter(written: Record<string, unknown> | null | undefined): string {
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

function isLongText(value: unknown): value is string {
  return typeof value === 'string' && (value.includes('\n') || value.length > 120)
}

function BoardTable({ project, view }: { project: string; view: BoardView }) {
  const [open, setOpen] = useState<string | null>(null)
  const [expanded, setExpanded] = useState<string | null>(null)

  return (
    <>
      <table>
        <thead>
          <tr>
            <th>来源</th>
            {view.columns.map((column) => (
              <th key={column}>{column}</th>
            ))}
          </tr>
        </thead>
        <tbody>
          {view.rows.map((row) => (
            <tr key={row.row}>
              <td className="who">
                {describeWriter(row.values['writtenBy'] as Record<string, unknown> | null)}
              </td>
              {view.columns.map((column) => {
                const value = row.values[column]
                return (
                  <td key={column}>
                    {isLongText(value) ? (
                      <>
                        {expanded === `${row.row}:${column}` ? (
                          <MarkdownView source={value} />
                        ) : (
                          <span>{value.slice(0, 60)}…</span>
                        )}
                        <button
                          type="button"
                          onClick={() =>
                            setExpanded(
                              expanded === `${row.row}:${column}`
                                ? null
                                : `${row.row}:${column}`,
                            )
                          }
                        >
                          {expanded === `${row.row}:${column}` ? '收起' : '展开'}
                        </button>
                        {/* BRD-010：不信任渲染的人可以直接看原文。 */}
                        <a href={api.rawUrl(project, view.table, row.row, column)} download>
                          原文
                        </a>
                      </>
                    ) : (
                      String(value ?? '')
                    )}
                  </td>
                )
              })}
              <td>
                <button type="button" onClick={() => setOpen(row.row)}>
                  历史
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {open && (
        <RowDetail
          project={project}
          table={view.table}
          row={open}
          onClose={() => setOpen(null)}
        />
      )}
    </>
  )
}

export function App() {
  const [signedIn, setSignedIn] = useState(true)
  const [project, setProject] = useState<string | null>(null)
  const [board, setBoard] = useState<string | null>(null)

  const [me, meError] = useAsync<Identity>(() => api.me(), [signedIn])
  const [projects] = useAsync<{ projects: Project[] }>(() => api.projects(), [signedIn])
  const [boards] = useAsync<{ boards: BoardSummary[] }>(
    () => (project ? api.boards(project) : Promise.resolve({ boards: [] })),
    [project],
  )
  const [view] = useAsync<BoardView | null>(
    () => (project && board ? api.board(project, board) : Promise.resolve(null)),
    [project, board],
  )

  const signOut = useCallback(() => {
    logout().then(() => setSignedIn(false))
  }, [])

  if (meError || !me) {
    return <Login onDone={() => setSignedIn(true)} />
  }

  return (
    <div className="app">
      <header className="top">
        {/* BRD-011：明确展示当前用户身份。 */}
        <span className="me">
          {me.display_name}（{me.provider}/{me.account}）
        </span>
        <button type="button" onClick={signOut}>
          注销
        </button>
      </header>

      <nav>
        <h2>项目</h2>
        <ul>
          {projects?.projects.map((entry) => (
            <li key={entry.project}>
              <button type="button" onClick={() => { setProject(entry.project); setBoard(null) }}>
                {entry.display_name}
                <small>
                  {entry.slug} · {entry.role}
                  {entry.archived && ' · 已归档'}
                </small>
              </button>
            </li>
          ))}
        </ul>

        {project && (
          <>
            <h2>看板</h2>
            <ul>
              {boards?.boards.map((entry) => (
                <li key={entry.board}>
                  <button type="button" onClick={() => setBoard(entry.board)}>
                    {entry.name}
                    <small>{entry.table}</small>
                  </button>
                </li>
              ))}
            </ul>
            {boards?.boards.length === 0 && (
              <AgentHint command={`board.define project=${project} table=… name=…`} />
            )}
          </>
        )}
      </nav>

      <main>
        {view && project ? (
          <>
            <h1>{view.name}</h1>
            <BoardTable project={project} view={view} />
            <AgentHint command={`row.${view.table}.insert project=${project} …`} />
          </>
        ) : (
          <p className="empty">选一个看板。</p>
        )}
      </main>
    </div>
  )
}
