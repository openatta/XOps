import { beforeEach, describe, expect, it } from 'vitest'

import { href, intercept, navigate, parse } from './router'

describe('路由', () => {
  it('四条路径都认得出来', () => {
    expect(parse('/login')).toEqual({ page: 'login' })
    expect(parse('/me')).toEqual({ page: 'me' })
    expect(parse('/projects/p1')).toEqual({ page: 'project', project: 'p1' })
    expect(parse('/projects/p1/boards/b2')).toEqual({
      page: 'board',
      project: 'p1',
      board: 'b2',
    })
  })

  it('根路径是个人看板', () => {
    // 一个人登录进来第一眼该看的是"有什么在等我"，不是一个空的项目列表。
    expect(parse('/')).toEqual({ page: 'me' })
    expect(parse('')).toEqual({ page: 'me' })
  })

  it('不认识的路径说出来，不悄悄回首页', () => {
    // 悄悄回首页会把一个拼错的链接变成"这里什么都没有"，查起来很慢。
    expect(parse('/nope')).toEqual({ page: 'unknown', path: '/nope' })
    expect(parse('/projects/p1/boards')).toEqual({
      page: 'unknown',
      path: '/projects/p1/boards',
    })
  })

  it('地址与解析对得上，且经过转义', () => {
    // ⚠️ 生成与解析必须是一对。两边各写各的，出错的方式是**链接看着对、点开是别的页**。
    expect(parse(href.project('p1'))).toEqual({ page: 'project', project: 'p1' })
    expect(parse(href.board('p1', 'b2'))).toEqual({
      page: 'board',
      project: 'p1',
      board: 'b2',
    })
    const odd = 'a/b'
    expect(href.project(odd)).toBe('/projects/a%2Fb')
    expect(parse(href.project(odd))).toEqual({ page: 'project', project: odd })
  })
})

describe('地址真的换得动', () => {
  // ⚠️ 上面那四条测的是 `parse` 与 `href`，**两个纯函数**。
  // 真正碰 `window.history` 的是 `navigate` 与 `useRoute`，
  // 而它们在 `environment: 'node'` 的年代根本跑不起来。
  beforeEach(() => window.history.replaceState(null, '', '/me'))

  it('navigate 换地址并叫醒监听的人', () => {
    let 醒了 = 0
    const 听 = () => (醒了 += 1)
    window.addEventListener('popstate', 听)
    navigate('/projects/P1')
    window.removeEventListener('popstate', 听)

    expect(window.location.pathname).toBe('/projects/P1')
    expect(醒了).toBe(1)
  })

  it('去同一个地址不做任何事', () => {
    // 不挡的话，同一条链接点两下会往历史里塞两条一样的记录，
    // 于是要按两次后退才回得去。
    let 醒了 = 0
    const 听 = () => (醒了 += 1)
    window.addEventListener('popstate', 听)
    navigate('/me')
    window.removeEventListener('popstate', 听)
    expect(醒了).toBe(0)
  })

  it('replace 不往后退历史里留东西', () => {
    // ⚠️ 会话过期被踢到登录页，按一下后退又回到那一页、再被踢一次——
    // **那是一个转不出来的圈**。
    const 深度 = window.history.length
    navigate('/login', true)
    expect(window.location.pathname).toBe('/login')
    expect(window.history.length).toBe(深度)
  })

  it('intercept 只拦普通左键', () => {
    const 造 = (选项: Partial<Parameters<typeof intercept>[0]>) => ({
      preventDefault: () => {},
      metaKey: false,
      ctrlKey: false,
      shiftKey: false,
      altKey: false,
      button: 0,
      ...选项,
    })
    expect(intercept(造({}), '/projects/P1')).toBe(true)
    window.history.replaceState(null, '', '/me')
    for (const 例外 of [
      { metaKey: true },
      { ctrlKey: true },
      { shiftKey: true },
      { altKey: true },
      { button: 1 },
    ]) {
      expect(intercept(造(例外), '/projects/P1'), JSON.stringify(例外)).toBe(false)
      expect(window.location.pathname).toBe('/me')
    }
  })
})
