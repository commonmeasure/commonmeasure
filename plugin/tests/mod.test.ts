// Tests for the mod's display hooks (`hooks/register.js`), run by `claude
// plugin test plugin`: the facts under a mediated call's row, the sources
// line under an answer, `/cm keep` and `/cm session`, and the strict-mode
// question. The stubs stand in for Claude Code, the edge's MCP server and the
// `commonmeasure` binary; the binary's JSON is the shape `commonmeasure
// session --json` prints (`crates/commonmeasure-cli/tests/session_join.rs`
// holds that side). No model, network or process runs.
//
// What each test catches: facts drawn from anything but the edge's own lines,
// a refusal with no toast, a row drawn before the call answered, a sources
// line whose counts are not the session command's, an observed count left
// out, a line under a turn that crossed nothing, a keep that loses a log or
// skips the evidence, a strict-mode WebFetch made natively without the
// person's say or on a dismissed question, and a WebFetch made natively
// when the source policy could not be read.
import { expect, test } from 'claude-code/testing'

const EDGE = 'plugin_commonmeasure_commonmeasure'
const TOOLS = [
  { name: 'WebFetch', description: 'fetch', mcp: false },
  { name: `mcp__${EDGE}__context_fetch`, description: 'Fetch a URL through Common Measure.', mcp: true },
  { name: `mcp__${EDGE}__context_search`, description: 'Search.', mcp: true },
]

const DELIVERED =
  'Common Measure · host example.org · terms RSL: ai-input, report · ruling delivered (strict) · cost unknown · grade mediated · receipt owed · sha256:3f9a5b6c…'
const REFUSED =
  'Common Measure · host blocked.example · terms Content-Signal: no ai-input · ruling refused (strict) · cost unknown · grade mediated · receipt none · sha256 unknown'

function edgeAnswer(lines: string[], value: unknown, isError = false) {
  const content = [{ type: 'text', text: JSON.stringify(value) }]
  if (lines.length > 0) content.unshift({ type: 'text', text: lines.join('\n') })
  return { value: { content, isError } }
}

function fetched() {
  return {
    url: 'https://example.org/page', http_status: 200, policy: 'Admitted.', breach: null,
    recorded_in: '/home/op/.commonmeasure/sessions/local-1.ndjson', content: 'Page text',
  }
}

function ran(stdout: string, exitCode = 0, stderr = '') {
  return { value: { exitCode, stdout, stderr, isStdoutTruncated: false, isStderrTruncated: false } }
}

// What `commonmeasure session --json` prints for a turn with two carried
// crossings and one refusal.
const SESSION = {
  session: 'host-session',
  path: '/home/op/.commonmeasure/sessions/host-session.ndjson',
  joined: ['/home/op/.commonmeasure/sessions/local-1.ndjson'],
  since: null,
  records: 6,
  summary: { observed: 1, mediated: 1, refused: 1, reconstructed: 0, grounded_witnessed: 2 },
  sources: {
    crossings: [
      { grade: 'observed', url: 'https://other.example/a', host: 'other.example', terms: 'unknown', ruling: 'not ruled', cost: 'unknown', receipt: 'unknown', grounded: true, hash: 'sha256 unknown' },
      { grade: 'mediated', url: 'https://example.org/page', host: 'example.org', terms: 'RSL: ai-input, report', ruling: 'delivered', cost: 'unknown', receipt: 'owed', grounded: true, hash: 'sha256:3f9a5b6c…' },
      { grade: 'refused', url: 'https://blocked.example/x', host: 'withheld', terms: 'unknown', ruling: 'refused', cost: 'unknown', receipt: 'none', grounded: false, hash: 'sha256 unknown' },
    ],
    cost: 'unknown',
    receipts_owed: 1,
  },
}

const QUIET = { ...SESSION, summary: { observed: 0, mediated: 0, refused: 0, reconstructed: 0 }, sources: { crossings: [], cost: 'none', receipts_owed: 0 } }

