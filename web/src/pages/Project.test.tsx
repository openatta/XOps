/**
 * 项目看板页画成什么样。
 */

import { describe, expect, it, vi, beforeEach } from 'vitest'
import { act, render, screen } from '@testing-library/react'

import { api } from '../api'
import type { BoardSummary, Member, Project, TableSummary } from '../api'
import type { Async } from '../useAsync'
import { ProjectPage } from './Project'

const 项目: Project = {
  project: 'P1',
  slug: 'acme',
  display_name: 'Acme',
  role: 'owner',
  archived: false,
}

function 就绪<T>(value: T): Async<T> {
  return { value, error: null, unauthorized: false, loading: false }
}

function 备好(members: Member[] = [], tables: TableSummary[] = []) {
  vi.spyOn(api, 'members').mockResolvedValue({ members })
  vi.spyOn(api, 'tables').mockResolvedValue({ tables })
}

function 看板(board: string, table: string, name: string): BoardSummary {
  return { board, name, table }
}

function 表(table: string, kind = 'user', protection = 'normal'): TableSummary {
  return { table, kind, protection, columns: [{ column: 'title', kind: '文本', required: true }] }
}

/**
 * 画一遍，并且**等成员与表清单那两块也画完**。
 *
 * ⚠️ 不等的话 React 会在用例结束之后才 setState，控制台刷一片 act 警告。
 * 警告本身无害，可它们会把**将来真正的警告淹掉**——
 * 一个总在报警的东西等于没有报警。
 */
async function 画(boards: Async<{ boards: BoardSummary[] }>) {
  render(<ProjectPage project="P1" detail={项目} boards={boards} />)
  await act(async () => {})
}

beforeEach(() => vi.restoreAllMocks())

describe('项目看板页', () => {
  it('系统表的看板与业务表的看板分开列', async () => {
    // ⚠️ 混在一个列表里只有表名能区分，而表名不是建看板的人取的那个名字。
    备好()
    await 画(就绪({ boards: [看板('b1', 'bugs', '缺陷'), 看板('b2', '_runs', '执行')] }))

    const 标题 = screen.getAllByRole('heading', { level: 2 }).map((h) => h.textContent)
    expect(标题).toEqual(['看板', '平台自己的表', '成员', '表'])
    expect(screen.getByText('缺陷').closest('a')?.getAttribute('href')).toBe(
      '/projects/P1/boards/b1',
    )
    expect(screen.getByText(/跑了什么、成没成/)).toBeTruthy()
  })

  it('一个看板都没有的时候给的是命令，不是按钮（BRD-005）', async () => {
    备好()
    await 画(就绪({ boards: [] }))
    expect(screen.getByText(/board\.define project=P1/)).toBeTruthy()
    expect(screen.queryAllByRole('button')).toHaveLength(0)
  })

  it('只有业务看板时，提醒平台表也可以建（执行与流程否则看不到）', async () => {
    备好()
    await 画(就绪({ boards: [看板('b1', 'bugs', '缺陷')] }))
    expect(screen.getByText(/table=_runs/)).toBeTruthy()
  })

  it('还没建看板的表带一个标记 —— 否则它在页面上等于不存在', async () => {
    // ⚠️ 这是这一页存在的一半理由：在表清单这条路由之前，
    // 前端只知道有哪些**看板**，**而没有任何地方会说这件事**。
    备好([], [表('bugs'), 表('notes')])
    await 画(就绪({ boards: [看板('b1', 'bugs', '缺陷')] }))
    expect(screen.getAllByText('还没有看板')).toHaveLength(1)
    expect(screen.getByText('notes').textContent).toContain('还没有看板')
  })

  it('系统表与受保护的表各带自己的标记', async () => {
    备好([], [表('_runs', 'system'), 表('owners', 'user', 'protected')])
    await 画(就绪({ boards: [] }))
    expect(screen.getByText('平台')).toBeTruthy()
    expect(screen.getByText('只有所有者能写')).toBeTruthy()
  })

  it('成员显示名字与角色（PRJ-007：角色是这个项目里的）', async () => {
    备好([{ user: 'u1', display_name: '甲', role: 'owner', added_at: 0 }])
    await 画(就绪({ boards: [] }))
    expect(screen.getByText('甲')).toBeTruthy()
    expect(screen.getAllByText('owner').length).toBeGreaterThan(0)
  })

  it('读不到看板清单时说出原因，不装成空项目', async () => {
    // ⚠️ "读不到"与"这个项目没有看板"必须分得开——
    // 装成空的，看的人会去查一个根本不存在的问题。
    备好()
    await 画({ value: null, error: '读不到', unauthorized: false, loading: false })
    expect(screen.getByText('读不到')).toBeTruthy()
    expect(screen.queryByText(/还没有业务表的看板/)).toBeNull()
  })
})
