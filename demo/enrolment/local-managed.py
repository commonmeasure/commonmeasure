#!/usr/bin/env python3
"""Exercise a real local Hub and released edge with an isolated test database.

The database must already be migrated. Only synthetic rows are inserted;
no hosted authentication or agent-host integration is claimed. Source fetches
use Common Measure's public website. Raw evidence stays in the output directory.
"""
import argparse
import hashlib
import http.server
import json
import os
from pathlib import Path
import secrets
import shutil
import socket
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request
import urllib.parse
import uuid

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--database-url', required=True, help='An isolated, migrated local test database')
parser.add_argument('--hub-binary', required=True, type=Path, help='The real Hub server binary')
parser.add_argument('--edge', default='commonmeasure', help='The released edge executable')
parser.add_argument('--output', type=Path, help='A new directory for raw evidence; defaults to a temporary directory')
args = parser.parse_args()
DB = args.database_url
hub_binary = args.hub_binary.resolve(strict=True)
BINARY = shutil.which(args.edge)
if not BINARY:
    parser.error('the edge executable was not found')
if args.output:
    args.output.mkdir(mode=0o700, parents=True, exist_ok=False)
    OUT = args.output.resolve()
else:
    OUT = Path(tempfile.mkdtemp(prefix='cm-managed-acceptance-'))
scratch = Path(tempfile.mkdtemp(prefix='cm-pilot-', dir=OUT))
home = scratch / 'edge'
project = scratch / 'selected-project'
outside = scratch / 'private-project'
for path in (home, project, outside):
    path.mkdir(mode=0o700)
transcript = []

def run_process(argv, **options):
    """Subprocess exceptions must not print token or database arguments."""
    try:
        return subprocess.run(argv, **options)
    except (subprocess.TimeoutExpired, subprocess.CalledProcessError, OSError):
        raise RuntimeError('acceptance subprocess failed or timed out; arguments withheld') from None

def record(message):
    message = str(message).replace(str(scratch), '<test>')
    transcript.append(message)
    print(message, flush=True)

def free_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]

port = free_port()
origin = f'http://127.0.0.1:{port}'
token = secrets.token_urlsafe(32)
user_id, org_id = str(uuid.uuid4()), str(uuid.uuid4())
sql = f"""
INSERT INTO users(id,email) VALUES ('{user_id}','pilot-{user_id}@acceptance.example');
INSERT INTO organizations(id,name,slug) VALUES ('{org_id}','Pilot acceptance','pilot-{org_id}');
INSERT INTO memberships(user_id,organization_id,role) VALUES ('{user_id}','{org_id}','owner');
INSERT INTO sessions(id,user_id,organization_id,expires_at)
VALUES ('{hashlib.sha256(token.encode()).hexdigest()}','{user_id}','{org_id}',NOW()+INTERVAL '1 hour');
"""
run_process(['psql', DB, '-v', 'ON_ERROR_STOP=1', '-q', '-c', sql],
            check=True, capture_output=True, timeout=30)
environment = {**os.environ, 'DATABASE_URL': DB, 'PORT': str(port),
               'IDENTITY_ORIGIN': origin, 'API_KEY_ENCRYPTION_KEY': secrets.token_hex(32)}
backend_log = open(OUT / 'backend.log', 'w')
backend = subprocess.Popen([str(hub_binary)],
                           env=environment, stdout=backend_log, stderr=backend_log)
edge_env = {**os.environ, 'COMMONMEASURE_HOME': str(home)}

def api(path, body=None):
    request = urllib.request.Request(origin + '/api/v1/' + path,
        data=json.dumps(body).encode() if body is not None else None,
        headers={'Cookie': '__Host-session=' + token, 'Origin': origin,
                 'Content-Type': 'application/json'})
    with urllib.request.urlopen(request, timeout=15) as response:
        return json.load(response)