// The engine's own stubs every test needs: a drawing surface, a session id
// and directory, and quiet display calls.
function engine(on, { toasts = [] as string[], logs = [] as string[], surfaces = ['terminal'] } = {}) {
  on('session.surfaces', () => ({ value: surfaces }))
  on('session.id', () => ({ value: 'host-session' }))
  on('session.cwd', () => ({ value: '/work' }))
  on('session.start', ($, e) => ({ cwd: e.cwd }))
  on('command.register', ($, e) => ({ value: { command: e.name } }))
  on('turn.start', () => ({ turnId: 'turn-1' }))
  on('ui.toast', ($, e) => {
    toasts.push(e.text)
    return { value: undefined }
  })
  on('ui.log', ($, e) => {
    logs.push(e.text)
    return { value: undefined }
  })
}

const ROW = { tool: 'WebFetch', input: {}, isRunning: false, isErrored: false, isInterrupted: false, output: {} }

async function drawn($, id: string, props = {}) {
  const tree = await $.ui.render({ component: 'ToolUse', surface: 'terminal', requestId: id, props: { ...ROW, tool_use_id: id, ...props } })
  return JSON.stringify(tree)
}

function engineRow(on) {
  on('ui.render', ($, e) => {
    const { Text } = $.ui.resolve(e)
    return h(Text, null, 'engine row')
  })
}

test('a routed WebFetch row shows the edge facts once the call has answered', async ($, on) => {
  engine(on)
  engineRow(on)
  on('tool.list', () => ({ value: TOOLS }))
  on('mcp.call', () => edgeAnswer([DELIVERED], fetched()))
  on('model.complete', () => ({ value: { isAnswered: true, text: 'ok', usage: null } }))

  await $.tool.call({ tool: 'WebFetch', url: 'https://example.org/page', prompt: 'p', tool_use_id: 'row-1' })

  const done = await drawn($, 'row-1')
  expect(done).toContain('engine row')
  expect(done).toContain('host example.org · terms RSL: ai-input, report · ruling delivered (strict)')
  expect(done).not.toContain('Common Measure · host')
  expect(await drawn($, 'row-1', { isRunning: true })).not.toContain('ruling delivered')
  expect(await drawn($, 'row-unknown')).not.toContain('ruling')
})

test('a refused WebFetch draws its facts and raises one toast', async ($, on) => {
  const toasts: string[] = []
  engine(on, { toasts })
  engineRow(on)
  on('tool.list', () => ({ value: TOOLS }))
  on('mcp.call', () => edgeAnswer([REFUSED], { error: 'refused: the source declares no AI input.' }, true))

  const out = await $.tool.call({ tool: 'WebFetch', url: 'https://blocked.example/x', prompt: 'p', tool_use_id: 'row-2' })

  expect(out.deny ?? out.text).toContain('refused: the source declares no AI input.')
  expect(toasts).toEqual([`Refused by source policy: ${REFUSED.slice('Common Measure · '.length)}`])
  expect(await drawn($, 'row-2', { isErrored: true })).toContain('ruling refused (strict)')
})

test("the model's own context_fetch call keeps its lines for its row", async ($, on) => {
  engine(on)
  engineRow(on)
  on('tool.call', () => ({ result: [], text: `${DELIVERED}\n{"url":"https://example.org/page"}` }))

  await $.tool.call({ tool: `mcp__${EDGE}__context_fetch`, url: 'https://example.org/page', tool_use_id: 'row-3' })

  expect(await drawn($, 'row-3', { tool: `mcp__${EDGE}__context_fetch` })).toContain('receipt owed')
})

test('where nothing draws, each fact is a log line instead', async ($, on) => {
  const logs: string[] = []
  engine(on, { logs, surfaces: [] })
  on('tool.list', () => ({ value: TOOLS }))
  on('mcp.call', () => edgeAnswer([DELIVERED], fetched()))
  on('model.complete', () => ({ value: { isAnswered: true, text: 'ok', usage: null } }))

  await $.tool.call({ tool: 'WebFetch', url: 'https://example.org/page', prompt: 'p', tool_use_id: 'row-4' })

  expect(logs).toEqual([DELIVERED])
})

