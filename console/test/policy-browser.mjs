// Run after cargo build -p commonmeasure-console --example design_preview.
// PLAYWRIGHT_MODULE names an installed playwright ES entry; no downloads run.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href);
const evidence = resolve('target/policy-ux-evidence');
mkdirSync(evidence, { recursive: true });
const browser = await chromium.launch({headless:true});
const findings = [];
let child;
const port = 4198;
const base = `http://127.0.0.1:${port}`;
async function start(variant) {
  child = spawn(resolve('target/debug/examples/design_preview'), [], {env:{...process.env, CM_DESIGN_PREVIEW_PORT:String(port), CM_DESIGN_PREVIEW_POLICY:variant}, stdio:['ignore','pipe','pipe']});
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('Preview did not start')), 15000);
    child.once('exit', code => {clearTimeout(timer); reject(new Error(`Preview exited ${code}`));});
    child.stdout.on('data', bytes => { if (bytes.toString().includes('http://')) { clearTimeout(timer); resolve(); } });
  });
}
async function stop() { const exited = once(child, 'exit'); child.kill(); await exited; child = null; }
async function api(page, path, payload) {
  return page.evaluate(async ({path, payload}) => {
    const r = await fetch(path, payload ? {method:'POST', headers:{'Content-Type':'application/json'}, body:JSON.stringify(payload)} : undefined);
    return {status:r.status, body:await r.json()};
  }, {path, payload});
}
async function capture(page, name) {
  const overflow = await page.evaluate(() => [...document.querySelectorAll('main *')].filter(e => e.getBoundingClientRect().right > innerWidth + 1).map(e => [e.tagName,e.className,e.getBoundingClientRect().width,e.textContent.slice(0,60)]));
  if (await page.evaluate(() => document.documentElement.scrollWidth > innerWidth)) console.log(name, overflow);
  await page.screenshot({path:resolve(evidence, `${name}.png`), fullPage:true});
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false, `${name}: horizontal overflow`);
}
try {
  for (const variant of ['many', 'empty', 'one', 'managed', 'absent', 'malformed', 'deployment-error']) {
    await start(variant);
    const page = await browser.newPage();
    const errors = [];
    await page.addInitScript(() => { window.policyCspViolations = []; document.addEventListener('securitypolicyviolation', e => window.policyCspViolations.push(e.violatedDirective)); });
    page.on('pageerror', e => errors.push(e.message));
    await page.goto(`${base}/app/policy`);
    const initial = (await api(page, '/api/policy')).body;
    for (const theme of ['light','dark']) {
      await page.evaluate(theme => {document.documentElement.dataset.theme=theme;localStorage.setItem('commonmeasure-theme',theme);}, theme);
      for (const width of [390,1280]) {
        await page.setViewportSize({width,height:900});
        await capture(page, `${variant}-${theme}-${width}`);
      }
    }
    if (variant === 'many') {
      await page.getByRole('button', {name:'Edit policy',exact:true}).focus();
      await page.keyboard.press('Enter');
      await page.getByLabel('Default mode', {exact:true}).waitFor();
      assert.equal(await page.evaluate(() => document.activeElement.textContent), 'Edit policy');
      await page.getByLabel('Default mode', {exact:true}).selectOption('strict');
      assert.equal(await page.getByLabel('Mode for /synthetic/design-preview', {exact:true}).locator('option').first().textContent(), 'Inherit default (strict)');
      for (const theme of ['light','dark']) {
        await page.evaluate(theme => document.documentElement.dataset.theme=theme, theme);
        for (const width of [390,1280]) { await page.setViewportSize({width,height:900}); await capture(page, `edit-${theme}-${width}`); }
      }
      await page.getByRole('button', {name:'Review changes',exact:true}).click();
      await page.getByRole('button', {name:'Save',exact:true}).waitFor();
      for (const theme of ['light','dark']) {
        await page.evaluate(theme => document.documentElement.dataset.theme=theme, theme);
        for (const width of [390,1280]) { await page.setViewportSize({width,height:900}); await capture(page, `review-${theme}-${width}`); }
      }
      await page.getByRole('button', {name:'Save',exact:true}).click();
      await page.waitForSelector('[data-policy-edit]');
      const saved = (await api(page, '/api/policy')).body;
      assert.equal(saved.document.policy_mode, 'strict');
      assert.deepEqual(saved.document.scopes, initial.document.scopes);
      assert.equal(saved.document.scopes[1].policy_mode, undefined);
      assert.equal((await api(page, '/api/policy/save', {revision:initial.revision, document:JSON.stringify(initial.document)})).status, 409);
      await page.getByRole('button', {name:'Edit policy',exact:true}).click();
      await page.getByText('Full policy JSON', {exact:true}).click();
      await page.getByLabel('Policy JSON', {exact:true}).fill('{');
      assert.equal(await page.getByLabel('Default mode', {exact:true}).isDisabled(), true);
      await page.getByRole('button', {name:'Review changes',exact:true}).click();
      await page.locator('.pf-error').filter({hasText:/JSON|property|Expected/}).waitFor();
      assert.equal(await page.evaluate(() => document.activeElement.className), 'pf-error');
      assert.equal((await api(page, '/api/policy')).body.revision, saved.revision);
      await capture(page, 'invalid-draft');
      await page.getByRole('button', {name:'Cancel',exact:true}).click();
      await page.getByRole('button', {name:'Edit policy',exact:true}).click();
      await page.getByLabel('Default mode', {exact:true}).selectOption('prefer');
      await page.getByRole('button', {name:'Review changes',exact:true}).click();
      await page.getByRole('button', {name:'Save',exact:true}).waitFor();
      await api(page, '/api/policy/mode', {revision:saved.revision, mode:'observe'});
      await page.getByRole('button', {name:'Save',exact:true}).click();
      await page.locator('.pf-error').filter({hasText:/revision/}).waitFor();
      await capture(page, 'stale-draft');
      await page.getByRole('button', {name:'Back to edit',exact:true}).click();
      assert.equal(await page.getByLabel('Default mode', {exact:true}).inputValue(), 'prefer');
      await page.getByRole('button', {name:'Cancel',exact:true}).click();
      await page.getByText('Historical forecast', {exact:true}).focus();
      await page.keyboard.press('Enter');
      await page.getByRole('button', {name:'Forecast mode',exact:true}).click();
      await page.locator('#forecast-mode').filter({hasText:/crossings|Nothing|recorded/}).waitFor();
      await capture(page, 'forecast');
      await page.evaluate(() => document.documentElement.style.fontSize='32px');
      await page.setViewportSize({width:390,height:900});
      await capture(page, 'magnified-390');
      findings.push('Real Edge adapter: edit/review/save/reload, inherited mode, scope preservation, stale save, invalid JSON, retained draft, forecast and 200% root type size passed.');
    } else if (['managed', 'deployment-error'].includes(variant)) {
      assert.equal(await page.locator('[data-policy-edit]').count(), 0);
      for (const path of ['/api/policy/save','/api/policy/mode','/api/policy/deny']) {
        const result = await api(page, path, {revision:initial.revision, document:'{}', mode:'strict', host:'example.test'});
        assert.equal(result.status,403,path);
      }
      assert.equal((await api(page, '/api/policy')).body.revision, initial.revision);
      findings.push(`${variant}: read-only and every policy mutation endpoint rejects writes.`);
    } else if (['absent','malformed'].includes(variant)) {
      assert.equal(await page.locator('[data-policy-edit]').count(),0);
      const rejected = await api(page, '/api/policy/save', {revision:initial.revision ?? '', document:'{}'});
      assert.ok(rejected.status >= 400);
    }
    assert.deepEqual(errors, [], variant);
    assert.deepEqual(await page.evaluate(() => window.policyCspViolations), [], `${variant}: CSP violations`);
    await page.close(); await stop();
  }
  writeFileSync(resolve(evidence,'results.json'), JSON.stringify(findings,null,2)+'\n');
  console.log(findings.join('\n'));
} finally { if(child) await stop(); await browser.close(); }
