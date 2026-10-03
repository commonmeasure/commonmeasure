#!/usr/bin/env node
/** Generate or verify embedded CSS from the vendored website design release:
 * the token declarations for every consumer, and the shared application chrome
 * (chrome.css) for Hub and Edge. The contrast gate runs first on the token
 * source, so a palette that fails an obligation is never written into a
 * stylesheet. */
import { readFileSync, writeFileSync, copyFileSync, mkdirSync } from 'node:fs';
import { dirname, resolve, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';

const packageDir = dirname(fileURLToPath(import.meta.url));
const root = dirname(packageDir);
const args = process.argv.slice(2);
const check = args.includes('--check');
const verbose = args.includes('--verbose');
const source = readFileSync(join(packageDir, 'tokens.json'), 'utf8');
const chrome = readFileSync(join(packageDir, 'chrome.css'), 'utf8');
const tokens = JSON.parse(source);
const { consumer } = JSON.parse(readFileSync(join(packageDir, 'consumer.json'), 'utf8'));
if (!/^\d+\.\d+\.\d+$/.test(tokens.version)) throw new Error('Token version must be semver.');
const sha256 = (text) => createHash('sha256').update(text).digest('hex');
const digest = sha256(source);
const chromeDigest = sha256(chrome);
const files = ['tokens.json', 'chrome.css', 'generate.mjs', 'README.md'];
const targets = {
  site: [['ai/src/styles/cobalt.css', 'site-dark'], ['ai/src/styles/modern-light.css', 'site-light']],
  hub: [['web/src/routes/layout.css', 'hub']],
  edge: [['console/styles.css', 'edge'], ['docs/guide/guide.css', 'guide']],
};
if (!(consumer in targets)) throw new Error(`Unknown consumer ${consumer}`);

// --- the contrast gate: WCAG 2.2 obligations on the token source ---------

function oklchToLinear(l, c, hDeg) {
  const h = (hDeg * Math.PI) / 180;
  const a = c * Math.cos(h), b = c * Math.sin(h);
  const l_ = l + 0.3963377774 * a + 0.2158037573 * b;
  const m_ = l - 0.1055613458 * a - 0.0638541728 * b;
  const s_ = l - 0.0894841775 * a - 1.291485548 * b;
  const [l3, m3, s3] = [l_ ** 3, m_ ** 3, s_ ** 3];
  const clamp = (x) => Math.min(1, Math.max(0, x));
  return [
    clamp(4.0767416621 * l3 - 3.3077115913 * m3 + 0.2309699292 * s3),
    clamp(-1.2684380046 * l3 + 2.6097574011 * m3 - 0.3413193965 * s3),
    clamp(-0.0041960863 * l3 - 0.7034186147 * m3 + 1.707614701 * s3),
  ];
}
const hexToLinear = (hex) => [0, 2, 4].map((i) => {
  const v = parseInt(hex.slice(i, i + 2), 16) / 255;
  return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
});
/** Opaque colours only: shadows and translucent scrims are not text colours. */
function colours(theme) {
  const out = {};
  for (const [name, value] of Object.entries(theme)) {
    const hex = /^#([\da-f]{6})$/i.exec(value);
    const oklch = /^oklch\(([\d.]+)%\s+([\d.]+)\s+([\d.]+)\)$/.exec(value);
    if (hex) out[name] = hexToLinear(hex[1]);
    else if (oklch) out[name] = oklchToLinear(Number(oklch[1]) / 100, Number(oklch[2]), Number(oklch[3]));
  }
  return out;
}
const luminance = ([r, g, b]) => 0.2126 * r + 0.7152 * g + 0.0722 * b;
function contrast(a, b) {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}
// [foreground, background, minimum ratio, where it appears]
const grounds = ['canvas', 'surface', 'inset', 'data', 'elevated'];
const pairs = [
  ...grounds.flatMap((ground) => [
    ['fg', ground, 7, `primary text on ${ground}`],
    ['fg-muted', ground, 4.5, `secondary text on ${ground}`],
    ['fg-subtle', ground, 4.5, `metadata and placeholders on ${ground}`],
    ['accent', ground, 4.5, `links, selection and focus on ${ground}`],
    ['accent-strong', ground, 4.5, `link hover on ${ground}`],
    ['edge-strong', ground, 3, `interactive borders on ${ground} (1.4.11 non-text)`],
  ]),
  ['on-action', 'action', 4.5, 'primary button label'],
  ['on-action', 'action-hover', 4.5, 'primary button label on hover'],
  ['on-accent', 'accent', 4.5, 'label on an accent fill'],
  ['on-accent', 'accent-strong', 4.5, 'label on an accent fill on hover'],
  ['on-accent', 'err', 4.5, 'danger button label'],
  ['on-brand', 'brand', 7, 'brand band text'],
  ['brand-link', 'brand', 4.5, 'brand band links'],
  ...['ok', 'warn', 'err'].flatMap((state) => [
    [state, 'surface', 4.5, `${state} text on cards`],
    [state, 'canvas', 4.5, `${state} text on the page`],
    [state, `${state}-soft`, 4.5, `${state} text on its own tint (badges, alert titles)`],
    ['fg-muted', `${state}-soft`, 4.5, `alert body on the ${state} tint`],
  ]),
];
function contrastFailures() {
  const failures = [];
  for (const mode of ['light', 'dark']) {
    const t = colours(tokens.themes[mode]);
    for (const [f, b, floor, note] of pairs) {
      if (!t[f] || !t[b]) { failures.push(`[${mode}] missing opaque token: ${t[f] ? b : f}`); continue; }
      const ratio = contrast(t[f], t[b]);
      if (verbose) console.log(`${ratio >= floor ? 'pass' : 'FAIL'} [${mode}] ${ratio.toFixed(2).padStart(5)} (need ${floor}) ${f} on ${b}: ${note}`);
      if (ratio < floor) failures.push(`[${mode}] ${f} on ${b} is ${ratio.toFixed(2)}:1, below ${floor}:1 (${note})`);
    }
    // A primary action needs a visible boundary on every ground it sits on.
    for (const ground of grounds) {
      if (Math.max(contrast(t['on-action'], t[ground]), contrast(t.action, t[ground])) < 3) failures.push(`[${mode}] primary action has no 3:1 boundary on ${ground}`);
    }
    // Chart marks on the data surface. Light series 3 to 5 are relieved by
    // direct labels or a table (docs/design.md in the hub), dark has no relief.
    for (const n of mode === 'dark' ? [1, 2, 3, 4, 5, 6, 7, 8] : [1, 2, 6, 7, 8]) {
      if (contrast(t[`series-${n}`], t.data) < 3) failures.push(`[${mode}] chart series ${n} below 3:1 on data`);
    }
  }
  return failures;
}
const failures = contrastFailures();
if (failures.length) {
  for (const failure of failures) console.error(`Contrast: ${failure}`);
  console.error(`${failures.length} contrast obligation(s) not met; the release is not generated.`);
  process.exit(1);
}

// --- rendering --------------------------------------------------------------

const declaration = (values, indent = '\t') => Object.entries(values).map(([key, value]) => `${indent}--${key}: ${value};`).join('\n');
const shared = (profile) => ({ ...tokens.foundations, ...tokens.aliases, ...tokens.profiles[profile] });
const block = (selector, theme, profile, foundations = false) => `${selector} {\n\tcolor-scheme: ${theme};\n${declaration({ ...(foundations ? shared(profile) : {}), ...tokens.themes[theme] })}\n}`;
const indent = (text) => text.split('\n').map((line) => (line ? `\t${line}` : line)).join('\n');
/** Light by default, an explicit dark choice, and system following the OS. */
const application = (profile) => `:root {\n${declaration(shared(profile))}\n}\n\n${block(":root,\n[data-theme='light']", 'light', profile)}\n\n${block("[data-theme='dark']", 'dark', profile)}\n\n@media (prefers-color-scheme: dark) {\n${indent(block(":root[data-theme='system']", 'dark', profile))}\n}`;
function render(id) {
  const withChrome = id === 'hub' || id === 'edge';
  const header = `/* Website design tokens ${tokens.version}; source SHA-256 ${digest}${withChrome ? `;\n * chrome.css SHA-256 ${chromeDigest}` : ''}.\n * Generated by design-tokens/generate.mjs. Edit the website source, then sync. */`;
  let body;
  if (id === 'site-dark') body = block(':root', 'dark', 'site', true);
  else if (id === 'site-light') body = block("html:root[data-design='light']", 'light', 'site');
  else body = application(id);
  if (withChrome) body += `\n\n@layer components {\n${indent(chrome.trim())}\n}`;
  return `/* design-tokens:start ${id} */\n${header}\n${body}\n/* design-tokens:end ${id} */`;
}
let failed = false;
for (const [relative, id] of targets[consumer]) {
  const path = join(root, relative);
  const current = readFileSync(path, 'utf8');
  const start = `/* design-tokens:start ${id} */`;
  const end = `/* design-tokens:end ${id} */`;
  const from = current.indexOf(start), to = current.indexOf(end);
  if (from < 0 || to < from || current.indexOf(start, from + 1) !== -1) throw new Error(`Missing or duplicate token slot: ${relative}`);
  const expected = current.slice(0, from) + render(id) + current.slice(to + end.length);
  // Core colour/geometry/type declarations belong to the release, not hidden local overrides.
  const outside = current.slice(0, from) + current.slice(to + end.length);
  const protectedNames = new Set([...Object.keys(tokens.themes.light), ...Object.keys(tokens.foundations)]);
  for (const match of outside.matchAll(/--([\w-]+)\s*:\s*([^;]+);/g)) {
    if (protectedNames.has(match[1])) throw new Error(`Unowned token override --${match[1]} in ${relative}`);
  }
  if (current !== expected) {
    if (check) { console.error(`Token drift: ${relative}. Run node design-tokens/generate.mjs.`); failed = true; }
    else writeFileSync(path, expected);
  }
}
for (const flag of ['--compare', '--sync']) {
  const index = args.indexOf(flag);
  if (index === -1) continue;
  if (consumer !== 'site') throw new Error(`${flag} is only supported from the authoritative website checkout.`);
  const destinations = args.slice(index + 1).filter((arg) => !arg.startsWith('--'));
  if (!destinations.length) throw new Error(`${flag} requires repository paths.`);
  for (const destination of destinations) {
    const targetRoot = resolve(destination);
    const targetPackage = join(targetRoot, 'design-tokens');
    if (flag === '--sync') {
      mkdirSync(targetPackage, { recursive: true });
      for (const file of files) copyFileSync(join(packageDir, file), join(targetPackage, file));
      const result = spawnSync(process.execPath, [join(targetPackage, 'generate.mjs')], { stdio: 'inherit' });
      if (result.status !== 0) failed = true;
    } else {
      for (const file of files) {
        if (!readFileSync(join(packageDir, file)).equals(readFileSync(join(targetPackage, file)))) {
          console.error(`Release drift: ${targetRoot}/design-tokens/${file}`); failed = true;
        }
      }
      const result = spawnSync(process.execPath, [join(targetPackage, 'generate.mjs'), '--check'], { stdio: 'inherit' });
      if (result.status !== 0) failed = true;
    }
  }
}
if (failed) process.exitCode = 1;
else console.log(`${consumer}: design release ${tokens.version} ${check ? 'verified' : 'generated'}; contrast obligations met in both themes`);