const TURN = { durationMs: 10, isAborted: false, turnId: 'turn-1', reason: 'answer' as const }

test("the sources line states the session command's counts for the turn", async ($, on) => {
  engine(on)
  const argv: string[][] = []
  on('process.run', ($, e) => {
    argv.push([...e.argv])
    return ran(JSON.stringify(SESSION))
  })
  on('turn.complete', ($, e) => ({ text: e.answer }))

  const before = new Date().toISOString()
  await $.turn.start({ text: 'what does it say?' })
  const out = await $.turn.complete({ ...TURN, answer: 'It says so (https://example.org/page).' })

  expect(out.text).toBe('Common Measure · 2 sources · 1 mediated · 1 observed · 1 refused · cost unknown · 1 receipt owed · 1 cited')
  expect(argv).toHaveLength(1)
  expect(argv[0][0]).toMatch(/\/bin\/commonmeasure-launch$/)
  expect(argv[0].slice(1, 4)).toEqual(['session', 'host-session', '--json'])
  expect(argv[0][4]).toBe('--since')
  expect(argv[0][5] >= before).toBe(true)
})

test('a turn that crossed nothing gets no line, and the observed count stays when it is zero', async ($, on) => {
  engine(on)
  let document: unknown = QUIET
  on('process.run', () => ran(JSON.stringify(document)))
  on('turn.complete', ($, e) => ({ text: e.answer }))

  await $.turn.start({ text: 'hello' })
  expect((await $.turn.complete({ ...TURN, answer: 'Hello.' })).text).toBe('Hello.')

  document = { ...QUIET, summary: { observed: 0, mediated: 2, refused: 0, reconstructed: 0 }, sources: { crossings: [], cost: '0.03 USD quoted', receipts_owed: 0 } }
  await $.turn.start({ text: 'again' })
  expect((await $.turn.complete({ ...TURN, answer: 'Done.' })).text).toBe(
    'Common Measure · 2 sources · 2 mediated · 0 observed · cost 0.03 USD quoted',
  )
})

test("a subagent's turn and an unreadable record leave the answer alone", async ($, on) => {
  const logs: string[] = []
  engine(on, { logs })
  on('process.run', () => ran('', 1, 'commonmeasure: no session has recorded anything yet'))
  on('turn.complete', ($, e) => ({ text: e.answer }))

  await $.turn.start({ text: 'q' })
  expect((await $.turn.complete({ ...TURN, answer: 'A.', agentId: 'agent-1' })).text).toBe('A.')
  expect((await $.turn.complete({ ...TURN, answer: 'B.' })).text).toBe('B.')
  expect(logs).toEqual(['Common Measure · sources unknown: no session has recorded anything yet'])
})

