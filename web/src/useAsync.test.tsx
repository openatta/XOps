/**
 * `useAsync` 把「没登录」与「读不到」分开这件事。
 *
 * ⚠️ 早先这里只有一个 `error: string`，于是 `App` 拿 `me()` 失败当成
 * "该显示登录页"——**后端 500、网络抖一下，用户看到的都是登录页**，
 * 一个已经登录的人被告知请登录，而真正的原因一个字都没显示出来。
 */

import { describe, expect, it } from 'vitest'
import { act, render, screen } from '@testing-library/react'

import { ApiError } from './api'
import { useAsync } from './useAsync'

function 探针({ load }: { load: () => Promise<string> }) {
  const state = useAsync<string>(load, [])
  return (
    <pre data-testid="state">
      {JSON.stringify({
        value: state.value,
        error: state.error,
        unauthorized: state.unauthorized,
        loading: state.loading,
      })}
    </pre>
  )
}

async function 跑(load: () => Promise<string>) {
  render(<探针 load={load} />)
  await act(async () => {})
  return JSON.parse(screen.getByTestId('state').textContent ?? '{}') as {
    value: string | null
    error: string | null
    unauthorized: boolean
    loading: boolean
  }
}

describe('useAsync', () => {
  it('读到了就是读到了', async () => {
    const state = await 跑(() => Promise.resolve('好'))
    expect(state).toEqual({ value: '好', error: null, unauthorized: false, loading: false })
  })

  it('401 走 unauthorized，而且不占用 error', async () => {
    // ⚠️ **只有它该把人送去登录页。** 混进 error 里，
    // "请登录"就会盖住每一种失败。
    const state = await 跑(() => Promise.reject(new ApiError(401, '请先登录')))
    expect(state.unauthorized).toBe(true)
    expect(state.error).toBeNull()
  })

  it('500 走 error，不冒充没登录', async () => {
    const state = await 跑(() => Promise.reject(new ApiError(500, '库炸了')))
    expect(state.unauthorized).toBe(false)
    expect(state.error).toBe('库炸了')
  })

  it('404 也走 error', async () => {
    const state = await 跑(() => Promise.reject(new ApiError(404, '不存在')))
    expect(state.unauthorized).toBe(false)
    expect(state.error).toBe('不存在')
  })

  it('不是 ApiError 的东西也有话说，不是一片空白', async () => {
    const state = await 跑(() => Promise.reject(new TypeError('网络断了')))
    expect(state.error).toBe('读不到')
    expect(state.unauthorized).toBe(false)
  })

  it('一开始是 loading，不是"读不到"', async () => {
    // 装载中与读不到画成一样，会让人去查一个还没发生的问题。
    render(<探针 load={() => new Promise(() => {})} />)
    const state = JSON.parse(screen.getByTestId('state').textContent ?? '{}')
    expect(state.loading).toBe(true)
    expect(state.error).toBeNull()
    expect(state.unauthorized).toBe(false)
  })
})
