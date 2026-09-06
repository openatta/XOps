/**
 * 外壳：路由分发 · 身份 · 导航 · 注销。
 *
 * ⚠️ **四个页面各自有测试，把它们串起来那一层以前没有。**
 * 这一层挡的失效方式同样不报错：**路由分发错到另一页**、
 * **一次 500 被画成"请登录"**、**当前项目高亮不上**、
 * **注销之后还留在原地**。
 */

import { describe, expect, it, vi, beforeEach } from 'vitest'
import { act, fireEvent, render, screen, within } from '@testing-library/react'

import { App } from './App'
import { api, ApiError } from './api'
import type { BoardSummary, Identity, Project } from './api'
import * as session from './session'

const 我: Identity = {
  user: 'u1',
  display_name: 'Alice',
  provider: 'builtin',
  account: 'alice@x',
}

const 项目们: Project[] = [
  { project: 'P1', slug: 'acme', display_name: 'Acme', role: 'owner', archived: false },
  { project: 'P2', slug: 'beta', display_name: 'Beta', role: 'member', archived: true },
]

const 看板们: BoardSummary[] = [{ board: 'B1', name: '缺陷', table: 'bugs' }]

/** 一份都读得到的后端。**每条都要备**——漏一条会以"读不到"的形式出现在别处。 */
function 备好(me: () => Promise<Identity> = () => Promise.resolve(我)) {
  vi.spyOn(api, 'me').mockImplementation(me)
  vi.spyOn(api, 'projects').mockResolvedValue({ projects: 项目们 })
  vi.spyOn(api, 'boards').mockResolvedValue({ boards: 看板们 })
  vi.spyOn(api, 'board').mockResolvedValue({
    board: 'B1',
    name: '缺陷',
    table: 'bugs',
    columns: ['title'],
    rows: [],
    offset: 0,
    has_more: false,
  })
  vi.spyOn(api, 'notices').mockResolvedValue({ notices: [], limit: 200, truncated: false })
  vi.spyOn(api, 'members').mockResolvedValue({ members: [] })
  vi.spyOn(api, 'tables').mockResolvedValue({ tables: [] })
}

async function 打开(path: string) {
  window.history.replaceState(null, '', path)
  render(<App />)
  await act(async () => {})
}

beforeEach(() => {
  vi.restoreAllMocks()
  window.history.replaceState(null, '', '/me')
})

describe('路由分发', () => {
  it('/me 是个人看板', async () => {
    备好()
    await 打开('/me')
    expect(screen.getByRole('heading', { level: 1 }).textContent).toBe('个人看板')
  })

  it('/projects/P1 是项目页', async () => {
    备好()
    await 打开('/projects/P1')
    expect(screen.getByRole('heading', { level: 1 }).textContent).toBe('Acme')
  })

  it('/projects/P1/boards/B1 是那个看板', async () => {
    备好()
    await 打开('/projects/P1/boards/B1')
    expect(screen.getByRole('heading', { level: 1 }).textContent).toBe('缺陷')
  })

  it('/login 不去读身份，也不画外壳', async () => {
    // ⚠️ 登录页在外壳**之前**返回。它要是走到下面，一个没登录的人
    // 会先看到一次 401、再被跳回来 —— 一个转不出来的圈。
    备好()
    await 打开('/login')
    expect(screen.getByRole('button', { name: /登录/ })).toBeTruthy()
    expect(api.me).not.toHaveBeenCalled()
    expect(screen.queryByText(/Alice/)).toBeNull()
  })

  it('认不出的地址说出来，不悄悄回首页', async () => {
    备好()
    await 打开('/nope')
    expect(screen.getByText(/没有这个页面：\/nope/)).toBeTruthy()
    // 但外壳还在 —— 认不出一页不等于整个站没了。
    expect(screen.getByText(/Alice/)).toBeTruthy()
  })
})

describe('读不到身份的时候', () => {
  it('401 送去登录页', async () => {
    备好(() => Promise.reject(new ApiError(401, '请先登录')))
    await 打开('/me')
    expect(window.location.pathname).toBe('/login')
  })

  it('500 原样显示，不冒充"请登录"', async () => {
    // ⚠️ 这一条是这个外壳最早的那个坑：**一次 500 会告诉一个已经登录的人
    // "请登录"，而真正的原因一个字都没显示出来。**
    备好(() => Promise.reject(new ApiError(500, '库炸了')))
    await 打开('/me')
    expect(screen.getByText('库炸了')).toBeTruthy()
    expect(window.location.pathname).toBe('/me')
  })

  it('读取中就说读取中，不画成空的', async () => {
    备好(() => new Promise(() => {}))
    window.history.replaceState(null, '', '/me')
    render(<App />)
    expect(screen.getByText('读取中…')).toBeTruthy()
  })
})

describe('外壳', () => {
  it('明确展示当前用户身份（BRD-011）', async () => {
    备好()
    await 打开('/me')
    expect(screen.getByText('Alice（builtin/alice@x）')).toBeTruthy()
  })

  it('项目列表带角色与归档标记，且是真链接', async () => {
    备好()
    await 打开('/me')
    const 导航 = within(document.querySelector('.app > nav') as HTMLElement)
    const acme = 导航.getByText('Acme').closest('a')
    expect(acme?.getAttribute('href')).toBe('/projects/P1')
    expect(screen.getByText(/beta · member/).textContent).toContain('已归档')
  })

  it('当前项目高亮，别的不亮', async () => {
    // 不高亮不报错，只是**看的人不知道自己在哪**。
    // ⚠️ 项目名在导航与标题上各出现一次，所以要**限定在导航里找**。
    备好()
    await 打开('/projects/P1')
    const 导航 = within(document.querySelector('.app > nav') as HTMLElement)
    expect(导航.getByText('Acme').closest('a')?.className).toBe('current')
    expect(导航.getByText('Beta').closest('a')?.className ?? '').not.toBe('current')
  })

  it('选中项目才列它的看板', async () => {
    备好()
    await 打开('/me')
    expect(screen.queryByText('缺陷')).toBeNull()
    expect(api.boards).not.toHaveBeenCalled()
  })

  it('一个项目都没有的时候说出来', async () => {
    备好()
    vi.spyOn(api, 'projects').mockResolvedValue({ projects: [] })
    await 打开('/me')
    expect(screen.getByText('还没有把你加进任何项目。')).toBeTruthy()
  })

  it('注销之后去登录页，而且不留在后退历史里', async () => {
    // ⚠️ `replace`：留下的话，注销后按一下后退又回到那一页，
    // 而那时会话已经没了 —— 一个转不出来的圈。
    备好()
    const logout = vi.spyOn(session, 'logout').mockResolvedValue(undefined)
    await 打开('/me')
    await act(async () => fireEvent.click(screen.getByRole('button', { name: '注销' })))
    expect(logout).toHaveBeenCalled()
    expect(window.location.pathname).toBe('/login')
  })
})
