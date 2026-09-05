/**
 * 路由。**自己写，不引一个库。**
 *
 * 这个 SPA 一共四条路径、没有嵌套、没有懒加载、没有守卫链——
 * `RP-06` 把"npm 依赖面"列成本包引入的新风险（供应链与构建可重现），
 * 而一个路由库换回来的是 40 行代码。**不划算的那种依赖。**
 *
 * ⚠️ **深链是后端已经兑现了的**：`crates/xops-web/src/assets.rs` 里命不中 API 路由
 * 就回落到 `index.html`。所以下面这些是真的地址——刷新、收藏、复制给别人都成立，
 * 不是把状态藏在 `useState` 里假装成路由。
 *
 * ```text
 * /                                     → 跳到 /me
 * /login                                登录
 * /me                                   个人看板（NTF-001，平台内建的固定视图）
 * /projects/{project}                   项目看板页
 * /projects/{project}/boards/{board}    一个看板
 * ```
 *
 * **这里没有 fetch，也不碰 DOM 之外的东西。** 只读纪律（`BRD-005` ②）与它无关，
 * 但别让它变成有关的——路由是最容易被顺手加上"跳转前先 POST 一下"的地方。
 */

import { useEffect, useState } from 'react'

export type Route =
  | { page: 'login' }
  | { page: 'me' }
  | { page: 'project'; project: string }
  | { page: 'board'; project: string; board: string }
  | { page: 'unknown'; path: string }

/** 首页去哪。**个人看板**——一个人登录进来第一眼该看的是"有什么在等我"。 */
export const HOME = '/me'

export function parse(path: string): Route {
  const parts = path.split('/').filter((part) => part.length > 0)
  const [first, second, third, fourth] = parts
  if (parts.length === 0) return { page: 'me' }
  if (parts.length === 1 && first === 'login') return { page: 'login' }
  if (parts.length === 1 && first === 'me') return { page: 'me' }
  if (parts.length === 2 && first === 'projects' && second) {
    return { page: 'project', project: decodeURIComponent(second) }
  }
  if (parts.length === 4 && first === 'projects' && third === 'boards' && second && fourth) {
    return {
      page: 'board',
      project: decodeURIComponent(second),
      board: decodeURIComponent(fourth),
    }
  }
  return { page: 'unknown', path }
}

export const href = {
  login: () => '/login',
  me: () => '/me',
  project: (project: string) => `/projects/${encodeURIComponent(project)}`,
  board: (project: string, board: string) =>
    `/projects/${encodeURIComponent(project)}/boards/${encodeURIComponent(board)}`,
}

/**
 * 换一个地址。
 *
 * `replace` 用在**不该留在后退历史里**的跳转上：会话过期被踢到登录页，
 * 按一下后退又回到那个页面、再被踢一次，是一个转不出来的圈。
 */
export function navigate(path: string, replace = false) {
  if (path === window.location.pathname) return
  if (replace) window.history.replaceState(null, '', path)
  else window.history.pushState(null, '', path)
  window.dispatchEvent(new PopStateEvent('popstate'))
}

/** 当前路由。浏览器前进后退与 `navigate` 走同一个事件。 */
export function useRoute(): Route {
  const [path, setPath] = useState(() => window.location.pathname)
  useEffect(() => {
    const sync = () => setPath(window.location.pathname)
    window.addEventListener('popstate', sync)
    return () => window.removeEventListener('popstate', sync)
  }, [])
  return parse(path)
}

/**
 * 站内链接。
 *
 * ⚠️ **必须是真的 `<a href>`，不能是一个 `onClick` 的 `<button>`。**
 * 中键点开、复制链接地址、给别人发过去——这些是"看板"这种东西被使用的常态，
 * 而一个按钮把它们全都拿掉了，并且**不报错**。
 * 拦下默认行为只是为了不整页重载；带修饰键的点击照样交给浏览器。
 */
export function intercept(
  event: { preventDefault: () => void; metaKey: boolean; ctrlKey: boolean; shiftKey: boolean; altKey: boolean; button: number },
  path: string,
): boolean {
  if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return false
  if (event.button !== 0) return false
  event.preventDefault()
  navigate(path)
  return true
}
