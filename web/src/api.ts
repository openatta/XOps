/**
 * 后端的只读面。
 *
 * **这个文件里只有 GET。** `BRD-005` 的第 ② 道说的是"前端不存在调用写接口的代码路径"，
 * 而它兑现的方式不是靠自觉：写操作的唯一出口在 `session.ts`（两个凭据类端点），
 * 别处出现任何非 GET 的 `fetch`，`scripts/no-write-calls.mjs` 会在 `npm run check` 时拦下来。
 *
 * ⚠️ **顺序不能反**：第 ① 道（后端不存在写路由）已经在 RP-05 那边了。
 * 只有 ② 没有 ①，等于把一条安全属性交给前端自觉。
 */

export type Identity = {
  user: string
  display_name: string
  provider: string
  account: string
}

export type Project = {
  project: string
  slug: string
  display_name: string
  role: string
  archived: boolean
}

export type BoardSummary = { board: string; name: string; table: string }

export type Row = { row: string; values: Record<string, unknown> }

export type BoardView = {
  board: string
  name: string
  table: string
  columns: string[]
  rows: Row[]
}

export type Version = {
  seq: number
  op: string
  at: number
  written_by: Record<string, unknown> | null
  values: Record<string, unknown>
}

export type RowHistory = { table: string; row: string; versions: Version[] }

export type Settlement = {
  table: string
  row: string
  at: number
  written_by: Record<string, unknown> | null
  values: Record<string, unknown>
}

export class ApiError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message)
  }
}

async function get<T>(path: string): Promise<T> {
  const response = await fetch(path, { credentials: 'same-origin' })
  if (!response.ok) {
    const body = (await response.json().catch(() => ({}))) as { error?: string }
    throw new ApiError(response.status, body.error ?? '读不到')
  }
  return (await response.json()) as T
}

async function getText(path: string): Promise<string> {
  const response = await fetch(path, { credentials: 'same-origin' })
  if (!response.ok) throw new ApiError(response.status, '读不到')
  return await response.text()
}

export const api = {
  me: () => get<Identity>('/api/me'),
  projects: () => get<{ projects: Project[] }>('/api/projects'),
  boards: (project: string) =>
    get<{ boards: BoardSummary[] }>(`/api/projects/${project}/boards`),
  board: (project: string, board: string) =>
    get<BoardView>(`/api/projects/${project}/boards/${board}`),
  history: (project: string, table: string, row: string) =>
    get<RowHistory>(`/api/projects/${project}/tables/${table}/rows/${row}/history`),
  settlements: (project: string, table: string, instance: string) =>
    get<{ settlements: Settlement[] }>(
      `/api/projects/${project}/tables/${table}/instances/${instance}/settlements`,
    ),
  /** 长文本的原文（`BRD-010`）。**不经渲染。** */
  raw: (project: string, table: string, row: string, column: string) =>
    getText(`/api/projects/${project}/tables/${table}/rows/${row}/columns/${column}/raw`),
  rawUrl: (project: string, table: string, row: string, column: string) =>
    `/api/projects/${project}/tables/${table}/rows/${row}/columns/${column}/raw`,
}
