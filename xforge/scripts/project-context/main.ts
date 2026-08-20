import { access, readFile } from 'node:fs/promises';
import path from 'node:path';

const root = process.cwd();
const candidates = ['package.json', 'pyproject.toml', 'go.mod', 'Cargo.toml'];
const detected: string[] = [];

for (const candidate of candidates) {
  try {
    await access(path.join(root, candidate));
    detected.push(candidate);
  } catch {
    // An absent ecosystem file is expected.
  }
}

const manifest = await readFile(path.join(root, 'xforge', 'manifest.yaml'), 'utf8');
process.stdout.write(`${JSON.stringify({ root, detected, manifestBytes: Buffer.byteLength(manifest) })}\n`);