def command(*args, cwd=project, expected=0, show=True):
    result = run_process([BINARY, *args], env=edge_env, cwd=cwd,
                            text=True, capture_output=True, timeout=45)
    if show:
        record('$ commonmeasure ' + ' '.join(args))
        record(result.stdout.strip())
        if result.stderr.strip():
            record(result.stderr.strip())
    if expected is not None:
        assert result.returncode == expected, (args[0], result.returncode, result.stderr)
    return result

def status():
    return json.loads(command('status', '--json', show=False).stdout)

def fetch(url, session, refused=False, cwd=project):
    log_path = home / 'sessions' / f'{session}.ndjson'
    previous_count = len(log_path.read_text().splitlines()) if log_path.exists() else 0
    messages = [
        {'jsonrpc': '2.0', 'id': 1, 'method': 'initialize', 'params': {
            'protocolVersion': '2025-11-25', 'capabilities': {},
            'clientInfo': {'name': 'pilot-acceptance-driver', 'version': '1'}}},
        {'jsonrpc': '2.0', 'method': 'notifications/initialized'},
        {'jsonrpc': '2.0', 'id': 2, 'method': 'tools/call', 'params': {
            'name': 'context_fetch', 'arguments': {'url': url}}},
    ]
    result = run_process([BINARY, 'mcp', '--host', 'claude-code', '--session', session],
        env=edge_env, cwd=cwd, input='\n'.join(map(json.dumps, messages))+'\n',
        text=True, capture_output=True, timeout=60)
    assert result.returncode == 0, result.stderr
    replies = [json.loads(line) for line in result.stdout.splitlines() if line.strip()]
    reply = next(value for value in replies if value.get('id') == 2)
    assert 'error' not in reply and isinstance(reply.get('result'), dict), reply
    is_error = reply['result'].get('isError', False)
    assert is_error == refused, reply
    rows = [json.loads(line) for line in log_path.read_text().splitlines()[previous_count:]]
    event = 'crossing_refused' if refused else 'crossing_mediated'
    crossings = [row['payload'] for row in rows if row['event'] == event
                 and row['payload']['url'] == url]
    assert len(crossings) == 1, (event, crossings)
    crossing = crossings[0]
    # This record version has no refusal-stage code. Check the named policy
    # refusal so a transport, authentication or declaration error cannot pass.
    if refused:
        reason = f'The job denies host {urllib.parse.urlsplit(url).hostname}.'
        assert crossing['refusal'] == reason, crossing
        assert 'content_hash' not in crossing, crossing
    else:
        assert crossing['http_status'] == 200, crossing
        assert crossing['content_hash'].startswith('sha256:'), crossing
        assert crossing['retrieved_hash'].startswith('sha256:'), crossing
    record(f'MCP context_fetch {url}: ' + ('refused' if is_error else 'admitted') + f'; session {session}')
    record('Record: ' + json.dumps({key: crossing[key] for key in
           ('refusal', 'content_hash', 'retrieved_hash', 'http_status', 'policy_identity')
           if key in crossing}))
    return reply

