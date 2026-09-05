#!/usr/bin/env node
/**
 * 契约基线的校验与合并 —— XOps 自己的那一半。
 *
 * 这个脚本是**过渡形态**，但它的格式不是。它逐条复刻了 XForge 契约治理在
 * `core/contract-delta.ts` 与 `core/contract-merger.ts` 里的解析与合并语义，
 * 为的是将来接 XForge 时 `docs/contracts/*.md` 能原样搬进 `xforge/contracts/`，
 * `docs/contracts/deltas/<change>/*.md` 能原样搬进 `xforge/changes/<change>/contracts/`，
 * 而不是重写一遍。**改这里的解析规则前先去核对那两个文件。**
 *
 * 两处刻意的不同，各有理由：
 *   1. XForge 在「合并后基线为空」时**删除**记录文件；这里改成写回 `(none)`。
 *      仓里现在一行实现都没有，三份基线长期为空是正常状态，删掉它们等于
 *      每次都要重建目录结构。
 *   2. XForge 的 delta 活在 Change 目录里，由 archive 触发合并；这里由人在提交前
 *      跑一次 `sync` 触发。基线不会自己前进，忘了跑的后果是延迟出现的 ——
 *      所以 `check` 会在还有未合并的 delta 时提醒。见 docs/contracts/README.md §5。
 *
 * 用法：
 *   node scripts/contracts.mjs dump      问二进制"你实际提供什么"，写进方言文件
 *   node scripts/contracts.mjs check     校验基线、delta、台账，**并比对基线与实现**
 *   node scripts/contracts.mjs sync      把 delta 合并进基线并删除已合并的 delta 目录
 *
 * # 两根模型（README §3）
 *
 * ```text
 * 基线   docs/contracts/*.md              上一次被批准的接口记录
 * 实现   docs/contracts/{api,rust,data}/  服务今天真正提供的接口，由 dump 自证
 * check = diff(基线, 实现)
 * ```
 *
 * ⚠️ **实现那一根以前是空的。** 基线是散文，`check` 只校验记录格式、delta 结构
 * 与台账——**没有任何东西证明代码长得跟基线一样**。一条加了但没登记的路由、
 * 一个删了但基线还留着的 tool，都不会被谁说一句。
 *
 * 现在补上了两面：`api:mcp.tool.*` 与 `api:http.paths.*`，
 * 来源是 `xopsd --dump-contracts`——**问的是装配好的进程，不是源码**。
 * `sql:*` 与 `rust:*` 还欠着，见 `check` 里那段注释。
 *
 * 无外部依赖，只用 Node 内置模块。有 `cargo xtask` 之后这几个子命令搬过去。
 */

import { readdir, readFile, writeFile, mkdir, rm, stat } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import path from 'node:path';
import process from 'node:process';

const ROOT = path.resolve(path.dirname(new URL(import.meta.url).pathname), '..');
const CONTRACTS_DIR = path.join(ROOT, 'docs/contracts');
const DELTAS_DIR = path.join(CONTRACTS_DIR, 'deltas');
const LEDGER_PATH = path.join(CONTRACTS_DIR, 'DECISIONS.yaml');
/** 直接位于 docs/contracts/ 下、但不是基线记录的文件。 */
const NOT_A_RECORD = new Set(['README.md', 'DECISIONS.yaml']);
/** 方言文件（实现那一根）落在哪。 */
const DIALECT_DIR = path.join(CONTRACTS_DIR, 'api');
const TOOLS_FILE = path.join(DIALECT_DIR, 'mcp-tools.txt');
const ROUTES_FILE = path.join(DIALECT_DIR, 'http-routes.txt');

/* ------------------------------------------------------------------ *
 * 解析：与 XForge core/contract-delta.ts 的正则逐字一致
 * ------------------------------------------------------------------ */

const SECTION_HEADER = /^## (ADDED|MODIFIED|REMOVED) Contract Elements[ \t]*$/;
const ELEMENT_HEADER = /^### Element:[ \t]*(.*)$/;
const EMPTY_ASSERTION = /^[ \t]*(?:[-*+][ \t]+)?\(none\)[ \t]*$/i;
/** `<kind>:<selector>`；kind 是小写 kebab，selector 不含空白。 */
const ELEMENT_ID = /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?:\S+$/;
const ELEMENT_ID_MAX = 512;

