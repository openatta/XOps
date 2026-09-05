/**
 * 个人看板（`NTF-001`）。
 *
 * ⚠️ **它是平台内建的固定视图，不是用户能配的那种看板**（`BRD-004`）——
 * `_notices` 建不了自由看板，`check_boardable` 当场拒。所以这一页不走
 * `BoardTable` 那条渲染路径，它有自己的形状。
 *
 * 归属：条目在 RP-17，数据经 RP-05 的读模型，这一页归 RP-06。
 * ⚠️ **这三者以前互相指对方，于是它一直没有被做**——通知服务整套都在，
 * 只读面上却一条通知路由都没有。2026-09-04 才定下来，
 * 见 `docs/requirements/README.md` §4 那条注。
 *
 * # 这一页上没有按钮
 *
 * `BRD-005` 点了名：**「标记已读」也是一次 MCP 调用**，走平台专属 tool
 * （`NTF-009`、`NTF-011`：`readAt` 是全系统唯一一个用户可改的系统表列）。
 * 所以这里给的是命令，不是按钮——**前端里根本不存在能发出写请求的代码路径**，
 * `scripts/frontend-discipline.mjs` 枚举全部源码盯着这件事。
 *
 * # 正文原样显示
 *
 * `NTF-003` 内容由确定性代码生成、不经模型；`NTF-004` 里面的自由文本
 * **原样引用或截断，不改写、不摘要、不翻译**。所以这一页也不加工——
 * 摘要、归并、"共 3 条相似"在这里都是越界。
 */

import { api } from '../api'
import type { Notice, Project } from '../api'
import { useAsync } from '../useAsync'
import { AgentHint, Link, Loading, Problem } from '../shared'
import { href } from '../router'

/** 值得通知的**五类**（`NTF-007`）。**没有第六类。** */
const KIND_LABEL: Record<string, string> = {
  'node-awaiting-me': '有节点在等我处理',
  'instance-decided': '流程实例已决定',
  'row-not-settled': '我写的行未被采纳',
  'run-finished': '执行完成或失败',
  'row-assigned-to-me': '表里的某行指派给我',
}

/** 五类的显示顺序。**要我动手的排在前面**，已经发生完的排在后面。 */
const ORDER = [
  'node-awaiting-me',
  'row-assigned-to-me',
  'row-not-settled',
  'instance-decided',
  'run-finished',
]

function Group({
  kind,
  notices,
  projects,
}: {
  kind: string
  notices: Notice[]
  projects: Project[]
}) {
  if (notices.length === 0) return null
  return (
    <section className="notice-group">
      {/*
        ⚠️ **标题里不放条数。** 一个数字在这种位置会被当成指标读，
        而 `BRD-002` 说平台不内建任何报表。下面每一条都在，数得出来。
      */}
      <h2>{KIND_LABEL[kind] ?? kind}</h2>
      <ol className="notices">
        {notices.map((notice) => {
          const project = projects.find((entry) => entry.project === notice.project)
          return (
            <li key={notice.notice}>
              <header>
                <time>{new Date(notice.created_at).toLocaleString()}</time>
                {/* NTF-014：跨项目一起排，所以每一条都得说清自己来自哪个项目。 */}
                {notice.project && (
                  <Link to={href.project(notice.project)}>
                    {project?.display_name ?? notice.project}
                  </Link>
                )}
              </header>
              {/* NTF-004：原样。不摘要、不改写、不翻译。 */}
              <p className="notice-text">{notice.text}</p>
              {/* NTF-006：是指针，不是内容。 */}
              <small className="who">{notice.subject}</small>
              <AgentHint
                what="看完了就标记已读，在你的 Agent 里跑："
                command={`notice.read notice=${notice.notice}`}
              />
            </li>
          )
        })}
      </ol>
    </section>
  )
}

export function PersonalPage() {
  const inbox = useAsync<{ notices: Notice[]; limit: number; truncated: boolean }>(
    () => api.notices(),
    [],
  )
  const projects = useAsync<{ projects: Project[] }>(() => api.projects(), [])

  if (inbox.loading) return <Loading />
  if (inbox.error) return <Problem error={inbox.error} />

  const notices = inbox.value?.notices ?? []
  const known = new Set(ORDER)
  // ⚠️ 上游哪天多出一类，**它不能就这么消失**。认不出来的归到最后，照样显示。
  const extra = [...new Set(notices.map((notice) => notice.kind))].filter(
    (kind) => !known.has(kind),
  )

  return (
    <>
      <h1>个人看板</h1>
      {/* 页面上是给人读的话，星号是给源码读的 —— 别把 Markdown 强调符渲染出去。 */}
      <p className="who">
        只显示未读。已读的从这里消失，就是「标记已读」的意思——这是一份待办，
        不是一条收件箱时间线。
      </p>

      {notices.length === 0 && <p className="empty">没有在等你的事。</p>}

      {[...ORDER, ...extra].map((kind) => (
        <Group
          key={kind}
          kind={kind}
          notices={notices.filter((notice) => notice.kind === kind)}
          projects={projects.value?.projects ?? []}
        />
      ))}

      {/*
        ⚠️ **截断要说出来。** 这一条的失效表现是"怎么没收到通知"，
        而那是查起来最慢的一种 —— `Notices::unread` 早先"扫前一万行再过滤"
        的那个坑就长这样。
      */}
      {inbox.value?.truncated && (
        <p className="error">
          未读超过 {inbox.value.limit} 条，这一页只给了前 {inbox.value.limit} 条。
          剩下的要先处理掉一些才看得见。
        </p>
      )}
    </>
  )
}