try:
    for _ in range(100):
        try:
            urllib.request.urlopen(origin + '/health', timeout=1).close()
            break
        except (OSError, urllib.error.URLError):
            time.sleep(.1)
    else:
        raise RuntimeError('local hub did not start')
    record('Local real-hub managed-policy acceptance; synthetic owner session seeded in isolated Postgres.')
    record('This run does not establish hosted sign-in, installation, or a real agent-host interaction.')
    record(command('--version', show=False).stdout.strip())
    record('Edge executable SHA-256: ' + hashlib.sha256(Path(BINARY).read_bytes()).hexdigest())
    record('Hub executable SHA-256: ' + hashlib.sha256(hub_binary.read_bytes()).hexdigest())
    record('Hub origin: ' + origin)
    record('Organisation: ' + org_id)
    record('MCP client: pilot-acceptance-driver; host argument claude-code is a transport test label.')
    assert api('policy/revisions') == []
    assert api('enrolment/keys') == []
    policy = {'policy_mode': 'strict', 'constraints': [
        {'kind': 'denied_source_host', 'host': 'example.com'}],
        'scopes': [{'match': str(project), 'engagement': 'pilot', 'allow_telemetry_egress': True}]}
    policy_file = scratch / 'candidate.json'
    policy_file.write_text(json.dumps(policy))
    command('policy', 'check', str(policy_file))
    first = api('policy/revisions', {'policy': policy})
    record('Published revision before enrolment: ' + json.dumps(first))
    minted = api('enrolment/tokens', {'name': 'pilot-acceptance'})
    connected = command('connect', origin, '--token', minted['value'], '--managed', show=False)
    record('$ commonmeasure connect <hub> --token <redacted> --managed')
    record(connected.stdout.strip())
    current = status()
    assert current['applied']['revision'] == first['revision'], current
    first_envelope = (home / 'managed/last-known-good.json').read_bytes()
    record('Applied revision and digest: ' + json.dumps(current['applied']))
    fetch('https://commonmeasure.ai/', 'pilot-selected')
    fetch('https://example.com/', 'pilot-selected', refused=True)
    fetch('https://commonmeasure.ai/', 'pilot-private', cwd=outside)
    command('relay')
    summary = api('telemetry/summary')
    record('Hub summary: ' + json.dumps(summary))
    assert summary['content_retrieved'] == 1, summary
    assert summary['sessions'] == 1, summary
    before = summary.copy()
    command('relay')
    after = api('telemetry/summary')
    assert after['content_retrieved'] == before['content_retrieved'], after
    record('Repeated relay: no events newly spooled; retrieval count unchanged. This does not exercise receiver deduplication.')
    second_policy = json.loads(json.dumps(policy))
    second_policy['constraints'].append({'kind': 'denied_source_host', 'host': 'commonmeasure.ai'})
    second = api('policy/revisions', {'policy': second_policy})
    assert status()['applied']['revision'] == first['revision']
    record(f"Desired revision {second['revision']}; applied revision {first['revision']} before sync.")
    command('policy', 'sync')
    assert status()['applied']['revision'] == second['revision']
    fetch('https://commonmeasure.ai/', 'pilot-updated', refused=True)
    deployment_path = home / 'deployment.json'
    deployment_bytes = deployment_path.read_bytes()
    class Replay(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(first_envelope)
        def log_message(self, *_):
            pass
    replay = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Replay)
    threading.Thread(target=replay.serve_forever, daemon=True).start()
    deployment = json.loads(deployment_bytes)
    deployment['policy_url'] = f'http://127.0.0.1:{replay.server_port}/api/v1/policy/desired'
    deployment_path.write_text(json.dumps(deployment))
    rejected = command('policy', 'sync', expected=None)
    assert rejected.returncode != 0 and 'rollback' in rejected.stdout, rejected.stdout
    assert status()['applied']['revision'] == second['revision']
    record('Rollback replay: exact revision-1 envelope from the real hub, replayed through loopback HTTP; signer and organisation unchanged.')
    replay.shutdown()
    replay.server_close()
    unreachable = command('policy', 'sync', expected=None)
    assert unreachable.returncode != 0
    assert status()['applied']['revision'] == second['revision']
    fetch('https://commonmeasure.ai/', 'pilot-offline', refused=True)
    record('Unavailable management endpoint: last-known-good revision still enforces the source refusal.')
    deployment_path.write_bytes(deployment_bytes)
    command('policy', 'sync')
    command('disconnect')
    record('PASS: local hub rollout, clearance, delivery, repeated relay, rollback replay and unavailable-management enforcement.')
finally:
    backend.terminate()
    backend.wait(timeout=10)
    backend_log.close()
    (OUT / 'local-managed-run.txt').write_text('\n'.join(transcript)+'\n')
    (OUT / 'scratch-path.txt').write_text(str(scratch))
    print('Evidence directory:', OUT, flush=True)