/**
 * 哪种 kind 属于哪份基线。
 *
 * 合并是**按文件名路由**的（delta 叫 rust.md 就并进 rust.md），所以把一条 `sql:` 元素
 * 写进 rust 的 delta，它会安安静静地落到错的基线里 —— 踩过一次，因此有了这张表。
 */
const KIND_DOMAIN = { api: 'api', rust: 'rust', sql: 'data' };

function checkKind(file, domain, id, where) {
  const kind = id.slice(0, id.indexOf(':'));
  const expected = KIND_DOMAIN[kind];
  if (expected === undefined) {
    fail(file, `${where} 的 kind "${kind}" 没有对应的基线。已知的只有：${Object.keys(KIND_DOMAIN).join(' / ')}`);
    return;
  }
  if (expected !== domain) {
    fail(file, `${where} 是一条 ${kind}: 元素，它属于 ${expected}.md，不属于 ${domain}.md —— 合并按文件名路由，放错了会落进错的基线`);
  }
}

/**
 * 把 HTML 注释里的内容抹掉，但**保留行数**——诊断报的是行号。
 * 注释掉的示例不是一份声明：TEMPLATE.md 里那段被注释的破坏性变更例子，
 * 照抄过去不该当场把 check 弄红。
 */
function blankHtmlComments(source) {
  return source.replace(/<!--[\s\S]*?-->/g, (match) => match.replace(/[^\n]/g, ''));
}

const problems = [];
const fail = (file, message) => problems.push({ file, message });

