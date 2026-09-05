/**
 * 一个看板：一张表的一个视图（`BRD-001`）。
 *
 * 单行历史与结算行是**两个视图、两次查询**——平台不做 join（`BRD-006`、`TBL-023`）。
 */

import { useEffect, useState } from 'react'

import { MarkdownView } from '../MarkdownView'
import { api } from '../api'
import type { BoardView, RowHistory, Settlement } from '../api'
import { useAsync } from '../useAsync'
import { AgentHint, Loading, Problem, describeWriter } from '../shared'

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
  const history = useAsync<RowHistory>(() => api.history(project, table, row), [
    project,
    table,
    row,
  ])
  const instance = history.value?.versions.at(-1)?.values['_instance']
  const settlements = useAsync<{ settlements: Settlement[] }>(
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
      {history.error && <Problem error={history.error} />}
      <ol className="history">
        {history.value?.versions.map((version) => (
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
      {settlements.value?.settlements.length ? (
        <ol className="settlements">
          {settlements.value.settlements.map((settlement) => (
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

function isLongText(value: unknown): value is string {
  return typeof value === 'string' && (value.includes('\n') || value.length > 120)
}

export function BoardTable({ project, view }: { project: string; view: BoardView }) {
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
            <th />
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
                              expanded === `${row.row}:${column}` ? null : `${row.row}:${column}`,
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
        <RowDetail project={project} table={view.table} row={open} onClose={() => setOpen(null)} />
      )}
    </>
  )
}

export function BoardPage({ project, board }: { project: string; board: string }) {
  // 翻页的位置**不进地址栏**：一条看板链接指的是"这个看板"，不是"这个看板的第三页"。
  // 换看板要把它归零 —— 不归零的话，从一张长表切到一张短表会看到一片空白，
  // ⚠️ **而它不报错**。
  const [offset, setOffset] = useState(0)
  useEffect(() => setOffset(0), [project, board])
  const view = useAsync<BoardView>(() => api.board(project, board, offset), [
    project,
    board,
    offset,
  ])

  if (view.loading) return <Loading />
  if (view.error) return <Problem error={view.error} />
  if (!view.value) return <p className="empty">没有这个看板。</p>
  const page = view.value

  return (
    <>
      <h1>{page.name}</h1>
      <p className="who">表 {page.table}</p>
      <BoardTable project={project} view={page} />
      {(page.offset > 0 || page.has_more) && (
        <nav className="pager">
          <button
            type="button"
            disabled={page.offset === 0}
            onClick={() => setOffset(Math.max(0, page.offset - page.rows.length))}
          >
            上一页
          </button>
          {/* ⚠️ 这里显示的是**第几行起**，不是"共几页"——后端没给总数，前端也不算。 */}
          <span className="who">第 {page.offset + 1} 行起</span>
          <button
            type="button"
            disabled={!page.has_more}
            onClick={() => setOffset(page.offset + page.rows.length)}
          >
            下一页
          </button>
        </nav>
      )}
      <AgentHint command={`row.${page.table}.insert project=${project} …`} />
    </>
  )
}
