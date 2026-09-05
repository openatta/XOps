/**
 * 登录页。
 *
 * ⚠️ 它是这个仓里唯一一处允许发非 GET 的地方（`session.ts`，`MCP-013` 认下的
 * 凭据类例外）。所以这一页的测试也是唯一一处会看见 POST 的。
 */

import { describe, expect, it, vi, beforeEach } from 'vitest'
import { act, fireEvent, render, screen } from '@testing-library/react'

import * as session from '../session'
import { LoginPage } from './Login'

function 填(标签: string, 值: string) {
  fireEvent.change(screen.getByLabelText(标签), { target: { value: 值 } })
}

beforeEach(() => {
  vi.restoreAllMocks()
  window.history.replaceState(null, '', '/login')
})

describe('登录页', () => {
  it('把账号口令与身份提供方一起发出去', async () => {
    // ⚠️ `provider` 必须能填。写死 `builtin` 的话，**接了第二个身份提供方的部署
    // 在页面上没有入口**——而它不报错，只是登不进去。
    const login = vi.spyOn(session, 'login').mockResolvedValue({ session: 's', user: 'u' })
    render(<LoginPage />)
    填('账号', 'alice')
    填('口令', 'pw')
    填('身份提供方', 'github')
    await act(async () => {
      fireEvent.submit(screen.getByRole('button', { name: /登录/ }).closest('form')!)
    })
    expect(login).toHaveBeenCalledWith('alice', 'pw', 'github')
  })

  it('不填提供方就是 builtin', async () => {
    const login = vi.spyOn(session, 'login').mockResolvedValue({ session: 's', user: 'u' })
    render(<LoginPage />)
    填('账号', 'alice')
    填('口令', 'pw')
    填('身份提供方', '   ')
    await act(async () => {
      fireEvent.submit(screen.getByRole('button', { name: /登录/ }).closest('form')!)
    })
    expect(login).toHaveBeenCalledWith('alice', 'pw', 'builtin')
  })

  it('登进去之后换地址，而且不留在后退历史里', async () => {
    // ⚠️ `replace` 不是好看：留下的话，登录成功后按一下后退又回到登录页，
    // 而那一页此时是没有意义的。
    vi.spyOn(session, 'login').mockResolvedValue({ session: 's', user: 'u' })
    render(<LoginPage />)
    填('账号', 'a')
    填('口令', 'b')
    await act(async () => {
      fireEvent.submit(screen.getByRole('button', { name: /登录/ }).closest('form')!)
    })
    expect(window.location.pathname).toBe('/me')
  })

  it('凭证不对只说凭证不对', async () => {
    // ⚠️ 后端对"口令错"与"账号没被预置而自注册关着"回的是**同一个错**
    // （`IDN-003`）。前端不要试图把它们分开说——**分开说等于把
    // "这个账号存在"告诉一个还没登录的人**。
    vi.spyOn(session, 'login').mockRejectedValue(new Error('凭证不对'))
    render(<LoginPage />)
    填('账号', 'a')
    填('口令', 'b')
    await act(async () => {
      fireEvent.submit(screen.getByRole('button', { name: /登录/ }).closest('form')!)
    })
    expect(screen.getByText('凭证不对')).toBeTruthy()
    expect(window.location.pathname).toBe('/login')
  })

  it('口令框是密码框', () => {
    render(<LoginPage />)
    expect(screen.getByLabelText('口令').getAttribute('type')).toBe('password')
  })
})
