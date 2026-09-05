/**
 * `BRD-008` / `BRD-009`：**渲染之后**的 DOM 里有什么。
 *
 * ⚠️ **这一份和 `markdown.test.ts` 不是重复的。** 那一份断言的是
 * `parseMarkdown` 交出来的**节点树**；`BRD-009` 说的是
 * "**必须实际构造恶意内容验证**（内联脚本、事件处理属性、外部资源引用），
 * **不能只做代码审查**"，而 RP-06 的验收写的是"**在 Web 上打开**：
 * 脚本不执行、外部资源不自动加载"。
 *
 * 节点树对、渲染错，是完全可能的——多加一个 `dangerouslySetInnerHTML`
 * 就够了，而节点树那一层一个字都不会变。
 * `scripts/frontend-discipline.mjs` 拦得住那个字符串，
 * **拦不住第二种把字符串塞进 DOM 的写法**。所以这一层要自己看。
 */

import { describe, expect, it } from 'vitest'
import { render } from '@testing-library/react'

import { MarkdownView } from './MarkdownView'

/** 渲染出来的那棵子树。 */
function 渲染(source: string): HTMLElement {
  return render(<MarkdownView source={source} />).container
}

describe('渲染之后的 DOM 里没有可执行的东西', () => {
  it('内联脚本进不去 DOM，只是字', () => {
    const dom = 渲染('<script>window.被执行了 = true</script>')
    expect(dom.querySelector('script')).toBeNull()
    expect(dom.textContent).toContain('<script>')
  })

  it('事件处理属性没有可以附着的地方', () => {
    const dom = 渲染('<img src=x onerror="window.被执行了 = true">')
    expect(dom.querySelector('img')).toBeNull()
    // 枚举整棵子树上的每一个属性：**不按名字挑几个来查**——
    // 漏掉的那一个不会报错。
    for (const element of dom.querySelectorAll('*')) {
      for (const attribute of element.attributes) {
        expect(attribute.name.startsWith('on')).toBe(false)
      }
    }
  })

  it('javascript 伪协议不会成为一个链接', () => {
    const dom = 渲染('[点我](javascript:window.被执行了=true)')
    expect(dom.querySelector('a')).toBeNull()
    expect(dom.textContent).toContain('点我')
  })

  it('外部资源一个都不加载', () => {
    const dom = 渲染(
      '![图](https://example.invalid/x.png)\n\n<iframe src="https://example.invalid"></iframe>',
    )
    // ⚠️ **按标签枚举，不按 Markdown 语法枚举**：会自己发请求的元素就这几种，
    // 而"我们的解析器不支持图片语法"是另一个层次的理由，靠不住。
    for (const tag of ['img', 'iframe', 'video', 'audio', 'source', 'embed', 'object', 'link']) {
      expect(dom.querySelector(tag), `${tag} 不该出现`).toBeNull()
    }
  })

  it('SVG 内嵌脚本同样只是字', () => {
    const dom = 渲染('<svg><script>window.被执行了 = true</script></svg>')
    expect(dom.querySelector('svg')).toBeNull()
    expect(dom.querySelector('script')).toBeNull()
  })

  it('http 与 https 链接是真链接，而且带 noopener', () => {
    const dom = 渲染('[官网](https://example.com/a)')
    const link = dom.querySelector('a')
    expect(link?.getAttribute('href')).toBe('https://example.com/a')
    // 链接可能指向被分析的仓里写下的地址——`rel` 上两样都要。
    expect(link?.getAttribute('rel')).toContain('noopener')
    expect(link?.getAttribute('rel')).toContain('noreferrer')
  })

  it('正常的标记照样渲染成元素', () => {
    // 反过来的那一半：**挡住一切的渲染器也是"安全"的，但它没用。**
    const dom = 渲染('# 标题\n\n一段**粗**字与 `代码`\n\n- 甲\n- 乙')
    expect(dom.querySelector('h1')?.textContent).toBe('标题')
    expect(dom.querySelector('strong')?.textContent).toBe('粗')
    expect(dom.querySelector('code')?.textContent).toBe('代码')
    expect(dom.querySelectorAll('li')).toHaveLength(2)
  })

  it('整棵子树上没有任何 dangerously 注入留下的痕迹', () => {
    // 一次总的兜底：把上面全部恶意片段拼在一起渲染，断言**没有一个元素**
    // 是脚本、也没有一个属性以 on 开头、也没有一个 src/href 指向 javascript:。
    const dom = 渲染(
      '<script>a</script>\n\n<img src=x onerror=b>\n\n[c](javascript:d)\n\n<svg onload=e>',
    )
    expect(dom.querySelectorAll('script, iframe, img, svg')).toHaveLength(0)
    for (const element of dom.querySelectorAll('*')) {
      for (const attribute of element.attributes) {
        expect(attribute.name.startsWith('on')).toBe(false)
        expect(attribute.value.toLowerCase().startsWith('javascript:')).toBe(false)
      }
    }
  })
})
