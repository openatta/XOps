/**
 * 个人看板（`NTF-001`）画成什么样。
 *
 * ⚠️ 这一层挡的失效方式**全部不报错**：分组错、排序错、截断没说、
 * 长出一个按钮。后端那 27 条断言看不见它们——数据是对的，画错了而已。
 */

import { describe, expect, it, vi, beforeEach } from 'vitest'
import { render, screen } from '@testing-library/react'

import { api } from '../api'
import type { Notice } from '../api'
import { PersonalPage } from './Personal'

function 通知(kind: string, text: string, project: string | null = 'P1'): Notice {
  return {
    notice: `n-${kind}-${text}`,
    kind,
    project,
    subject: `s/${text}`,
    text,
    created_at: 1_700_000_000_000,
  }
}

function 备好(notices: Notice[], extra: { limit?: number; truncated?: boolean } = {}) {
  vi.spyOn(api, 'notices').mockResolvedValue({
    notices,
    limit: extra.limit ?? 200,
    truncated: extra.truncated ?? false,
  })
  vi.spyOn(api, 'projects').mockResolvedValue({
    projects: [
      { project: 'P1', slug: 'acme', display_name: 'Acme', role: 'owner', archived: false },
    ],
  })
}

beforeEach(() => vi.restoreAllMocks())

describe('个人看板', () => {
  it('五类各自成组，要我动手的排在前面', async () => {
    // ⚠️ 顺序是这一页唯一的判断。排错了不报错，只是**要我动手的沉到底下**。
    备好([
      通知('run-finished', '执行完了'),
      通知('node-awaiting-me', '有节点在等我'),
      通知('instance-decided', '实例定了'),
    ])
    render(<PersonalPage />)
    await screen.findByText('有节点在等我')

    const 标题 = screen.getAllByRole('heading', { level: 2 }).map((h) => h.textContent)
    expect(标题).toEqual(['有节点在等我处理', '流程实例已决定', '执行完成或失败'])
  })

  it('一条都没有的时候说没有，而不是画一堆空组', async () => {
    备好([])
    render(<PersonalPage />)
    expect(await screen.findByText('没有在等你的事。')).toBeTruthy()
    expect(screen.queryByRole('heading', { level: 2 })).toBeNull()
  })

  it('正文原样显示，不摘要不改写（NTF-004）', async () => {
    const 原文 = '发起人自己表态不算数：#7 的这一行没有被采纳'
    备好([通知('row-not-settled', 原文)])
    render(<PersonalPage />)
    expect(await screen.findByText(原文)).toBeTruthy()
  })

  it('截断要说出来', async () => {
    // ⚠️ 这一条的失效表现是"**怎么没收到通知**"——查起来最慢的一种。
    备好([通知('node-awaiting-me', '一')], { limit: 200, truncated: true })
    render(<PersonalPage />)
    await screen.findByText('一')
    expect(screen.getByText(/未读超过 200 条/)).toBeTruthy()
  })

  it('没到上限就不吓唬人', async () => {
    备好([通知('node-awaiting-me', '一')])
    render(<PersonalPage />)
    await screen.findByText('一')
    expect(screen.queryByText(/未读超过/)).toBeNull()
  })

  it('页面上没有任何按钮 —— 标记已读是一次 MCP 调用（BRD-005）', async () => {
    // ⚠️ `frontend-discipline.mjs` 拦的是 `fetch(method: 'POST')` 这种**字面形状**，
    // 拦不住"长出一个按钮"。而 `BRD-005` 点名说过：
    // **个人看板上的「标记已读」也是发一次 MCP 调用**，页面该给命令不给按钮。
    备好([通知('node-awaiting-me', '一')])
    render(<PersonalPage />)
    await screen.findByText('一')
    expect(screen.queryAllByRole('button')).toHaveLength(0)
    expect(screen.getByText(/notice\.read notice=/)).toBeTruthy()
  })

  it('跨项目一起排，所以每条都说清自己来自哪个项目（NTF-014）', async () => {
    备好([通知('node-awaiting-me', '甲项目的'), 通知('node-awaiting-me', '没有项目的', null)])
    render(<PersonalPage />)
    await screen.findByText('甲项目的')
    // 解得出显示名就显示名字，解不出就不画那个链接——**不编一个**。
    expect(screen.getAllByRole('link').map((a) => a.textContent)).toEqual(['Acme'])
  })

  it('认不出的新类别不会消失', async () => {
    // ⚠️ 上游哪天多出第六类，**它不能就这么从页面上没了**。
    备好([通知('brand-new-kind', '新类别的一条')])
    render(<PersonalPage />)
    expect(await screen.findByText('新类别的一条')).toBeTruthy()
    expect(screen.getByRole('heading', { level: 2 }).textContent).toBe('brand-new-kind')
  })
})
