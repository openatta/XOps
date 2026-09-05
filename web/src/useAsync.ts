/**
 * 取一次数据。
 *
 * ⚠️ **它把"没登录"与"读不到"分开了。** 早先这里只有一个 `error: string`，
 * 于是 `App` 拿 `me()` 失败当成"该显示登录页"——**后端 500、网络抖一下，
 * 用户看到的都是登录页**，一个已经登录的人被告知请登录。
 * 只读面对没有会话回的是 401（`server.rs` 把 `Denied` 映成它），
 * 那一条与别的错必须分得开。
 */

import { useEffect, useState } from 'react'

import { ApiError } from './api'

export type Async<T> = {
  value: T | null
  /** 读不到的原因。**401 不进这里**——它走 `unauthorized`。 */
  error: string | null
  /** 后端说这次请求没有会话（401）。 */
  unauthorized: boolean
  loading: boolean
}

export function useAsync<T>(load: () => Promise<T>, deps: unknown[]): Async<T> {
  const [state, setState] = useState<Async<T>>({
    value: null,
    error: null,
    unauthorized: false,
    loading: true,
  })
  useEffect(() => {
    let alive = true
    setState((previous) => ({ ...previous, loading: true }))
    load()
      .then((value) => {
        if (alive) setState({ value, error: null, unauthorized: false, loading: false })
      })
      .catch((cause: unknown) => {
        if (!alive) return
        const denied = cause instanceof ApiError && cause.status === 401
        setState({
          value: null,
          error: denied ? null : cause instanceof ApiError ? cause.message : '读不到',
          unauthorized: denied,
          loading: false,
        })
      })
    return () => {
      alive = false
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps)
  return state
}
