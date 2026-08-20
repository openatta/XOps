#!/usr/bin/env node
// Deterministic Git-history activity extractor for the xforge-kanban Skill.
// Reads only `git log` (plus, best-effort, `xforge state` for module grouping);
// never writes, never invents data not present in history.
// No third-party dependencies — Node.js built-ins only.
import { execFileSync } from 'node:child_process';
import process from 'node:process';

function parseArgs(argv) {
  const args = { root: '.', since: null, until: null, author: null };
  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index];
    if (flag === '--root') args.root = argv[++index];
    else if (flag === '--since') args.since = argv[++index];
    else if (flag === '--until') args.until = argv[++index];
    else if (flag === '--author') args.author = argv[++index];
  }
  return args;
}

function git(root, gitArgs) {
  return execFileSync('git', gitArgs, { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
}

function fail(message) {
  process.stderr.write(`${message}\n`);
  process.exit(1);
}

// Best-effort project.modules lookup via the already-parsed `xforge state` (no --change:
// this reads static project structure, not Change/Flow/Gate lifecycle state, so it does
// not violate this Skill's "independent of Change/Flow/Gate" invariant). Falls back to a
// single implicit root module when XForge is unavailable or this is not an XForge project,
// so the script keeps working standalone and single-module output stays unchanged.
function resolveModules(root) {
  // Try the globally installed CLI first, then the project-local pin: a project may use
  // either, and `npx --no-install` is the only way to reach a binary that lives in
  // node_modules/.bin rather than on PATH.
  const invocations = [
    ['xforge', ['state']],
    ['npx', ['--no-install', 'xforge', 'state']],
  ];
  for (const [command, args] of invocations) {
    try {
      const output = execFileSync(command, args, { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] });
      const parsed = JSON.parse(output);
      const modules = parsed?.data?.project?.modules;
      if (Array.isArray(modules) && modules.length > 0 && modules.every((module) => module && typeof module.id === 'string' && typeof module.path === 'string')) {
        return { modules: modules.map((module) => ({ id: module.id, path: module.path, kind: module.kind ?? null })), source: 'xforge-state' };
      }
    } catch {
      // This invocation form is unavailable — try the next one.
    }
  }
  // XForge CLI unavailable, not an XForge-managed project, or unparsable output — degrade.
  return { modules: [{ id: 'root', path: '.', kind: null }], source: 'implicit-root' };
}

// Longest-prefix-match: the module with the longest normalized path that contains
// filePath wins. A module declared with path "." (or "") matches everything and acts
// as the lowest-priority fallback, never overriding a more specific module.
function resolveModuleForPath(filePath, modules) {
  let match = null;
  let matchLength = -1;
  for (const module of modules) {
    const normalized = module.path === '.' || module.path === '' ? '' : module.path.replace(/^\.\//, '').replace(/\/+$/, '');
    const isMatch = normalized === '' || filePath === normalized || filePath.startsWith(`${normalized}/`);
    if (isMatch && normalized.length > matchLength) {
      match = module;
      matchLength = normalized.length;
    }
  }
  return match;
}

const args = parseArgs(process.argv.slice(2));

try {
  git(args.root, ['rev-parse', '--is-inside-work-tree']);
} catch {
  fail('Not inside a Git repository (or git is unavailable on PATH).');
}

let shallow = false;
try {
  shallow = git(args.root, ['rev-parse', '--is-shallow-repository']).trim() === 'true';
} catch {
  // Older Git versions lack this flag; leave shallow as unknown-false rather than fail.
}

// --no-merges: merge commits have no single meaningful diff for line-count attribution.
// \x01/\x02 are unlikely-to-collide field/record separators, safer than a printable delimiter.
const logArgs = ['log', '--no-merges', '--numstat', '--date=iso-strict', '--format=%x01%H%x02%ad%x02%an%x02%ae%x02%s'];
if (args.since) logArgs.push(`--since=${args.since}`);
if (args.until) logArgs.push(`--until=${args.until}`);
if (args.author) logArgs.push(`--author=${args.author}`);

let raw;
try {
  raw = git(args.root, logArgs);
} catch (error) {
  fail(`git log failed: ${error.message}`);
}

function emptyResult(moduleResolution) {
  return {
    ok: true,
    shallow,
    commitCount: 0,
    range: null,
    contributors: [],
    activity: {},
    typeBreakdown: {},
    moduleResolution: moduleResolution.source,
    modules: [],
    unscoped: { linesAdded: 0, linesDeleted: 0, fileCount: 0 },
    crossModuleCommits: [],
  };
}

const moduleResolution = resolveModules(args.root);

if (!raw || !raw.trim()) {
  process.stdout.write(`${JSON.stringify(emptyResult(moduleResolution), null, 2)}\n`);
  process.exit(0);
}

const CONVENTIONAL_TYPE = /^([a-z]+)(\([a-zA-Z0-9._-]+\))?!?:\s/;

const commits = [];
for (const chunk of raw.split('\x01').slice(1)) {
  const lines = chunk.split('\n');
  const header = lines[0] ?? '';
  const [hash, date, name, email, ...subjectParts] = header.split('\x02');
  const subject = subjectParts.join('\x02');
  const files = [];
  for (const line of lines.slice(1)) {
    if (!line.trim()) continue;
    const [addedField, deletedField, ...pathParts] = line.split('\t');
    const added = addedField !== '-' && !Number.isNaN(Number(addedField)) ? Number(addedField) : 0;
    const deleted = deletedField !== '-' && !Number.isNaN(Number(deletedField)) ? Number(deletedField) : 0;
    files.push({ path: pathParts.join('\t'), added, deleted });
  }
  const added = files.reduce((sum, file) => sum + file.added, 0);
  const deleted = files.reduce((sum, file) => sum + file.deleted, 0);
  const typeMatch = subject.match(CONVENTIONAL_TYPE);
  commits.push({ hash, date, name, email, subject, added, deleted, files, type: typeMatch ? typeMatch[1] : null });
}

// --- Global (whole-repository) aggregation, unchanged from prior behavior ---

const byEmail = new Map();
for (const commit of commits) {
  const key = commit.email || commit.name || 'unknown';
  if (!byEmail.has(key)) {
    byEmail.set(key, {
      email: commit.email || null,
      names: new Set(),
      commits: 0,
      added: 0,
      deleted: 0,
      days: new Set(),
      first: commit.date,
      last: commit.date,
    });
  }
  const entry = byEmail.get(key);
  entry.names.add(commit.name);
  entry.commits += 1;
  entry.added += commit.added;
  entry.deleted += commit.deleted;
  entry.days.add(commit.date.slice(0, 10));
  if (commit.date < entry.first) entry.first = commit.date;
  if (commit.date > entry.last) entry.last = commit.date;
}

const contributors = [...byEmail.values()]
  .map((entry) => ({
    email: entry.email,
    names: [...entry.names],
    commits: entry.commits,
    linesAdded: entry.added,
    linesDeleted: entry.deleted,
    activeDays: entry.days.size,
    firstCommit: entry.first,
    lastCommit: entry.last,
  }))
  .sort((left, right) => right.commits - left.commits);

// ISO weekday (1=Mon..7=Sun) x local hour, matching each commit's own recorded timezone offset.
const activity = {};
for (const commit of commits) {
  const parsed = new Date(commit.date);
  const isoWeekday = ((parsed.getDay() + 6) % 7) + 1;
  const hour = parsed.getHours();
  const key = `${isoWeekday}-${String(hour).padStart(2, '0')}`;
  activity[key] = (activity[key] ?? 0) + 1;
}

const typeBreakdown = {};
for (const commit of commits) {
  const key = commit.type ?? 'unclassified';
  if (!typeBreakdown[key]) typeBreakdown[key] = { count: 0, subjects: [] };
  typeBreakdown[key].count += 1;
  if (typeBreakdown[key].subjects.length < 20) typeBreakdown[key].subjects.push({ hash: commit.hash.slice(0, 9), subject: commit.subject });
}

// --- Per-module aggregation. Degrades to a single "root" module bucket (equivalent to
// the global numbers above) when moduleResolution.source is "implicit-root". ---

const moduleStats = new Map();
const unscoped = { linesAdded: 0, linesDeleted: 0, fileCount: 0 };
const crossModuleCommits = [];

function ensureModuleStats(module) {
  if (!moduleStats.has(module.id)) {
    moduleStats.set(module.id, {
      module,
      commitCount: 0,
      linesAdded: 0,
      linesDeleted: 0,
      contributorsByKey: new Map(),
      activity: {},
      typeBreakdown: {},
      first: null,
      last: null,
    });
  }
  return moduleStats.get(module.id);
}

for (const commit of commits) {
  const perCommitModuleLines = new Map();
  for (const file of commit.files) {
    const resolved = resolveModuleForPath(file.path, moduleResolution.modules);
    if (!resolved) {
      unscoped.linesAdded += file.added;
      unscoped.linesDeleted += file.deleted;
      unscoped.fileCount += 1;
      continue;
    }
    const entry = perCommitModuleLines.get(resolved.id) ?? { added: 0, deleted: 0, module: resolved };
    entry.added += file.added;
    entry.deleted += file.deleted;
    perCommitModuleLines.set(resolved.id, entry);
  }

  if (perCommitModuleLines.size > 1) {
    crossModuleCommits.push({ hash: commit.hash.slice(0, 9), subject: commit.subject, modules: [...perCommitModuleLines.keys()] });
  }

  for (const lines of perCommitModuleLines.values()) {
    const stats = ensureModuleStats(lines.module);
    stats.commitCount += 1;
    stats.linesAdded += lines.added;
    stats.linesDeleted += lines.deleted;
    if (!stats.first || commit.date < stats.first) stats.first = commit.date;
    if (!stats.last || commit.date > stats.last) stats.last = commit.date;

    const key = commit.email || commit.name || 'unknown';
    if (!stats.contributorsByKey.has(key)) {
      stats.contributorsByKey.set(key, {
        email: commit.email || null,
        names: new Set(),
        commits: 0,
        added: 0,
        deleted: 0,
        days: new Set(),
        first: commit.date,
        last: commit.date,
      });
    }
    const contributor = stats.contributorsByKey.get(key);
    contributor.names.add(commit.name);
    contributor.commits += 1;
    contributor.added += lines.added;
    contributor.deleted += lines.deleted;
    contributor.days.add(commit.date.slice(0, 10));
    if (commit.date < contributor.first) contributor.first = commit.date;
    if (commit.date > contributor.last) contributor.last = commit.date;

    const parsed = new Date(commit.date);
    const isoWeekday = ((parsed.getDay() + 6) % 7) + 1;
    const hour = parsed.getHours();
    const activityKey = `${isoWeekday}-${String(hour).padStart(2, '0')}`;
    stats.activity[activityKey] = (stats.activity[activityKey] ?? 0) + 1;

    const typeKey = commit.type ?? 'unclassified';
    if (!stats.typeBreakdown[typeKey]) stats.typeBreakdown[typeKey] = { count: 0, subjects: [] };
    stats.typeBreakdown[typeKey].count += 1;
    if (stats.typeBreakdown[typeKey].subjects.length < 20) stats.typeBreakdown[typeKey].subjects.push({ hash: commit.hash.slice(0, 9), subject: commit.subject });
  }
}

const modules = [...moduleStats.values()]
  .map((stats) => ({
    id: stats.module.id,
    path: stats.module.path,
    kind: stats.module.kind,
    commitCount: stats.commitCount,
    linesAdded: stats.linesAdded,
    linesDeleted: stats.linesDeleted,
    firstCommit: stats.first,
    lastCommit: stats.last,
    contributors: [...stats.contributorsByKey.values()]
      .map((contributor) => ({
        email: contributor.email,
        names: [...contributor.names],
        commits: contributor.commits,
        linesAdded: contributor.added,
        linesDeleted: contributor.deleted,
        activeDays: contributor.days.size,
        firstCommit: contributor.first,
        lastCommit: contributor.last,
      }))
      .sort((left, right) => right.commits - left.commits),
    activity: stats.activity,
    typeBreakdown: stats.typeBreakdown,
  }))
  .sort((left, right) => right.commitCount - left.commitCount);

const dates = commits.map((commit) => commit.date).sort();

process.stdout.write(`${JSON.stringify({
  ok: true,
  shallow,
  commitCount: commits.length,
  range: { from: dates[0], to: dates[dates.length - 1] },
  contributors,
  activity,
  typeBreakdown,
  moduleResolution: moduleResolution.source,
  modules,
  unscoped,
  crossModuleCommits,
}, null, 2)}\n`);
