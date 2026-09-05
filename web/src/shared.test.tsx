/**
 * `Link` 与 `AgentHint`。
 *
 * ⚠️ `Link` 那几条盯的是**为什么它必须是 `<a href>` 而不是 `<button onClick>`**：
 * 中键点开、复制链接地址、发给别人——这些是看板这种东西被使用的常态，
 * 而一个按钮把它们全都拿掉了，**并且不报错**。
 */

import { describe, expect, it, beforeEach } from 'vitest'
import { fireEvent, render, screen } from '@testing-library/react'

import { AgentHint, Link } from './shared'

beforeEach(() => window.history.replaceState(null, '', '/me'))

/**
 * 点一下，回答"**应用拦下了吗**"。
 *
 * ⚠️ 顺带把真正的跳转挡掉：没拦下的那几次，jsdom 会去执行 `<a>` 的默认行为，
 * 然后打一行 `Not implemented: navigation`。那行**不是失败**，可它会把
 * 将来真正的报错淹掉——**一个总在报错的输出等于没有输出**。
 *
 * 文档上这个监听器在冒泡的最后一环，所以它读到的 `defaultPrevented`
 * 就是应用那一侧的结论；读完再自己 `preventDefault`，跳转就不会发生。
 */
function 点(元素: Element, 选项: MouseEventInit = {}): boolean {
  let 拦下了 = false
  const 记一笔 = (event: Event) => {
    拦下了 = event.defaultPrevented
    event.preventDefault()
  }
  document.addEventListener('click', 记一笔)
  try {
    fireEvent(元素, new MouseEvent('click', { bubbles: true, cancelable: true, button: 0, ...选项 }))
  } finally {
    document.removeEventListener('click', 记一笔)
  }
  return 拦下了
}

describe('Link', () => {
  it('是一个带 href 的真链接', () => {
    render(<Link to="/projects/P1">去项目</Link>)
    expect(screen.getByRole('link', { name: '去项目' }).getAttribute('href')).toBe('/projects/P1')
  })

  it('普通左键点击不整页重载，只换地址', () => {
    render(<Link to="/projects/P1">去项目</Link>)
    expect(点(screen.getByRole('link'))).toBe(true)
    expect(window.location.pathname).toBe('/projects/P1')
  })

  it('带修饰键的点击交给浏览器 —— 那是"在新标签页打开"', () => {
    render(<Link to="/projects/P1">去项目</Link>)
    for (const 修饰 of [{ metaKey: true }, { ctrlKey: true }, { shiftKey: true }, { altKey: true }]) {
      expect(点(screen.getByRole('link'), 修饰), JSON.stringify(修饰)).toBe(false)
    }
    expect(window.location.pathname).toBe('/me')
  })

  it('中键点击也交给浏览器', () => {
    render(<Link to="/projects/P1">去项目</Link>)
    expect(点(screen.getByRole('link'), { button: 1 })).toBe(false)
    expect(window.location.pathname).toBe('/me')
  })
})

describe('AgentHint', () => {
  it('给的是一条命令，不是一个按钮（BRD-005）', () => {
    render(<AgentHint command="notice.read notice=n1" />)
    expect(screen.getByText('notice.read notice=n1')).toBeTruthy()
    expect(screen.queryAllByRole('button')).toHaveLength(0)
  })
})
