// Common Measure shared policy form. Host effects are supplied by the adapter.
export const contract = 'commonmeasure-policy-form/v1';
const clone = value => structuredClone(value);
const equal = (a, b) => JSON.stringify(a) === JSON.stringify(b);
// JSON numbers beyond the browser's integer precision must never be rounded
// into a different policy during an unrelated edit.
export function assertSafeNumbers(value, path = '') {
  if (typeof value === 'number' && (!Number.isFinite(value) || (Number.isInteger(value) && !Number.isSafeInteger(value)))) {
    throw new Error(`Cannot edit ${path || '/'} in this browser: the number exceeds exact integer precision. Edit the policy file directly.`);
  }
  if (value && typeof value === 'object') {
    for (const [key, child] of Object.entries(value)) assertSafeNumbers(child, `${path}/${key}`);
  }
}
export function changes(before, after, path = '') {
  if (equal(before, after)) return [];
  if (before && after && !Array.isArray(before) && !Array.isArray(after)
      && typeof before === 'object' && typeof after === 'object') {
    return [...new Set([...Object.keys(before), ...Object.keys(after)])]
      .flatMap(key => changes(before[key], after[key], `${path}/${key}`));
  }
  return [{ path: path || '/', before, after }];
}
export function setMode(policy, index, mode) {
  const next = clone(policy);
  const target = index === null ? next : next.scopes[index];
  if (mode === '') delete target.policy_mode;
  else target.policy_mode = mode;
  return next;
}
export function denyHost(policy, index, host) {
  const next = clone(policy);
  const target = index === null ? next : next.scopes[index];
  target.constraints ??= clone(next.constraints ?? []);
  target.constraints.push({ kind: 'denied_source_host', host });
  return next;
}
function node(tag, text, attrs = {}) {
  const element = document.createElement(tag);
  if (text !== undefined) element.textContent = text;
  for (const [key, value] of Object.entries(attrs)) element.setAttribute(key, value);
  return element;
}
function button(text, action, primary = false) {
  const b = node('button', text, { type: 'button', class: primary ? 'pf-primary' : '' });
  b.addEventListener('click', action);
  return b;
}
function resolve(schema, field) {
  return field.$ref ? schema.$defs[field.$ref.split('/').at(-1)] : field;
}

/** Mount one editor. load -> {document, revision, writable, reason?};
 * check({document, revision}) validates without writing; commit uses the same
 * payload and performs authoritative validation and concurrency checks.
 * All adapter errors reject with a human-readable Error. No HTML is accepted.
 * Returns destroy(); the host owns read views and navigation after commit.
 */
