/**
 * XOps 的只读前端。
 *
 * **看板上没有任何直接写库的按钮**（`BRD-005`）。要写就发一次 MCP 调用——
 * 所以需要动手的地方，页面给的是"在你的 Agent 里跑这条命令"，而不是一个按钮。
 *
 * 这个文件只剩下**外壳与路由分发**：身份、导航、四条路径各自交给一页。
 */

import { useCallback } from 'react'

import { api } from './api'
import type { BoardSummary, Identity, Project } from './api'
import { logout } from './session'
import { HOME, href, navigate, useRoute } from './router'
import { useAsync } from './useAsync'
import { Link, Loading, Problem } from './shared'
import { LoginPage } from './pages/Login'
import { BoardPage } from './pages/Board'
import { ProjectPage } from './pages/Project'
import { PersonalPage } from './pages/Personal'

export function App() {
  const route = useRoute()
  const me = useAsync<Identity>(() => api.me(), [route.page === 'login'])
  const projects = useAsync<{ projects: Project[] }>(() => api.projects(), [me.value?.user])

  // 当前项目：项目页与看板页都有它。
  const project =
    route.page === 'project' || route.page === 'board' ? route.project : null
  const boards = useAsync<{ boards: BoardSummary[] }>(
    () => (project ? api.boards(project) : Promise.resolve({ boards: [] })),
    [project],
  )

  const signOut = useCallback(() => {
    logout().then(() => navigate(href.login(), true))
  }, [])

  if (route.page === 'login') return <LoginPage />

  // ⚠️ **只有 401 才送去登录页。** 别的错原样显示——
  // 早先这里是"`me()` 读不到就渲染登录表单"，于是一次 500 会告诉一个
  // 已经登录的人"请登录"，而真正的原因一个字都没显示出来。
  if (me.unauthorized) {
    navigate(href.login(), true)
    return null
  }
  if (me.loading) return <Loading />
  if (me.error) return <Problem error={me.error} />
  if (!me.value) return null

  return (
    <div className="app">
      <header className="top">
        {/* BRD-011：明确展示当前用户身份。 */}
        <span className="me">
          {me.value.display_name}（{me.value.provider}/{me.value.account}）
        </span>
        <nav className="quick">
          <Link to={HOME}>个人看板</Link>
        </nav>
        <button type="button" onClick={signOut}>
          注销
        </button>
      </header>

      <nav>
        <h2>项目</h2>
        {projects.error && <Problem error={projects.error} />}
        <ul>
          {projects.value?.projects.map((entry) => (
            <li key={entry.project}>
              <Link
                to={href.project(entry.project)}
                className={entry.project === project ? 'current' : undefined}
              >
                {entry.display_name}
                <small>
                  {entry.slug} · {entry.role}
                  {entry.archived && ' · 已归档'}
                </small>
              </Link>
            </li>
          ))}
        </ul>
        {projects.value?.projects.length === 0 && (
          <p className="empty">还没有把你加进任何项目。</p>
        )}

        {project && (
          <>
            <h2>看板</h2>
            <ul>
              {boards.value?.boards.map((entry) => (
                <li key={entry.board}>
                  <Link
                    to={href.board(project, entry.board)}
                    className={
                      route.page === 'board' && route.board === entry.board
                        ? 'current'
                        : undefined
                    }
                  >
                    {entry.name}
                    <small>{entry.table}</small>
                  </Link>
                </li>
              ))}
            </ul>
          </>
        )}
      </nav>

      <main>
        {route.page === 'me' && <PersonalPage />}
        {route.page === 'project' && (
          <ProjectPage
            project={route.project}
            detail={projects.value?.projects.find((entry) => entry.project === route.project)}
            boards={boards}
          />
        )}
        {route.page === 'board' && <BoardPage project={route.project} board={route.board} />}
        {route.page === 'unknown' && (
          <p className="empty">没有这个页面：{route.path}</p>
        )}
      </main>
    </div>
  )
}
