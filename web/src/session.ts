/**
 * 会话面。**全前端唯一允许出现非 GET 请求的文件。**
 *
 * 它对应 `MCP-013` 认下的那个凭据类例外：只建立与销毁会话，**不写任何业务对象**。
 * `scripts/no-write-calls.mjs` 把这个文件列为唯一豁免——豁免写在检查脚本里，
 * 而不是写在注释里，所以"再开一个口子"这件事必须先改那个脚本。
 */

export async function login(account: string, secret: string, provider = 'builtin') {
  const response = await fetch('/session', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    credentials: 'same-origin',
    body: JSON.stringify({ provider, account, secret }),
  })
  if (!response.ok) throw new Error('凭证不对')
  return (await response.json()) as { session: string; user: string }
}

export async function logout() {
  await fetch('/session', { method: 'DELETE', credentials: 'same-origin' })
}
