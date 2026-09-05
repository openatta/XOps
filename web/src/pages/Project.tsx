/**
 * 项目看板页。
 *
 * ⚠️ **系统表的看板与业务表的看板分开列。** 两者都是 `BRD-001` 的自由看板
 * （`board.define` 见到下划线走 `TableId::system`，`check_boardable` 只拦 `_notices`），
 * 但读的人看的是两件事：`_runs` 是"平台替我干活干得怎么样"，`bugs` 是"我的活本身"。
 * 混在一个列表里只有表名能区分，而表名是建看板的人起的名字之外的那一列。
 *
 * **这不是报表**（`BRD-002`）：没有聚合、没有趋势、没有跨项目对比，
 * 页面上**一个数都不是算出来的**——它只是把已有的东西按种类排了个序。
 * 判据是 `BRD-003`：哪天要在这里写"什么是缺陷密度"，就越界了。
 */

import { api } from '../api'
import type { BoardSummary, Member, Project, TableSummary } from '../api'
import type { Async } from '../useAsync'
import { useAsync } from '../useAsync'
import { AgentHint, Link, Loading, Problem } from '../shared'
import { href } from '../router'

/** 平台自己的四张表。`_notices` 不在里面——它建不了自由看板（`BRD-004`）。 */
const SYSTEM_TABLE_MEANING: Record<string, string> = {
  _runs: '执行：跑了什么、成没成、烧了多少 token',
  _flows: '流程实例：主体是谁、走到哪、谁发起的',
  _flow_nodes: '流程节点：哪一个在等人处理、谁结算的',
  _plugins: '插件：装了哪些、什么状态、声明了什么能力',
}

function BoardList({
  project,
  boards,
  empty,
}: {
  project: string
  boards: BoardSummary[]
  empty: string
}) {
  if (boards.length === 0) return <p className="empty">{empty}</p>
  return (
    <ul className="boards">
      {boards.map((board) => (
        <li key={board.board}>
          <Link to={href.board(project, board.board)}>
            {board.name}
            <small>
              {board.table}
              {SYSTEM_TABLE_MEANING[board.table] && ` · ${SYSTEM_TABLE_MEANING[board.table]}`}
            </small>
          </Link>
        </li>
      ))}
    </ul>
  )
}

function Members({ project }: { project: string }) {
  const members = useAsync<{ members: Member[] }>(() => api.members(project), [project])
  if (members.loading) return <Loading />
  if (members.error) return <Problem error={members.error} />
  return (
    <ul className="members">
      {members.value?.members.map((member) => (
        <li key={member.user}>
          <span>{member.display_name}</span>
          {/* PRJ-007：角色是（项目, 用户）上的一条记录，不是这个人身上的属性。 */}
          <small>{member.role}</small>
        </li>
      ))}
    </ul>
  )
}

/**
 * 这个项目有哪些表。
 *
 * ⚠️ **它存在的理由是"一张还没建看板的表在页面上完全不存在"。** 在这之前，
 * 前端只知道有哪些**看板**——一张刚建好、还没人给它建看板的表，
 * 在页面上和不存在没有区别，而**没有任何地方会说这件事**。
 */
function Tables({ project, boards }: { project: string; boards: BoardSummary[] }) {
  const tables = useAsync<{ tables: TableSummary[] }>(() => api.tables(project), [project])
  if (tables.loading) return <Loading />
  if (tables.error) return <Problem error={tables.error} />

  const boarded = new Set(boards.map((board) => board.table))
  const list = tables.value?.tables ?? []
  return (
    <ul className="tables">
      {list.map((table) => (
        <li key={table.table}>
          <span>
            {table.table}
            {table.kind === 'system' && <small className="tag">平台</small>}
            {table.protection === 'protected' && <small className="tag">只有所有者能写</small>}
            {!boarded.has(table.table) && <small className="tag">还没有看板</small>}
          </span>
          <small>{table.columns.map((column) => column.column).join(' · ')}</small>
        </li>
      ))}
      {list.length === 0 && <p className="empty">这个项目还没有表。</p>}
    </ul>
  )
}

export function ProjectPage({
  project,
  detail,
  boards,
}: {
  project: string
  detail: Project | undefined
  boards: Async<{ boards: BoardSummary[] }>
}) {
  if (boards.loading) return <Loading />
  if (boards.error) return <Problem error={boards.error} />

  const all = boards.value?.boards ?? []
  const system = all.filter((board) => board.table.startsWith('_'))
  const business = all.filter((board) => !board.table.startsWith('_'))

  return (
    <>
      <h1>{detail?.display_name ?? project}</h1>
      <p className="who">
        {detail ? `${detail.slug} · 我的角色 ${detail.role}` : project}
        {detail?.archived && ' · 已归档'}
      </p>

      <h2>看板</h2>
      <BoardList project={project} boards={business} empty="这个项目还没有业务表的看板。" />

      <h2>平台自己的表</h2>
      <BoardList
        project={project}
        boards={system}
        empty="没有给平台表建过看板 —— 执行与流程因此在页面上看不到。"
      />

      <h2>成员</h2>
      <Members project={project} />

      <h2>表</h2>
      <Tables project={project} boards={all} />

      {all.length === 0 && <AgentHint command={`board.define project=${project} table=… name=…`} />}
      {business.length > 0 && system.length === 0 && (
        <AgentHint
          what="想在页面上看到执行与流程，给平台表也建一个看板："
          command={`board.define project=${project} table=_runs name=执行`}
        />
      )}
    </>
  )
}
