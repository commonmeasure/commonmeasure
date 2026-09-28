import { mount } from './policy-form.mjs';
async function response(response) {
  const value = await response.json();
  if (!response.ok) throw new Error(value.notice || value.error || `Request failed (${response.status}).`);
  return value;
}
const post = (path, payload) => fetch(path, {
  method: 'POST', headers: {'Content-Type': 'application/json'},
  body: JSON.stringify({revision: payload.revision, document: JSON.stringify(payload.document)})
}).then(response);
let editor;
document.addEventListener('click', async event => {
  if (!event.target.closest('[data-policy-edit]')) return;
  const root = document.getElementById('policy-editor');
  const read = document.getElementById('policy-read');
  const trigger = event.target.closest('[data-policy-edit]');
  const errorNotice = document.getElementById('policy-open-error');
  errorNotice.hidden = true;
  trigger.disabled = true;
  try {
    const schema = await fetch('/source-policy.schema.json').then(response);
    editor = await mount(root, {
      schema,
      adapter: {
        async load() {
          const p = await fetch('/api/policy', {cache: 'no-store'}).then(response);
          if (p.error || !p.declared) throw new Error(p.error || 'No policy is declared.');
          return {document: p.document, revision: p.revision, writable: !p.edit_error, reason: p.edit_error};
        },
        check: p => post('/api/policy/check', p),
        commit: p => post('/api/policy/save', p)
      },
      commitLabel: 'Save',
      consequence: 'This governs the next crossing and changes nothing already recorded. A running mediated session keeps the policy it loaded until it next starts.',
      onCancel() { editor.destroy(); read.hidden = false; trigger.disabled = false; trigger.focus(); },
      onCommitted() { location.reload(); }
    });
    read.hidden = true;
  } catch (error) {
    root.replaceChildren();
    read.hidden = false;
    trigger.disabled = false;
    errorNotice.querySelector('[data-policy-open-error-message]').textContent = error.message;
    errorNotice.hidden = false;
    errorNotice.focus();
  }
});
