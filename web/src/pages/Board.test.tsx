/**
 * 看板页：翻页与长文本那两处。
 */

import { describe, expect, it, vi, beforeEach } from 'vitest'
import { act, fireEvent, render, screen } from '@testing-library/react'

import { api } from '../api'
import type { BoardView } from '../api'
import { BoardPage } from './Board'

function 一页(offset: number, titles: string[], has_more: boolean): BoardView {
  return {
    board: 'b1',
    name: '全部缺陷',
    table: 'bugs',
    columns: ['title'],
    rows: titles.map((title, index) => ({
      row: `r${offset + index}`,
      values: { title, writtenBy: { kind: 'person', user: 'u1' } },
    })),
    offset,
    has_more,
  }
}

beforeEach(() => vi.restoreAllMocks())

describe('看板页', () => {
  it('翻页按 offset 走，且第一页没有「上一页」', async () => {
    const board = vi
      .spyOn(api, 'board')
      .mockImplementation((_p, _b, offset = 0) =>
        Promise.resolve(offset === 0 ? 一页(0, ['甲', '乙'], true) : 一页(2, ['丙'], false)),
      )
    render(<BoardPage project="P1" board="b1" />)
    await act(async () => {})

    expect(screen.getByRole('button', { name: '上一页' }).hasAttribute('disabled')).toBe(true)
    await act(async () => fireEvent.click(screen.getByRole('button', { name: '下一页' })))

    expect(board).toHaveBeenLastCalledWith('P1', 'b1', 2)
    expect(screen.getByText('丙')).toBeTruthy()
    expect(screen.getByRole('button', { name: '下一页' }).hasAttribute('disabled')).toBe(true)
  })

  it('只有一页的时候根本不画翻页条', async () => {
    vi.spyOn(api, 'board').mockResolvedValue(一页(0, ['甲'], false))
    render(<BoardPage project="P1" board="b1" />)
    await act(async () => {})
    expect(screen.queryByRole('button', { name: '下一页' })).toBeNull()
  })

  it('换看板时页码归零', async () => {
    // ⚠️ 不归零的话，从一张长表切到一张短表会看到**一片空白**，而它不报错。
    const board = vi.spyOn(api, 'board').mockResolvedValue(一页(0, ['甲', '乙'], true))
    const view = render(<BoardPage project="P1" board="b1" />)
    await act(async () => {})
    await act(async () => fireEvent.click(screen.getByRole('button', { name: '下一页' })))
    expect(board).toHaveBeenLastCalledWith('P1', 'b1', 2)

    view.rerender(<BoardPage project="P1" board="b2" />)
    await act(async () => {})
    expect(board).toHaveBeenLastCalledWith('P1', 'b2', 0)
  })

  it('页面上说的是第几行起，不是共几页', async () => {
    // 后端**故意不给总数**（一个总数会被读成指标，`BRD-002`），前端也不算。
    vi.spyOn(api, 'board').mockResolvedValue(一页(4, ['戊'], false))
    render(<BoardPage project="P1" board="b1" />)
    await act(async () => {})
    expect(screen.getByText('第 5 行起')).toBeTruthy()
    expect(screen.queryByText(/共.*页/)).toBeNull()
  })

  it('来源标识画出来（TBL-016）', async () => {
    vi.spyOn(api, 'board').mockResolvedValue(一页(0, ['甲'], false))
    render(<BoardPage project="P1" board="b1" />)
    await act(async () => {})
    expect(screen.getByText(/人 u1/)).toBeTruthy()
  })

  it('读不到就说读不到，不画一张空表', async () => {
    vi.spyOn(api, 'board').mockRejectedValue(new Error('炸了'))
    render(<BoardPage project="P1" board="b1" />)
    await act(async () => {})
    expect(screen.getByText('读不到')).toBeTruthy()
    expect(screen.queryByRole('table')).toBeNull()
  })

  it('长文本折起来，而且给得出原文的下载地址（BRD-010）', async () => {
    const 长 = '很长的一段。'.repeat(30)
    vi.spyOn(api, 'board').mockResolvedValue({
      ...一页(0, [], false),
      columns: ['body'],
      rows: [{ row: 'r0', values: { body: 长, writtenBy: { kind: 'platform' } } }],
    })
    render(<BoardPage project="P1" board="b1" />)
    await act(async () => {})

    expect(screen.getByRole('button', { name: '展开' })).toBeTruthy()
    // ⚠️ **不信任渲染的人要拿得到原文**，而且那个地址不经任何渲染。
    const 原文 = screen.getByRole('link', { name: '原文' })
    expect(原文.getAttribute('href')).toBe(
      '/api/projects/P1/tables/bugs/rows/r0/columns/body/raw',
    )
  })
})