/** `## Elements` 正文里的元素块，块之间以 `### Element:` 为界。 */
function elementBlocks(source) {
  const headers = [...source.matchAll(/^### Element:\s*(.+?)\s*$/gm)];
  return headers.map((match, index) => {
    const start = match.index;
    const end = headers[index + 1]?.index ?? source.length;
    return { id: match[1].trim(), content: source.slice(start, end).trimEnd() };
  });
}

/** 基线记录 = `## Elements` 之前的散文 + 元素块 + 之后的其余章节。 */
function recordParts(source) {
  const match = /^## Elements\s*$/m.exec(source);
  if (!match) return { before: source.trimEnd(), after: '', blocks: [] };
  const bodyStart = source.indexOf('\n', match.index + match[0].length);
  const remainderStart = bodyStart < 0 ? source.length : bodyStart + 1;
  const remainder = source.slice(remainderStart);
  const next = /^## /m.exec(remainder);
  const body = next ? remainder.slice(0, next.index) : remainder;
  const after = next ? remainder.slice(next.index).trim() : '';
  return { before: source.slice(0, match.index).trimEnd(), after, blocks: elementBlocks(body) };
}

/** 渲染回基线记录。空基线写 `(none)`，不删文件（见文件头第 1 条不同）。 */
function render(before, after, blocks) {
  const body = blocks.length > 0 ? blocks.map((b) => b.content).join('\n\n') : '(none)';
  const rendered = `${before}\n\n## Elements\n\n${body}`;
  return `${rendered}${after ? `\n\n${after}` : ''}\n`;
}

function hasDeltaSections(source) {
  return source.split(/\r?\n/).some((line) => SECTION_HEADER.test(line));
}

/**
 * 解析一份 delta 的三节。`(none)` 是一条断言，不是省略 —— 一节既没有元素
 * 也没有 `(none)`，是作者漏写了，不是"这一节没变化"。
 */
function parseDelta(source, file, domain) {
  const lines = source.split(/\r?\n/);
  const sections = [];
  let current = null;
  let element = null;
  for (const [index, line] of lines.entries()) {
    const header = SECTION_HEADER.exec(line);
    if (header) {
      current = { operation: header[1], line: index + 1, elements: [], assertedEmpty: false };
      element = null;
      sections.push(current);
      continue;
    }
    if (/^## /.test(line)) { current = null; element = null; continue; }
    if (!current) continue;
    const elementHeader = ELEMENT_HEADER.exec(line);
    if (elementHeader) {
      const id = elementHeader[1].trim();
      element = { id, line: index + 1, content: line };
      current.elements.push(element);
      if (!ELEMENT_ID.test(id)) {
        fail(file, `第 ${index + 1} 行的元素 id "${id}" 不合法：形如 <kind>:<selector>，kind 是小写 kebab，selector 不含空白（空格要写成路径的一部分，例如 api:http.paths./boards.get）`);
      } else if (id.length > ELEMENT_ID_MAX) {
        fail(file, `第 ${index + 1} 行的元素 id 超过 ${ELEMENT_ID_MAX} 字符`);
      } else {
        checkKind(file, domain, id, `第 ${index + 1} 行`);
      }
      continue;
    }
    if (/^### /.test(line)) { element = null; continue; }
    if (element) { element.content += `\n${line}`; continue; }
    if (EMPTY_ASSERTION.test(line)) current.assertedEmpty = true;
  }
  for (const section of sections) {
    section.elements = section.elements.map((e) => ({ ...e, content: e.content.trimEnd() }));
    if (section.elements.length === 0 && !section.assertedEmpty) {
      fail(file, `第 ${section.line} 行的 "## ${section.operation} Contract Elements" 既没有元素也没有 (none)。空节必须写 (none) —— 那是一条断言，不是省略`);
    }
  }
  for (const operation of ['ADDED', 'MODIFIED', 'REMOVED']) {
    if (!sections.some((s) => s.operation === operation)) {
      fail(file, `缺少 "## ${operation} Contract Elements" 一节。五节必须齐全，无变化写 (none)`);
    }
  }
  /* 后两节是散文，解析器不读它们的内容，但缺了就是没看过。 */
  for (const heading of ['Breaking Changes', 'Consumer Impact']) {
    if (!new RegExp(`^## ${heading}[ \\t]*$`, 'm').test(source)) {
      fail(file, `缺少 "## ${heading}" 一节。五节必须齐全，无变化写 (none)`);
    }
  }
  return sections;
}

const sectionOf = (sections, operation) =>
  (sections.find((s) => s.operation === operation)?.elements ?? []).map((e) => ({ id: e.id, content: e.content }));

/* ------------------------------------------------------------------ *
 * 合并：与 XForge core/contract-merger.ts 的判定逐条一致
 * ------------------------------------------------------------------ */

function mergeInto(record, sections, file, domain) {
  const operations = [...sectionOf(sections, 'ADDED'), ...sectionOf(sections, 'MODIFIED'), ...sectionOf(sections, 'REMOVED')];
  if (operations.length === 0) return record;

  const parts = recordParts(record);
  const active = new Map();
  for (const block of parts.blocks) {
    if (active.has(block.id)) {
      fail(file, `基线 ${domain} 里 "${block.id}" 记了两次，合并无法判断指的是哪一块`);
      continue;
    }
    active.set(block.id, block);
  }
  for (const block of sectionOf(sections, 'ADDED')) {
    if (active.has(block.id)) {
      fail(file, `不能 ADD "${block.id}"：基线已经记着它。本次要改它就写进 MODIFIED`);
      continue;
    }
    active.set(block.id, block);
  }
  for (const block of sectionOf(sections, 'MODIFIED')) {
    if (!active.has(block.id)) {
      fail(file, `不能 MODIFY "${block.id}"：基线里没有它。本次引入它就写进 ADDED`);
      continue;
    }
    active.set(block.id, block);
  }
  for (const block of sectionOf(sections, 'REMOVED')) {
    if (!active.delete(block.id)) fail(file, `不能 REMOVE "${block.id}"：基线里没有它`);
  }
  return render(parts.before, parts.after, [...active.values()]);
}

/** 基线不存在时只允许 ADD —— 改一条从没被记录过的元素，是在声称一份不存在的记录。 */
function newRecord(sections, file, domain) {
  if (sectionOf(sections, 'MODIFIED').length > 0 || sectionOf(sections, 'REMOVED').length > 0) {
    fail(file, `基线 docs/contracts/${domain}.md 还不存在，本 delta 只能 ADD`);
    return null;
  }
  const added = sectionOf(sections, 'ADDED');
  if (added.length === 0) return null;
  return render(`# ${domain.replace(/-/g, ' ')}\n\n## Purpose\n\n由已合并的变更建立。`, '', added);
}

/* ------------------------------------------------------------------ *
 * 决策台账：字段名与 XForge 的 contractDecisions.yaml 完全一致
 * ------------------------------------------------------------------ */

const LEDGER_FIELDS = ['question', 'decision', 'decidedBy', 'decidedAt'];

/**
 * 一个只认识本台账那一种形状的 YAML 读取器。
 * 认：顶层 `condition:` 与 `entries:`（`[]` 或缩进列表）、条目下的标量与 `>-` / `|` 折叠块。
 * 不认的一律报错 —— 与其猜，不如让人把它写成认识的样子。
 */
function parseLedger(source, file) {
  const lines = source.split(/\r?\n/).filter((line) => !/^\s*#/.test(line));
  const ledger = { condition: null, entries: [] };
  let entry = null;
  let block = null;

  const flush = () => {
    if (!block) return;
    const text = block.lines.map((l) => l.slice(block.indent)).join(block.fold ? ' ' : '\n').trim();
    entry[block.key] = text;
    block = null;
  };

  for (const [index, raw] of lines.entries()) {
    const at = `第 ${index + 1} 行`;
    if (!raw.trim()) { if (block) block.lines.push(''); continue; }
    const indent = raw.length - raw.trimStart().length;

    if (block && indent >= block.indent) { block.lines.push(raw); continue; }
    flush();

    const line = raw.trim();
    if (indent === 0) {
      const top = /^([A-Za-z][A-Za-z0-9_]*):\s*(.*)$/.exec(line);
      if (!top) { fail(file, `${at} 不是一个顶层键：${line}`); continue; }
      entry = null;
      if (top[1] === 'condition') ledger.condition = unquote(top[2]);
      else if (top[1] === 'entries') {
        if (top[2].trim() === '[]') ledger.entries = [];
        else if (top[2].trim() !== '') fail(file, `${at} entries 只接受 [] 或缩进列表`);
      } else fail(file, `${at} 未知的顶层键 "${top[1]}"，本台账只有 condition 与 entries`);
      continue;
    }

    const item = /^-\s+([A-Za-z][A-Za-z0-9_]*):\s*(.*)$/.exec(line);
    if (item) {
      entry = { __line: index + 1 };
      ledger.entries.push(entry);
      assign(entry, item[1], item[2], indent + 2, file, at);
      continue;
    }
    const field = /^([A-Za-z][A-Za-z0-9_]*):\s*(.*)$/.exec(line);
    if (!field) { fail(file, `${at} 读不懂：${line}`); continue; }
    if (!entry) { fail(file, `${at} 的 "${field[1]}" 不属于任何条目`); continue; }
    assign(entry, field[1], field[2], indent, file, at);
  }
  flush();

  function assign(target, key, value, indent, file, at) {
    const folded = value.trim();
    if (folded === '>-' || folded === '>' || folded === '|' || folded === '|-') {
      block = { key, indent: indent + 2, lines: [], fold: folded.startsWith('>') };
      entry = target;
      return;
    }
    if (folded === '') { fail(file, `${at} 的 "${key}" 是空的`); return; }
    target[key] = unquote(folded);
  }

  if (ledger.condition !== 'contractDecisions') {
    fail(file, `顶层必须是 "condition: contractDecisions"，读到的是 "${ledger.condition}"`);
  }
  return ledger;
}

const unquote = (value) => value.replace(/^["']|["']$/g, '').trim();

function checkLedger(ledger, file) {
  const seen = new Set();
  for (const entry of ledger.entries) {
    const at = `第 ${entry.__line} 行的条目`;
    if (!entry.id) { fail(file, `${at} 没有 id`); continue; }
    if (!/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(entry.id)) {
      fail(file, `${at} 的 id "${entry.id}" 不合法：只允许字母、数字与 . _ -`);
    }
    if (seen.has(entry.id)) fail(file, `${at} 的 id "${entry.id}" 重复`);
    seen.add(entry.id);
    for (const field of LEDGER_FIELDS) {
      if (!entry[field]) fail(file, `条目 "${entry.id}" 缺 ${field} —— 四个字段名是硬编码的，没有别名`);
    }
    if (entry.decidedAt && Number.isNaN(Date.parse(entry.decidedAt))) {
      fail(file, `条目 "${entry.id}" 的 decidedAt "${entry.decidedAt}" 解析不了，用 ISO 8601（2026-09-01T10:00:00Z）`);
    }
    if (entry.decidedBy && /^(TBD|待定|某人|someone)$/i.test(entry.decidedBy)) {
      fail(file, `条目 "${entry.id}" 的 decidedBy 是占位符。一开始就写真实身份（Git author），事后改会被历史比对当场拒掉`);
    }
  }
  return new Set([...seen]);
}

/* ------------------------------------------------------------------ *
 * 装载
 * ------------------------------------------------------------------ */

const exists = async (target) => { try { await stat(target); return true; } catch { return false; } };

async function loadBaselines() {
  const records = new Map();
  for (const name of (await readdir(CONTRACTS_DIR)).sort()) {
    if (!name.endsWith('.md') || NOT_A_RECORD.has(name)) continue;
    const file = `docs/contracts/${name}`;
    const source = blankHtmlComments(await readFile(path.join(CONTRACTS_DIR, name), 'utf8'));
    if (hasDeltaSections(source)) {
      fail(file, '这是一份基线记录，不该出现 delta 的 "## ADDED Contract Elements" 一节');
      continue;
    }
    if (!/^## Elements\s*$/m.test(source)) {
      fail(file, '基线记录必须有一节 "## Elements"（空基线在它下面写 (none)）');
      continue;
    }
    const domain = path.basename(name, '.md');
    const parts = recordParts(source);
    const seen = new Set();
    for (const block of parts.blocks) {
      if (ELEMENT_ID.test(block.id)) {
        checkKind(file, domain, block.id, `元素 "${block.id}"`);
      } else {
        fail(file, `元素 id "${block.id}" 不合法`);
      }
      if (seen.has(block.id)) fail(file, `元素 "${block.id}" 记了两次`);
      seen.add(block.id);
    }
    records.set(domain, { file, source });
  }
  return records;
}

async function loadDeltas() {
  if (!(await exists(DELTAS_DIR))) return [];
  const deltas = [];
  for (const change of (await readdir(DELTAS_DIR, { withFileTypes: true })).sort((a, b) => a.name.localeCompare(b.name))) {
    if (!change.isDirectory()) continue;
    const dir = path.join(DELTAS_DIR, change.name);
    for (const name of (await readdir(dir)).sort()) {
      if (!name.endsWith('.md') || name === 'README.md') continue;
      const file = `docs/contracts/deltas/${change.name}/${name}`;
      const source = blankHtmlComments(await readFile(path.join(dir, name), 'utf8'));
      if (!hasDeltaSections(source)) {
        fail(file, 'delta 必须用元素 delta 的分节（## ADDED Contract Elements），不是一份基线记录');
        continue;
      }
      deltas.push({ change: change.name, domain: path.basename(name, '.md'), file, source, dir });
    }
  }
  return deltas;
}

/**
 * 破坏性变更必须指向台账里一条具名人拍板的记录。
 * 这是"修改契约需要人拍板"在文件层面的兑现 —— 现在也是唯一的一道，
 * 没有服务端强制，见 docs/contracts/README.md §5。
 */
function checkBreaking(delta, decisions) {
  for (const [index, line] of delta.source.split(/\r?\n/).entries()) {
    if (!/\*\*BREAKING\*\*/.test(line)) continue;
    const window = delta.source.split(/\r?\n/).slice(index, index + 8).join('\n');
    const reference = /decision:\s*`?DECISIONS\.yaml#([A-Za-z0-9][A-Za-z0-9._-]*)`?/.exec(window);
    if (!reference) {
      fail(delta.file, `第 ${index + 1} 行标了 **BREAKING**，但随后 8 行里没有 "decision: \`DECISIONS.yaml#<id>\`"。破坏性变更必须有具名人拍板`);
      continue;
    }
    if (!decisions.has(reference[1])) {
      fail(delta.file, `第 ${index + 1} 行指向的决策 "${reference[1]}" 不在 docs/contracts/DECISIONS.yaml 里`);
    }
  }
}


/* ------------------------------------------------------------------ *
 * 实现那一根：dump 与比对
 * ------------------------------------------------------------------ */

/**
 * 问二进制"你实际提供什么"。
 *
 * ⚠️ **问的是装配好的进程，不是源码。** 扫源码只能看见"写下来的"，
 * 而这个仓踩过的坑里有一整类是"写下来了但没接上"——
 * 装配层从来没调过 `with_provider`，源码里那个 `BuiltinProvider` 好好地待着。
 */
function askBinary() {
  const candidates = [
    path.join(ROOT, 'target/debug/xopsd'),
    path.join(ROOT, 'target/release/xopsd'),
  ];
  for (const binary of candidates) {
    try {
      const out = execFileSync(binary, ['--dump-contracts'], {
        encoding: 'utf8',
        env: { ...process.env, XOPS_LOG: 'off' },
        maxBuffer: 8 * 1024 * 1024,
      });
      return JSON.parse(out);
    } catch {
      // 下一个候选。
    }
  }
  return null;
}

/** dump 出来的东西写成方言文件 —— 一行一条，好 diff、好在评审里读。 */
async function writeDialects(dump) {
  await mkdir(DIALECT_DIR, { recursive: true });
  const tools = [...dump.tools].sort().join('\n');
  const routes = dump.routes
    .map((route) => `${route.path} ${route.method.toLowerCase()}`)
    .sort()
    .join('\n');
  await writeFile(TOOLS_FILE, `${tools}\n`, 'utf8');
  await writeFile(ROUTES_FILE, `${routes}\n`, 'utf8');
  return { tools: dump.tools.length, routes: dump.routes.length };
}

/** 基线里登记过的 CEID。 */
function baselineIds(source, prefix) {
  const ids = new Set();
  for (const line of source.split(/\r?\n/)) {
    const match = /^### Element:[ \t]*(\S+)/.exec(line);
    if (match && match[1].startsWith(prefix)) ids.add(match[1]);
  }
  return ids;
}

/**
 * 比一面。**两个方向都要报**：
 *
 * ```text
 * 实现有、基线没有   加了接口没登记 —— 悄悄多出来的那一条
 * 基线有、实现没有   删了接口没撤记 —— 基线在替一个不存在的东西背书
 * ```
 *
 * ⚠️ **后一条更容易被忽略**，因为它不影响任何人调用；它的代价是**读基线的人
 * 相信了一件假的事**，而基线存在的全部意义就是被相信。
 */
function compare(face, fromImpl, fromBaseline, out) {
  const extra = [...fromImpl].filter((id) => !fromBaseline.has(id)).sort();
  const missing = [...fromBaseline].filter((id) => !fromImpl.has(id)).sort();
  for (const id of extra) {
    out.push(`  实现里有、基线里没有：${id}\n    ——加了接口没登记。写一份 delta（${face}）再 sync。`);
  }
  for (const id of missing) {
    out.push(`  基线里有、实现里没有：${id}\n    ——删了接口没撤记。基线正在替一个不存在的东西背书。`);
  }
}

/** 拿 dump 出来的东西比基线。回一组问题描述。 */
function compareAgainstBaseline(dump, apiBaseline) {
  const out = [];
  compare(
    'api',
    new Set(dump.tools.map((name) => `api:mcp.tool.${name}`)),
    baselineIds(apiBaseline, 'api:mcp.tool.'),
    out,
  );
  compare(
    'api',
    new Set(dump.routes.map((r) => `api:http.paths.${r.path}.${r.method.toLowerCase()}`)),
    baselineIds(apiBaseline, 'api:http.paths.'),
    out,
  );
  return out;
}

/* ------------------------------------------------------------------ *
 * 子命令
 * ------------------------------------------------------------------ */

async function run(command) {
  const baselines = await loadBaselines();
  const deltas = await loadDeltas();

  if (command === 'dump') {
    const dump = askBinary();
    if (dump === null) {
      console.error('问不到二进制。先 `cargo build -p xopsd`，再跑一次。');
      return 1;
    }
    const counts = await writeDialects(dump);
    console.log(`已写方言文件：${counts.tools} 个 tool，${counts.routes} 条路由`);
    console.log(`  docs/contracts/api/mcp-tools.txt`);
    console.log(`  docs/contracts/api/http-routes.txt`);
    return 0;
  }

  let decisions = new Set();
  if (await exists(LEDGER_PATH)) {
    const ledger = parseLedger(await readFile(LEDGER_PATH, 'utf8'), 'docs/contracts/DECISIONS.yaml');
    decisions = checkLedger(ledger, 'docs/contracts/DECISIONS.yaml');
  } else {
    fail('docs/contracts/DECISIONS.yaml', '决策台账不存在。没有待拍板的事就写 entries: [] —— 那是一条断言，不是空文件');
  }

  const merged = new Map();
  for (const delta of deltas) {
    checkBreaking(delta, decisions);
    const sections = parseDelta(delta.source, delta.file, delta.domain);
    const baseline = merged.get(delta.domain) ?? baselines.get(delta.domain)?.source ?? null;
    const next = baseline === null
      ? newRecord(sections, delta.file, delta.domain)
      : mergeInto(baseline, sections, delta.file, delta.domain);
    if (next !== null && next !== baseline) merged.set(delta.domain, next);
  }

  if (problems.length > 0) {
    console.error(`契约校验不通过，${problems.length} 处：\n`);
    for (const item of problems) console.error(`  ${item.file}\n    ${item.message}\n`);
    return 1;
  }

  if (command === 'check') {
    // ⚠️ **有未合并的 delta 时不比对。** delta 声明的正是"这次要加的那些"，
    // 那时实现比基线多出几条是**正常状态**，报出来只会训练人忽略它。
    // 一个经常误报的检查等于没有检查。
    if (deltas.length === 0) {
      const dump = askBinary();
      if (dump === null) {
        console.log('⚠️  没问到二进制（先 `cargo build -p xopsd`），**这次没有比对基线与实现**。');
        console.log('   校验的只有记录格式、delta 结构与台账 —— 那是过去唯一有的那一半。');
      } else {
        const api = baselines.get('api')?.source ?? '';
        const drift = compareAgainstBaseline(dump, api);
        if (drift.length > 0) {
          console.error(`基线与实现对不上，${drift.length} 处：\n`);
          for (const line of drift) console.error(`${line}\n`);
          console.error('见 docs/contracts/README.md §3、§4。**实现与契约不一致时改实现**——');
          console.error('改契约是一个需要具名人拍板的决定。');
          return 1;
        }
        console.log(`基线与实现对得上：${dump.tools.length} 个 tool，${dump.routes.length} 条路由。`);
      }
    }
    console.log(`契约校验通过：${baselines.size} 份基线，${deltas.length} 份 delta，${decisions.size} 条决策。`);
    if (deltas.length > 0) {
      const changes = [...new Set(deltas.map((d) => d.change))].join('、');
      console.log(`\n⚠️  还有未合并进基线的 delta：${changes}`);
      console.log('   提交前跑 `node scripts/contracts.mjs sync`，否则基线不会前进，');
      console.log('   同一条元素会在下次被"首次登记"第二次。见 docs/contracts/README.md §5。');
    }
    return 0;
  }

  if (merged.size === 0 && deltas.length === 0) {
    console.log('没有待合并的 delta。');
    return 0;
  }
  for (const [domain, content] of merged) {
    await writeFile(path.join(CONTRACTS_DIR, `${domain}.md`), content, 'utf8');
    console.log(`已合并进基线：docs/contracts/${domain}.md`);
  }
  for (const dir of new Set(deltas.map((d) => d.dir))) {
    await rm(dir, { recursive: true, force: true });
    console.log(`已删除已合并的 delta：${path.relative(ROOT, dir)}`);
  }
  return 0;
}

const command = process.argv[2];
if (!['check', 'sync', 'dump'].includes(command)) {
  console.error('用法：node scripts/contracts.mjs <dump|check|sync>');
  process.exit(2);
}
process.exit(await run(command));
