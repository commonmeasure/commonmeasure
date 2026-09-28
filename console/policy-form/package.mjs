#!/usr/bin/env node
// Deterministic byte-copy package; Rust embeds the very same source files.
import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync, mkdirSync, copyFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { resolve, join } from 'node:path';
const root = fileURLToPath(new URL('../../', import.meta.url));
const dir = join(root, 'console/policy-form');
const files = {
  'policy-form.mjs': 'console/policy-form/policy-form.mjs',
  'policy-form.css': 'console/policy-form/policy-form.css',
  'source-policy.schema.json': 'docs/contracts/source-policy.schema.json'
};
const sha = bytes => createHash('sha256').update(bytes).digest('hex');
const entries = Object.fromEntries(Object.entries(files).map(([name, source]) => [name, {source, sha256: sha(readFileSync(join(root, source)))}]));
const manifest = {contract: 'commonmeasure-policy-form/v1', files: entries, sha256: sha(JSON.stringify(entries))};
const encoded = JSON.stringify(manifest, null, 2) + '\n';
const [mode, destination] = process.argv.slice(2);
if (mode === '--write') writeFileSync(join(dir, 'manifest.json'), encoded);
else {
  if (readFileSync(join(dir, 'manifest.json'), 'utf8') !== encoded) throw new Error('Shared form manifest differs; run package.mjs --write after reviewing changes.');
  if (mode === '--output' && destination) {
    const out = resolve(destination); mkdirSync(out, {recursive:true});
    for (const [name, source] of Object.entries(files)) copyFileSync(join(root, source), join(out, name));
    const revision = execFileSync('git', ['rev-parse', 'HEAD'], {cwd:root, encoding:'utf8'}).trim();
    writeFileSync(join(out, 'UPSTREAM.json'), JSON.stringify({...manifest, repository:'commonmeasure/edge-private', revision}, null, 2) + '\n');
  } else if (mode !== '--check') throw new Error('Use --write, --check or --output <directory>.');
}
console.log(`${manifest.contract} sha256:${manifest.sha256}`);