export async function mount(root, { schema, adapter, commitLabel, consequence, onCancel, onCommitted }) {
  const snapshot = await adapter.load();
  assertSafeNumbers(snapshot.document);
  let draft = clone(snapshot.document), disposed = false, busy = false;
  const error = node('p', '', { role: 'alert', class: 'pf-error', tabindex: '-1' });
  const content = node('div');
  root.classList.add('policy-form');
  root.replaceChildren(error, content);
  if (!snapshot.writable) {
    error.textContent = snapshot.reason || 'Policy editing unavailable.';
    content.append(button('Cancel', onCancel));
    return { destroy() { root.replaceChildren(); } };
  }
  const payload = () => ({ document: clone(draft), revision: snapshot.revision });
  async function perform(action) {
    if (busy || disposed) return;
    busy = true;
    root.setAttribute('aria-busy', 'true');
    root.querySelectorAll('button, input, select, textarea').forEach(e => e.disabled = true);
    error.textContent = '';
    try { await action(); }
    catch (e) { if (!disposed) { error.textContent = e.message; error.focus(); } }
    finally {
      busy = false;
      root.removeAttribute('aria-busy');
      root.querySelectorAll('button, input, select, textarea').forEach(e => e.disabled = false);
    }
  }
  function heading(text) {
    const h = node('h2', text, { tabindex: '-1' });
    content.replaceChildren(h);
    h.focus();
  }
  function modeControl(container, index, label) {
    const target = index === null ? draft : draft.scopes[index];
    const field = index === null ? schema.properties.policy_mode : schema.$defs.PolicyScope.properties.policy_mode;
    // The schema owns the allowed mode vocabulary, including nullable overlays.
    const def = resolve(schema, field);
    const modeDef = def.anyOf ? resolve(schema, def.anyOf.find(x => x.$ref)) : def;
    const select = node('select', undefined, { 'aria-label': label });
    const modes = modeDef.oneOf.map(x => x.const);
    if (index !== null) select.append(node('option', `Inherit default (${draft.policy_mode ?? 'observe'})`, { value: '' }));
    for (const mode of modes) select.append(node('option', mode, { value: mode }));
    select.value = target.policy_mode ?? (index === null ? schema.properties.policy_mode.default : '');
    select.addEventListener('change', () => {
      draft = setMode(draft, index, select.value);
      const json = root.querySelector('textarea');
      if (json) json.value = JSON.stringify(draft, null, 2);
      for (const inherited of root.querySelectorAll('option[value=""]')) {
        inherited.textContent = `Inherit default (${draft.policy_mode ?? 'observe'})`;
      }
    });
    const wrap = node('label', label); wrap.append(select); container.append(wrap);
  }
  function scopeEditor(index) {
    const target = index === null ? draft : draft.scopes[index];
    const section = node('fieldset');
    section.append(node('legend', index === null ? 'Default policy' : target.match));
    modeControl(section, index, index === null ? 'Default mode' : `Mode for ${target.match}`);
    if (index !== null) section.append(node('p', 'An inherited mode follows the default. Choosing a mode gives this directory its own value.'));
    const host = node('input', undefined, { type: 'text', placeholder: 'tracker.example', 'aria-label': 'Host to refuse' });
    const label = node('label', 'Host to refuse'); label.append(host); section.append(label);
    section.append(button('Add host', () => {
      if (!host.value.trim()) { host.focus(); return; }
      draft = denyHost(draft, index, host.value.trim()); renderEdit();
    }));
    if (index !== null && target.constraints == null) section.append(node('p', 'Adding a host copies the default rules here; later default rule changes will not reach this directory.'));
    const hosts = (target.constraints ?? draft.constraints ?? []).filter(c => c.kind === 'denied_source_host');
    if (hosts.length) section.append(node('p', `Denied hosts: ${hosts.map(c => c.host).join(', ')}`));
    return section;
  }
  function renderEdit() {
    if (disposed) return;
    heading('Edit policy');
    content.append(node('p', 'Strict refuses a source that breaks a rule. Observe and Prefer record the breach.'));
    content.append(scopeEditor(null));
    for (let i = 0; i < (draft.scopes ?? []).length; i++) {
      const directory = node('details');
      directory.append(node('summary', draft.scopes[i].match), scopeEditor(i));
      content.append(directory);
    }
    const advanced = node('details'); advanced.append(node('summary', 'Full policy JSON'));
    const label = node('label', 'Policy JSON');
    const json = node('textarea', undefined, { rows: '16', spellcheck: 'false' });
    let jsonChanged = false;
    json.value = JSON.stringify(draft, null, 2);
    json.addEventListener('input', () => {
      jsonChanged = true;
      // One draft at a time: validate JSON before returning to simple controls.
      content.querySelectorAll('fieldset').forEach(field => { field.disabled = true; });
    });
    label.append(json); advanced.append(label);
    advanced.append(node('p', 'Scopes and source rules keep their order; the first match wins. A scope’s own rule list replaces the default list. Other fields are retained.'));
    advanced.append(node('p', 'Reporting permission can allow records to leave this Edge, subject to consent and approvals. Internal recording prefixes can admit private content to the local record. Review these fields before saving.'));
    advanced.append(node('p', 'Use JSON draft to validate these changes and return to the simple controls.'));
    advanced.append(button('Use JSON draft', () => {
      try {
        const parsed = JSON.parse(json.value);
        assertSafeNumbers(parsed);
        if (!parsed || Array.isArray(parsed) || typeof parsed !== 'object') throw new Error('Policy must be an object.');
        // Runtime validation comes before rendering an arbitrary JSON draft.
        perform(async () => { await adapter.check({document: parsed, revision: snapshot.revision}); draft = parsed; renderEdit(); });
      } catch (e) { error.textContent = e.message; error.focus(); }
    }));
    content.append(advanced);
    const actions = node('div', undefined, { class: 'pf-actions' });
    actions.append(button('Cancel', onCancel), button('Review changes', () => perform(async () => {
      if (jsonChanged) {
        const parsed = JSON.parse(json.value);
        assertSafeNumbers(parsed);
        await adapter.check({document: parsed, revision: snapshot.revision});
        draft = parsed;
      } else await adapter.check(payload());
      renderReview();
    }), true));
    content.append(actions);
  }
  function renderReview() {
    if (disposed) return;
    heading('Review changes');
    const diff = changes(snapshot.document, draft);
    if (!diff.length) content.append(node('p', 'No changes to save.'));
    for (const change of diff) {
      const group = node('section', undefined, { class: 'pf-change' });
      const labels = {policy_mode:'Default mode', constraints:'Source rules', scopes:'Directories', principals:'Principals', allow_private_hosts:'Private hosts', record_internal_prefixes:'Internal recording prefixes', terms:'Terms'};
      group.append(node('h3', labels[change.path.slice(1)] ?? 'Policy change'));
      for (const [label, value] of [['Before', change.before], ['After', change.after]]) {
        group.append(node('h4', label), node('pre', value === undefined ? 'Inherited / absent' : typeof value === 'string' ? value : JSON.stringify(value, null, 2)));
      }
      content.append(group);
    }
    content.append(node('p', consequence));
    const actions = node('div', undefined, { class: 'pf-actions' });
    actions.append(button('Back to edit', renderEdit));
    if (diff.length) actions.append(button(commitLabel, () => perform(async () => {
      const result = await adapter.commit(payload());
      if (!disposed) onCommitted(result);
    }), true));
    content.append(actions);
  }
  renderEdit();
  return { destroy() { disposed = true; root.replaceChildren(); } };
}
