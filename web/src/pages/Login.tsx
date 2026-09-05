/**
 * 登录页。
 *
 * ⚠️ **它有一个地址（`/login`），不再是"`me()` 读失败时的兜底"。**
 * 早先那个写法把两件事混成了一件：没有会话与后端出错都渲染登录表单，
 * 于是**一次 500 会告诉一个已经登录的人"请登录"**。
 * 现在只有 401 会把人送到这里（`useAsync` 把它单独分出来），别的错原样显示。
 *
 * `provider` 是可填的。后端 `Directory::login` 按 provider 列表查
 * （`crates/xops-identity/src/directory.rs`），写死 `builtin` 的话
 * **接了第二个身份提供方的部署在页面上没有入口**——而它不报错，只是登不进去。
 */

import { useState } from 'react'

import { login } from '../session'
import { HOME, navigate } from '../router'

export function LoginPage() {
  const [account, setAccount] = useState('')
  const [secret, setSecret] = useState('')
  const [provider, setProvider] = useState('builtin')
  const [failed, setFailed] = useState(false)
  const [busy, setBusy] = useState(false)

  return (
    <form
      className="login"
      onSubmit={(event) => {
        event.preventDefault()
        setBusy(true)
        login(account, secret, provider.trim() || 'builtin')
          .then(() => navigate(HOME, true))
          .catch(() => {
            setFailed(true)
            setBusy(false)
          })
      }}
    >
      <h1>XOps</h1>
      <label>
        账号
        <input
          value={account}
          autoComplete="username"
          onChange={(event) => setAccount(event.target.value)}
        />
      </label>
      <label>
        口令
        <input
          type="password"
          value={secret}
          autoComplete="current-password"
          onChange={(event) => setSecret(event.target.value)}
        />
      </label>
      <label>
        身份提供方
        <input
          value={provider}
          onChange={(event) => setProvider(event.target.value)}
          placeholder="builtin"
        />
      </label>
      <button type="submit" disabled={busy}>
        {busy ? '登录中…' : '登录'}
      </button>
      {/*
        ⚠️ 后端对"凭证不对"与"账号没被预置而自注册关着"回的是**同一个错**
        （`IDN-003`，且后者不创建任何用户记录）。前端不要试图把它们分开说——
        分开说等于把"这个账号存在"这件事告诉一个还没登录的人。
      */}
      {failed && <p className="error">凭证不对</p>}
    </form>
  )
}
