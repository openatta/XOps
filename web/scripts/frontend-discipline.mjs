#!/usr/bin/env node
/**
 * 前端的两条纪律，都要**有一个检查，不是靠人看**。
 *
 * 一、`BRD-005` 第 ② 道：**前端不存在调用写接口的代码路径**，用检查兜住
 * "就加一个按钮"这种持续压力。
 *
 * 它的作用是让越界在**评审阶段**被看见，而不是等它撞上 404。
 * ⚠️ **顺序不能反**：第 ① 道（后端不存在写路由）在 RP-05 那边，已经在了。
 * 只有 ② 没有 ①，等于把一条安全属性交给前端自觉。
 *
 * 唯一豁免的是 `src/session.ts`——`MCP-013` 认下的那个凭据类例外。
 * **豁免写在这里，不写在注释里**，所以"再开一个口子"必须先改这个文件。
 */

import { readdirSync, readFileSync, statSync } from 'node:fs'
import { dirname, join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'

/**
 * 二、`BRD-002`/`BRD-003`：**没有报表。** 枚举全部视图，不存在任何图表、趋势、
 * 聚合或跨项目对比。判断标准很直白：**如果有一天需要在平台代码里写"什么是缺陷密度"，
 * 那就越界了**——而它最先会以一个图表库的 import 出现。
 */

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const EXEMPT = ['src/session.ts']
/** 非 GET 的 fetch，以及任何直接写 DOM 的注入面。 */
const FORBIDDEN = [
  { pattern: /method:\s*['"](POST|PUT|PATCH|DELETE)['"]/g, why: '调用了写接口' },
  { pattern: /dangerouslySetInnerHTML/g, why: '把 HTML 字符串塞进了 DOM（BRD-008）' },
  { pattern: /\.innerHTML\s*=/g, why: '把 HTML 字符串塞进了 DOM（BRD-008）' },
  { pattern: /new\s+Function\s*\(/g, why: '在运行时构造代码' },
  { pattern: /\beval\s*\(/g, why: '在运行时求值' },
  { pattern: /<canvas/gi, why: '画了个图（BRD-002：平台不内建任何报表）' },
  { pattern: /\bchart\b/gi, why: '出现了图表（BRD-002）' },
]

/** 一出现就说明报表进来了的依赖。 */
const FORBIDDEN_DEPENDENCIES = [
  'chart.js',
  'recharts',
  'echarts',
  'victory',
  'nivo',
  'plotly',
  'd3',
  'apexcharts',
]

/**
 * 去掉注释再看代码。**纪律写在注释里是正常的**——这个文件自己就在注释里
 * 提到了那几个被禁的名字。naive 的切法在这个仓里够用：没有字符串字面量含 `//`。
 */
function codeOnly(source) {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .split('\n')
    .map((line) => line.split('//')[0])
    .join('\n')
}

function walk(directory) {
  const out = []
  for (const entry of readdirSync(directory)) {
    if (entry === 'node_modules' || entry === 'dist') continue
    const path = join(directory, entry)
    if (statSync(path).isDirectory()) out.push(...walk(path))
    else if (/\.(ts|tsx|js|jsx|mjs)$/.test(entry)) out.push(path)
  }
  return out
}

const offences = []
for (const path of walk(join(root, 'src'))) {
  const name = relative(root, path)
  if (EXEMPT.includes(name)) continue
  const source = codeOnly(readFileSync(path, 'utf8'))
  for (const { pattern, why } of FORBIDDEN) {
    for (const match of source.matchAll(pattern)) {
      const line = source.slice(0, match.index).split('\n').length
      offences.push(`  ${name}:${line}  ${why} —— ${match[0]}`)
    }
  }
}

const manifest = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8'))
const declared = Object.keys({ ...manifest.dependencies, ...manifest.devDependencies })
for (const forbidden of FORBIDDEN_DEPENDENCIES) {
  if (declared.some((name) => name === forbidden || name.startsWith(`${forbidden}/`))) {
    offences.push(`  package.json  依赖了图表库 ${forbidden}（BRD-002：平台不内建任何报表）`)
  }
}

if (offences.length > 0) {
  console.error('前端里出现了不该有的东西：\n')
  console.error(offences.join('\n'))
  console.error(
    '\n看板是只读的。要写就发一次 MCP 调用 —— 页面该给的是"在你的 Agent 里跑这条命令"，不是一个按钮。',
  )
  process.exit(1)
}

console.log(
  `前端检查通过：${EXEMPT.length} 个凭据类豁免之外没有任何写调用，也没有任何报表。`,
)
