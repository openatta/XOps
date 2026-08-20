/**
 * 骨架自身的验证（SKEL-001 / 003 / 005 / 006）。
 *
 * 这些测试通过**造出失败**来证明三条命令确实在处理新增的包——只断言
 * "命令退出 0" 是证明不了什么的：空仓库下什么都不做也是 0。
 *
 * 注意：断言测试命令覆盖范围时**不能**调用 `pnpm run test`，那会递归调用
 * 本文件自己。改用带路径过滤的 vitest 调用，只跑探针那一个文件。
 */
import { spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";
import { afterEach, describe, expect, test } from "vitest";

const ROOT = process.cwd();
const PROBE = join(ROOT, "packages", "probe-tmp");
const STRAY = join(ROOT, "stray-tmp");
const TIMEOUT = 180_000;

function run(program: string, args: string[]): { status: number; output: string } {
  const r = spawnSync(program, args, { cwd: ROOT, encoding: "utf8", shell: false });
  return { status: r.status ?? -1, output: `${r.stdout ?? ""}${r.stderr ?? ""}` };
}

/** 建一个最小 workspace 包，**不修改任何根配置**——这正是 SKEL-001 要证明的。 */
function writeProbe(source: string, testFile?: string): void {
  mkdirSync(PROBE, { recursive: true });
  writeFileSync(
    join(PROBE, "package.json"),
    JSON.stringify({ name: "@xops/probe-tmp", version: "0.0.0", private: true, type: "module" }),
  );
  writeFileSync(join(PROBE, "index.ts"), source);
  if (testFile !== undefined) writeFileSync(join(PROBE, "probe.test.ts"), testFile);
}

afterEach(() => {
  rmSync(PROBE, { recursive: true, force: true });
  rmSync(STRAY, { recursive: true, force: true });
  // 探针删除后不要留下空的 packages/ —— 骨架不铺空目录树。
  // 只在它确实为空时删除：一旦项目真的有了包，这里绝不能碰它们。
  const packagesDir = join(ROOT, "packages");
  if (existsSync(packagesDir) && readdirSync(packagesDir).length === 0) {
    rmdirSync(packagesDir);
  }
});

describe("SKEL-001 新增包无需修改根配置即被纳入", () => {
  test(
    "typecheck 覆盖新增包：注入类型错误后命令失败并指出该文件",
    () => {
      writeProbe('export const answer: number = 42;\n');
      expect(run("pnpm", ["run", "typecheck"]).status).toBe(0);

      writeProbe('export const answer: number = "not a number";\n');
      const bad = run("pnpm", ["run", "typecheck"]);
      expect(bad.status).not.toBe(0);
      expect(bad.output).toContain("probe-tmp/index.ts");
      expect(bad.output).toContain("TS2322");
    },
    TIMEOUT,
  );

  test(
    "lint 覆盖新增包：注入未使用变量后命令失败并命中规则",
    () => {
      writeProbe("const unused = 1;\nexport const answer = 42;\n");
      const bad = run("pnpm", ["run", "lint"]);
      expect(bad.status).not.toBe(0);
      expect(bad.output).toContain("no-unused-vars");
    },
    TIMEOUT,
  );

  test(
    "test 覆盖新增包：探针包内的测试会被收集并执行",
    () => {
      writeProbe(
        "export const answer = 42;\n",
        'import { test, expect } from "vitest";\nimport { answer } from "./index.js";\ntest("probe", () => { expect(answer).toBe(42); });\n',
      );
      // 路径过滤：只跑探针那一个文件，避免递归调用本测试文件。
      const ok = run("pnpm", ["exec", "vitest", "run", "packages/probe-tmp"]);
      expect(ok.status).toBe(0);
      expect(ok.output).toContain("probe.test.ts");
    },
    TIMEOUT,
  );

  test(
    "布局之外的目录不被当作 workspace 包，也不使命令失败",
    () => {
      mkdirSync(STRAY, { recursive: true });
      writeFileSync(join(STRAY, "thing.ts"), "export const x = 1;\n");

      expect(run("pnpm", ["run", "typecheck"]).status).toBe(0);

      const listed = run("pnpm", ["ls", "-r", "--depth", "-1", "--json"]);
      expect(listed.output).not.toContain("stray-tmp");
    },
    TIMEOUT,
  );
});

describe("SKEL-003 / 005 / 006 根配置约束", () => {
  const pkg = JSON.parse(readFileSync(join(ROOT, "package.json"), "utf8")) as {
    engines?: { node?: string };
    dependencies?: Record<string, string>;
  };

  test("engines 声明的最低 Node 主版本不低于 20", () => {
    const declared = pkg.engines?.node;
    expect(declared).toBeDefined();
    const floor = Number(/(\d+)/.exec(declared ?? "")?.[1]);
    expect(floor).toBeGreaterThanOrEqual(20);
  });

  // 这条测试来自一次真实的 CI 失败：engines 曾声明 >=20，而 packageManager
  // 钉的 pnpm 要求 Node >=22.13，CI 照着 20 跑于是必然失败。声明的下限如果
  // 没有被 CI 真正跑到，它就只是一句没人验证的话。
  test("CI 矩阵的最低 Node 版本等于 engines 声明的下限", () => {
    const workflow = readFileSync(join(ROOT, ".github", "workflows", "ci.yml"), "utf8");
    const matrix = /node:\s*\[([^\]]+)\]/.exec(workflow)?.[1];
    expect(matrix).toBeDefined();

    const versions = (matrix ?? "").split(",").map((v) => Number(v.trim()));
    expect(versions.length).toBeGreaterThan(0);

    const declaredFloor = Number(/(\d+)/.exec(pkg.engines?.node ?? "")?.[1]);
    expect(Math.min(...versions)).toBe(declaredFloor);
  });

  test("骨架不引入任何产品运行时依赖", () => {
    expect(pkg.dependencies ?? {}).toEqual({});
  });

  test("依赖与构建产物被版本控制忽略", () => {
    const ignored = readFileSync(join(ROOT, ".gitignore"), "utf8");
    for (const entry of ["node_modules/", "dist/", "coverage/"]) {
      expect(ignored).toContain(entry);
    }
  });
});