test('/cm keep binds the last answer to every log of the session and exports beside it', async ($, on) => {
  engine(on)
  const argv: string[][] = []
  const written: Array<{ path: string; text: string }> = []
  on('fs.write', ($, e) => {
    written.push({ path: e.path, text: e.text })
    return { value: undefined }
  })
  on('process.run', ($, e) => {
    const args = e.argv.slice(1)
    argv.push(args)
    if (args[0] === 'session') return ran(JSON.stringify(SESSION))
    if (args[1] === 'init') return ran('{"artifact_id":"urn:uuid:a"}')
    if (args[1] === 'associate') return ran(`{"record_id":"assoc-${argv.length}"}`)
    if (args[1] === 'snapshot') return ran('{"snapshot_id":"5e0b1c2d-0000-4000-8000-000000000000","snapshot_digest":"sha256:ff"}')
    if (args[1] === 'export') return ran(JSON.stringify({ output: args[7], snapshot_id: args[5], snapshot_digest: 'sha256:ff' }))
    return ran('', 2, 'unexpected')
  })
  on('turn.complete', ($, e) => ({ text: e.answer }))
  await $.session.start({ cwd: '/work', surface: 'terminal', isInteractive: true })
  await $.turn.complete({ ...TURN, answer: 'The answer.' })

  const out = await $.command.run({ command: 'cm', args: 'keep' })

  expect(written).toHaveLength(1)
  expect(written[0].path).toMatch(/^\/work\/\.commonmeasure\/kept\/answer-.*\.md$/)
  expect(written[0].text).toBe('The answer.')
  const file = written[0].path
  const name = file.split('/').pop()
  const store = `/work/.commonmeasure/artifacts/${name}`
  expect(argv.map((args) => args.slice(0, 2).join(' '))).toEqual([
    'session host-session', 'artifact init', 'artifact associate', 'artifact associate', 'artifact snapshot', 'artifact export',
  ])
  expect(argv[2]).toContain('host-session')
  expect(argv[3]).toContain('local-1')
  expect(argv[4]).toEqual([
    'artifact', 'snapshot', 'file', file, '--store', store,
    '--evidence', `assoc-3=${SESSION.path}`, '--evidence', `assoc-4=${SESSION.joined[0]}`,
  ])
  expect(argv[5].slice(-2)).toEqual(['--output', `${file}.commonmeasure.json`])
  expect(out.text).toContain(`Bundle: ${file}.commonmeasure.json`)
  expect(out.text).toContain('- mediated · host example.org · terms RSL: ai-input, report · ruling delivered · cost unknown · grounded · receipt owed')
  expect(out.text).toContain(`commonmeasure artifact verify ${file}.commonmeasure.json --file ${file} --expected-snapshot sha256:ff`)
  expect(out.text).toContain('Unsigned')
  expect(out.text).not.toContain('https://')
})

test('/cm keep with nothing to keep says so, and /cm session echoes hosts but no URL', async ($, on) => {
  engine(on)
  on('process.run', () => ran(JSON.stringify(SESSION)))
  await $.session.start({ cwd: '/work', surface: 'terminal', isInteractive: true })

  expect((await $.command.run({ command: 'cm', args: 'keep' })).text).toBe(
    'Nothing to keep: no answer yet in this session. Name a file: /cm keep <file>.',
  )
  const summary = (await $.command.run({ command: 'cm', args: 'session' })).text
  expect(summary).toContain('Common Measure · 2 sources · 1 mediated · 1 observed · 1 refused')
  expect(summary).toContain('- refused · host withheld')
  expect(summary).not.toContain('https://')
})

// Strict mode, edge not connected.
// What `commonmeasure policy identity` prints for a policy in `mode`.
const identity = (mode: string) => ran(`{"mode":"${mode}"}\nsha256:00\n`)

function strict(on, answer: (question: string) => unknown, policy: unknown = identity('strict')) {
  const reached: string[] = []
  engine(on)
  on('tool.list', () => ({ value: [{ name: 'WebFetch', description: 'fetch', mcp: false }] }))
  on('process.run', () => policy)
  on('tool.call', ($, e) => {
    if (e.tool === 'AskUserQuestion') return answer(e.questions[0].question)
    reached.push('native')
    return { result: 'native tool ran' }
  })
  return reached
}

const chose = (label: string) => (question: string) => ({ result: { questions: [], answers: { [question]: label } } })

test('strict mode asks first, and a refusal or a dismissal fetches nothing', async ($, on) => {
  const questions: string[] = []
  let answer = chose('Refuse')
  const reached = strict(on, (question) => {
    questions.push(question)
    return answer(question)
  })

  const refused = await $.tool.call({ tool: 'WebFetch', url: 'https://example.org/page', prompt: 'p', tool_use_id: 'ask-1' })
  answer = () => ({ deny: 'dismissed' })
  const dismissed = await $.tool.call({ tool: 'WebFetch', url: 'https://example.org/page', prompt: 'p', tool_use_id: 'ask-2' })

  expect(reached).toEqual([])
  expect(questions[0]).toBe('Common Measure is not connected and the source policy is strict. How should this WebFetch of example.org go ahead?')
  for (const out of [refused, dismissed]) expect(out.deny ?? out.text).toContain('the user refused this WebFetch')
})

