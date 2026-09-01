import { describe, expect, it } from 'vitest'

import { isSafeHref, parseInline, parseMarkdown, toPlainText } from './markdown'

/**
 * `BRD-009`：**必须实际构造恶意内容验证，不能只做代码审查。**
 *
 * 每一条都是一段"能往被分析的那个仓提交代码的人"写得出来的东西。
 */
describe('受限渲染挡得住什么', () => {
  it('内联脚本只是文本', () => {
    const blocks = parseMarkdown('# 标题\n\n<script>alert(1)</script>')
    const nodes = blocks.flatMap((block) =>
      block.kind === 'paragraph' || block.kind === 'heading' ? block.children : [],
    )
    expect(nodes.every((node) => node.kind === 'text' || node.kind === 'code')).toBe(true)
    // 一个字都没丢，但它是文本节点 —— 由 React 渲染成文本，不会变成标签。
    expect(toPlainText(blocks)).toContain('<script>alert(1)</script>')
  })

  it('事件处理属性没有可以附着的地方', () => {
    const blocks = parseMarkdown('<img src=x onerror=alert(1)>')
    expect(blocks.every((block) => block.kind === 'paragraph')).toBe(true)
    // 关键不是"onerror 这几个字被删掉了"—— 它当然还在，它是文本。
    // 关键是**整棵树里没有一个能带属性的节点**：唯一带值的属性是链接的 href，
    // 而它过 isSafeHref。属性没有可以附着的地方，事件处理器就无从谈起。
    const attributed = JSON.stringify(blocks).match(/"kind":"(?!text|code|paragraph)/g)
    expect(attributed).toBeNull()
  })

  it('javascript 伪协议链接降级成纯文本', () => {
    const nodes = parseInline('[点我](javascript:alert(1))')
    expect(nodes.every((node) => node.kind === 'text')).toBe(true)
    expect(isSafeHref('javascript:alert(1)')).toBe(false)
    expect(isSafeHref('data:text/html;base64,PHNjcmlwdD4=')).toBe(false)
    expect(isSafeHref('  JavaScript:alert(1)')).toBe(false)
    expect(isSafeHref('vbscript:msgbox(1)')).toBe(false)
  })

  it('http 与 https 链接才成为链接', () => {
    const nodes = parseInline('[官网](https://example.com) 与 [内网](http://10.0.0.1)')
    const links = nodes.filter((node) => node.kind === 'link')
    expect(links).toHaveLength(2)
    expect(isSafeHref('https://example.com')).toBe(true)
  })

  it('外部资源不会自动加载：图片与嵌入根本不支持', () => {
    const blocks = parseMarkdown('![头像](https://evil.example/track.png)\n\n<iframe src="x">')
    const rendered = JSON.stringify(blocks)
    expect(rendered).not.toContain('"kind":"image"')
    expect(rendered).not.toContain('"kind":"iframe"')
    // 它们只是文本。
    expect(toPlainText(blocks)).toContain('![头像]')
  })

  it('SVG 内嵌脚本同样只是文本', () => {
    const blocks = parseMarkdown('<svg><script>alert(1)</script></svg>')
    expect(blocks.every((block) => block.kind === 'paragraph')).toBe(true)
    const attributed = JSON.stringify(blocks).match(/"kind":"(?!text|code|paragraph)/g)
    expect(attributed).toBeNull()
  })

  it('围栏代码块里的一切都是纯文本', () => {
    const blocks = parseMarkdown('```html\n<script>alert(1)</script>\n```')
    expect(blocks).toHaveLength(1)
    const [block] = blocks
    expect(block?.kind).toBe('code')
    if (block?.kind === 'code') {
      expect(block.text).toBe('<script>alert(1)</script>')
      expect(block.language).toBe('html')
    }
  })

  it('实体与尖括号没有特殊待遇', () => {
    const nodes = parseInline('&lt;script&gt; 与 <b>粗</b>')
    expect(nodes.every((node) => node.kind === 'text')).toBe(true)
  })
})

describe('受限渲染支持什么', () => {
  it('标题、段落、列表、引用、代码', () => {
    const blocks = parseMarkdown(
      '# 一级\n\n一段话。\n\n- 甲\n- 乙\n\n> 引用\n\n```\n代码\n```',
    )
    expect(blocks.map((block) => block.kind)).toEqual([
      'heading',
      'paragraph',
      'list',
      'quote',
      'code',
    ])
  })

  it('粗体、斜体、行内代码', () => {
    const nodes = parseInline('**粗** *斜* `码`')
    expect(nodes.map((node) => node.kind)).toEqual([
      'strong',
      'text',
      'emphasis',
      'text',
      'code',
    ])
  })

  it('行内代码里的星号不再被解析', () => {
    const nodes = parseInline('`**不是粗体**`')
    expect(nodes).toHaveLength(1)
    expect(nodes[0]).toEqual({ kind: 'code', text: '**不是粗体**' })
  })
})
