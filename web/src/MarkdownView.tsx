/**
 * 把受限 Markdown 的节点树渲染成 React 元素。
 *
 * ⚠️ **这个文件里没有 `dangerouslySetInnerHTML`，整个仓也不该有。**
 * 节点树到元素是一一对应的映射，文本走文本节点——注入不是被过滤掉的，是没有地方可注。
 */

import type { Block, Inline } from './markdown'
import { parseMarkdown } from './markdown'

function renderInline(nodes: Inline[]): React.ReactNode {
  return nodes.map((node, index) => {
    switch (node.kind) {
      case 'text':
        return <span key={index}>{node.text}</span>
      case 'code':
        return <code key={index}>{node.text}</code>
      case 'strong':
        return <strong key={index}>{renderInline(node.children)}</strong>
      case 'emphasis':
        return <em key={index}>{renderInline(node.children)}</em>
      case 'link':
        // rel 里的 noreferrer 一并给上：链接可能指向被分析的仓里写下的地址。
        return (
          <a key={index} href={node.href} target="_blank" rel="noopener noreferrer">
            {renderInline(node.children)}
          </a>
        )
    }
  })
}

function renderBlock(block: Block, index: number): React.ReactNode {
  switch (block.kind) {
    case 'heading': {
      const Tag = (['h1', 'h2', 'h3'] as const)[block.level - 1] ?? 'h3'
      return <Tag key={index}>{renderInline(block.children)}</Tag>
    }
    case 'paragraph':
      return <p key={index}>{renderInline(block.children)}</p>
    case 'quote':
      return <blockquote key={index}>{renderInline(block.children)}</blockquote>
    case 'list':
      return (
        <ul key={index}>
          {block.items.map((item, position) => (
            <li key={position}>{renderInline(item)}</li>
          ))}
        </ul>
      )
    case 'code':
      return (
        <pre key={index} data-language={block.language}>
          <code>{block.text}</code>
        </pre>
      )
  }
}

export function MarkdownView({ source }: { source: string }) {
  return <div className="markdown">{parseMarkdown(source).map(renderBlock)}</div>
}