test('strict mode: allow once runs the native tool; route connects the edge and fetches through it', async ($, on) => {
  let answer = chose('Allow once, observed')
  const reached = strict(on, (question) => answer(question))
  const servers: string[] = []
  on('mcp.connect', ($, e) => {
    servers.push(`connect ${e.server}`)
    return { value: { isConnected: true, server: 'plugin:commonmeasure:commonmeasure' } }
  })
  on('mcp.call', ($, e) => {
    servers.push(`call ${e.server}`)
    return edgeAnswer([DELIVERED], fetched())
  })
  on('model.complete', () => ({ value: { isAnswered: true, text: 'ok', usage: null } }))

  const allowed = await $.tool.call({ tool: 'WebFetch', url: 'https://example.org/page', prompt: 'p', tool_use_id: 'ask-3' })
  answer = chose('Route through Common Measure')
  const routed = await $.tool.call({ tool: 'WebFetch', url: 'https://example.org/page', prompt: 'p', tool_use_id: 'ask-4' })

  expect(allowed).toEqual({ result: 'native tool ran' })
  expect(reached).toEqual(['native'])
  expect(servers).toEqual(['connect commonmeasure', 'call plugin:commonmeasure:commonmeasure'])
  expect(routed.result.result).toBe('ok')
})

test('prefer mode never asks', async ($, on) => {
  const questions: string[] = []
  const reached = strict(
    on,
    (question) => {
      questions.push(question)
      return chose('Refuse')(question)
    },
    identity('prefer'),
  )

  await $.tool.call({ tool: 'WebFetch', url: 'https://example.org/page', prompt: 'p', tool_use_id: 'ask-5' })

  expect(questions).toEqual([])
  expect(reached).toEqual(['native'])
})

test('observe mode never asks', async ($, on) => {
  const questions: string[] = []
  const reached = strict(
    on,
    (question) => {
      questions.push(question)
      return chose('Refuse')(question)
    },
    identity('observe'),
  )

  const out = await $.tool.call({ tool: 'WebFetch', url: 'https://example.org/page', prompt: 'p', tool_use_id: 'ask-6' })

  expect(questions).toEqual([])
  expect(reached).toEqual(['native'])
  expect(out).toEqual({ result: 'native tool ran' })
})

// A policy that cannot be read may be strict: the WebFetch is refused with
// the gap named, and neither the native tool nor the question is reached. A
// stub's `{ deny }` makes `$.process.run` reject, as it does when the
// launcher cannot start or overruns its timeout.
const UNREADABLE = [
  { read: 'permission', policy: { deny: 'EACCES: permission denied' }, gap: 'the commonmeasure binary did not run' },
  { read: 'timeout', policy: { deny: 'timed out after 20000 ms' }, gap: 'the commonmeasure binary did not run' },
  { read: 'non-zero', policy: ran('', 1, 'commonmeasure: the policy file is unreadable'), gap: 'the policy file is unreadable' },
  { read: 'malformed', policy: ran('not JSON\n'), gap: 'the policy identity is not JSON' },
  { read: 'unknown mode', policy: identity('lenient'), gap: 'the policy identity names no known mode' },
]

for (const { read, policy, gap } of UNREADABLE) {
  test(`policy read failing (${read}): the WebFetch is refused and nothing is asked`, async ($, on) => {
    const questions: string[] = []
    const reached = strict(
      on,
      (question) => {
        questions.push(question)
        return chose('Allow once, observed')(question)
      },
      policy,
    )

    const out = await $.tool.call({ tool: 'WebFetch', url: 'https://example.org/page', prompt: 'p', tool_use_id: `unreadable-${read}` })

    expect(reached).toEqual([])
    expect(questions).toEqual([])
    expect(out.result).toBeUndefined()
    expect(out.deny).toContain('the source policy could not be read')
    expect(out.deny).toContain(gap)
  })
}
