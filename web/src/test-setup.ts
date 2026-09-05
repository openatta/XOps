/**
 * 每条用例之间把 DOM 清干净。
 *
 * ⚠️ `@testing-library/react` 的自动清理只在 `afterEach` 存在时生效，
 * 而那要 `globals: true`。不清的话上一条用例渲染的东西还挂在 document 上，
 * 下一条 `getByText` 会找到两个——**症状是"明明只渲染了一次"**。
 */

import { afterEach } from 'vitest'
import { cleanup } from '@testing-library/react'

afterEach(cleanup)
