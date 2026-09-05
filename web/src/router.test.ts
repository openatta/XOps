import { describe, expect, it } from 'vitest'

import { href, parse } from './router'

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
