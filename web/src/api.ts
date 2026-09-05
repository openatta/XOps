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

/**
 * 个人看板上的一条（`NTF-001`）。
 *
 * ⚠️ **`text` 是指针不是内容**（`NTF-006`），由确定性代码生成、不经模型（`NTF-003`），
 * 自由文本原样引用或截断（`NTF-004`）。**前端照原样显示，不再加工**——
 * 摘要、改写、翻译在这里都是越界。
 */
export type Notice = {
  notice: string
  /** 五类之一（`NTF-007`）。 */
  kind: string
  /** **可以是 null**——`_notices` 是平台全局表（`NTF-014`）。 */
  project: string | null
  subject: string
  text: string
  created_at: number
}

export type Member = {
  user: string
  display_name: string
  role: string
  added_at: number
}

export type ColumnSummary = { column: string; kind: string; required: boolean }

/** 一张表。**不含任何一行数据**——要看行就去看板那条路（`BRD-001`）。 */
export type TableSummary = {
  table: string
  kind: string
  protection: string
  columns: ColumnSummary[]
}

export type Row = { row: string; values: Record<string, unknown> }

export type BoardView = {
  board: string
  name: string
  table: string
  columns: string[]
  rows: Row[]
  offset: number
  /**
   * 后面还有没有。
   *
   * ⚠️ **没有"一共几行"，这是刻意的**：一个总数会被读成一个指标
   * （"缺陷 42 条"），而 `BRD-002` 说平台不内建任何报表。
   * 翻页需要的只是这一个布尔。
   */
  has_more: boolean
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
  /**
   * 个人看板（`NTF-001`）。
   *
   * ⚠️ **没有 user 参数，这是刻意的。** `NTF-010` 的硬限定靠调用方**表达不出**
   * "看别人的"这个请求兑现——不是"表达得出但被拒绝"。别给它加参数。
   */
  notices: () =>
    get<{ notices: Notice[]; limit: number; truncated: boolean }>('/api/me/notices'),
  members: (project: string) =>
    get<{ members: Member[] }>(`/api/projects/${project}/members`),
  tables: (project: string) =>
    get<{ tables: TableSummary[] }>(`/api/projects/${project}/tables`),
  boards: (project: string) =>
    get<{ boards: BoardSummary[] }>(`/api/projects/${project}/boards`),
  board: (project: string, board: string, offset = 0) =>
    get<BoardView>(`/api/projects/${project}/boards/${board}?offset=${offset}`),
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
