/**
 * 受限的 Markdown 解析。
 *
 * `BRD-008`：**长文本（Markdown）列渲染时必须禁用内联 HTML 与脚本，只渲染受限标记子集，
 * 外部资源不自动加载。** 那一条还写了它为什么是这条攻击链上唯一的门：
 *
 * > 绝大多数 Markdown 渲染库默认开启内联 HTML，而这些内容部分来自被分析的代码仓——
 * > **能往那个仓提交代码的人，就能影响它。**
 *
 * 所以这里不引渲染库，而是自己解析成一棵**节点树**，由 React 渲染成元素。
 * 这条路上**从头到尾没有 HTML 字符串**，因而也没有 `dangerouslySetInnerHTML`——
 * 注入不是被过滤掉的，是没有地方可注。
 *
 * 支持的子集，就这些：标题 · 段落 · 无序列表 · 引用 · 围栏代码块 ·
 * 行内代码 · 粗体 · 斜体 · 链接（**只认 http/https**）。
 *
 * **刻意不支持**：图片与任何嵌入（外部资源因此不会自动加载）· 表格 · 原始 HTML ·
 * 自动链接。不支持的写法一律当**纯文本**处理。
 */

export type Inline =
  | { kind: 'text'; text: string }
  | { kind: 'code'; text: string }
  | { kind: 'strong'; children: Inline[] }
  | { kind: 'emphasis'; children: Inline[] }
  | { kind: 'link'; href: string; children: Inline[] }

export type Block =
  | { kind: 'heading'; level: 1 | 2 | 3; children: Inline[] }
  | { kind: 'paragraph'; children: Inline[] }
  | { kind: 'list'; items: Inline[][] }
  | { kind: 'quote'; children: Inline[] }
  | { kind: 'code'; language: string; text: string }

/** 只有这两种协议的链接会被渲染成链接，别的一律降级成纯文本。 */
const ALLOWED_PROTOCOLS = ['http://', 'https://']

export function isSafeHref(href: string): boolean {
  const trimmed = href.trim().toLowerCase()
  return ALLOWED_PROTOCOLS.some((protocol) => trimmed.startsWith(protocol))
}

export function parseMarkdown(source: string): Block[] {
  const lines = source.replace(/\r\n/g, '\n').split('\n')
  const blocks: Block[] = []
  let index = 0

  while (index < lines.length) {
    const line = lines[index] ?? ''

    if (line.trim() === '') {
      index += 1
      continue
    }

    // 围栏代码块。**里面的一切都是纯文本**，连行内标记都不解析。
    const fence = /^```(\S*)\s*$/.exec(line)
    if (fence) {
      const language = fence[1] ?? ''
      const body: string[] = []
      index += 1
      while (index < lines.length && !/^```\s*$/.test(lines[index] ?? '')) {
        body.push(lines[index] ?? '')
        index += 1
      }
      index += 1
      blocks.push({ kind: 'code', language, text: body.join('\n') })
      continue
    }

    const heading = /^(#{1,3})\s+(.*)$/.exec(line)
    if (heading) {
      const level = (heading[1] ?? '#').length as 1 | 2 | 3
      blocks.push({ kind: 'heading', level, children: parseInline(heading[2] ?? '') })
      index += 1
      continue
    }

    if (/^>\s?/.test(line)) {
      const body: string[] = []
      while (index < lines.length && /^>\s?/.test(lines[index] ?? '')) {
        body.push((lines[index] ?? '').replace(/^>\s?/, ''))
        index += 1
      }
      blocks.push({ kind: 'quote', children: parseInline(body.join(' ')) })
      continue
    }

    if (/^[-*]\s+/.test(line)) {
      const items: Inline[][] = []
      while (index < lines.length && /^[-*]\s+/.test(lines[index] ?? '')) {
        items.push(parseInline((lines[index] ?? '').replace(/^[-*]\s+/, '')))
        index += 1
      }
      blocks.push({ kind: 'list', items })
      continue
    }

    const body: string[] = []
    while (
      index < lines.length &&
      (lines[index] ?? '').trim() !== '' &&
      !/^(#{1,3}\s|>|[-*]\s|```)/.test(lines[index] ?? '')
    ) {
      body.push(lines[index] ?? '')
      index += 1
    }
    blocks.push({ kind: 'paragraph', children: parseInline(body.join(' ')) })
  }

  return blocks
}

export function parseInline(source: string): Inline[] {
  const out: Inline[] = []
  let rest = source
  let buffer = ''

  const flush = () => {
    if (buffer !== '') {
      out.push({ kind: 'text', text: buffer })
      buffer = ''
    }
  }

  while (rest.length > 0) {
    // 行内代码优先：它里面的一切都不再解析。
    const code = /^`([^`]+)`/.exec(rest)
    if (code) {
      flush()
      out.push({ kind: 'code', text: code[1] ?? '' })
      rest = rest.slice(code[0].length)
      continue
    }
    const strong = /^\*\*([^*]+)\*\*/.exec(rest)
    if (strong) {
      flush()
      out.push({ kind: 'strong', children: parseInline(strong[1] ?? '') })
      rest = rest.slice(strong[0].length)
      continue
    }
    const emphasis = /^\*([^*]+)\*/.exec(rest)
    if (emphasis) {
      flush()
      out.push({ kind: 'emphasis', children: parseInline(emphasis[1] ?? '') })
      rest = rest.slice(emphasis[0].length)
      continue
    }
    // 图片：**整段当纯文本**。不支持图片是"外部资源不自动加载"的实现方式——
    // 而它必须比链接先匹配，否则 `![alt](url)` 会退化成一个可点的链接。
    const image = /^!\[([^\]]*)\]\(([^)\s]*)\)/.exec(rest)
    if (image) {
      buffer += image[0]
      rest = rest.slice(image[0].length)
      continue
    }
    const link = /^\[([^\]]*)\]\(([^)\s]+)\)/.exec(rest)
    if (link) {
      const href = link[2] ?? ''
      flush()
      if (isSafeHref(href)) {
        out.push({ kind: 'link', href, children: parseInline(link[1] ?? '') })
      } else {
        // javascript: / data: / vbscript: 这类一律降级成纯文本 —— 连标签都不生成。
        out.push({ kind: 'text', text: link[0] })
      }
      rest = rest.slice(link[0].length)
      continue
    }
    // 别的字符逐个进缓冲区。**`<` 与 `&` 没有任何特殊待遇**——它们只是文本，
    // 而文本由 React 渲染成文本节点，不会变成标签。
    buffer += rest[0] ?? ''
    rest = rest.slice(1)
  }

  flush()
  return out
}

/** 把一棵树摊平成纯文本。测试与"复制原文"用它。 */
export function toPlainText(blocks: Block[]): string {
  const inline = (nodes: Inline[]): string =>
    nodes
      .map((node) => {
        switch (node.kind) {
          case 'text':
          case 'code':
            return node.text
          case 'strong':
          case 'emphasis':
            return inline(node.children)
          case 'link':
            return inline(node.children)
        }
      })
      .join('')

  return blocks
    .map((block) => {
      switch (block.kind) {
        case 'heading':
        case 'paragraph':
        case 'quote':
          return inline(block.children)
        case 'list':
          return block.items.map(inline).join('\n')
        case 'code':
          return block.text
      }
    })
    .join('\n')
}
