import test from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import {existsSync, readFileSync} from 'node:fs';
import {Blocked} from '../../runner.mjs';
import {documentationCase, validateDocumentationBinding, markdownOutline, canonicalMarkdown,
  assertDocumentationDownload, assertDocumentationStructure, assertDocumentationOpenApi,
  guardDocumentationReads, readDocumentation} from './documentation.mjs';

const ctx = {assert: (value, code) => assert.ok(value, code), observe: async (_label, read) => read()};
const binding = {publicBase: 'http://127.0.0.1:4321/event-gateway', advertiseCanonical: false};
const sources = {
  integration: '# Agent Event Gateway integration guide\n\n## Implement a downstream agent provider\n\nA complete integration paragraph.\n',
  architecture: '# Event Gateway architecture and provider integration\n\n## Full event context and restart recovery\n\n```mermaid\nflowchart LR\n A-->B\n```\n',
  protocol: '# CMA Event Gateway protocol 1.0\n\n## 8. Tools API and MCP\n\nThe complete protocol body.\n',
};
const sections = [
  {id: 'integration', label: 'Integration guide', headings: [
    {id: 'integration-title', text: 'Agent Event Gateway integration guide'},
    {id: 'integration-provider', text: 'Implement a downstream agent provider'},
  ], diagrams: 0, unrenderedDiagrams: 0},
  {id: 'architecture', label: 'Architecture', headings: [
    {id: 'architecture-title', text: 'Event Gateway architecture and provider integration'},
    {id: 'architecture-recovery', text: 'Full event context and restart recovery'},
  ], diagrams: 1, unrenderedDiagrams: 0},
  {id: 'protocol', label: 'Protocol reference', headings: [
    {id: 'protocol-title', text: 'CMA Event Gateway protocol 1.0'},
    {id: 'protocol-tools', text: '8. Tools API and MCP'},
  ], diagrams: 0, unrenderedDiagrams: 0},
];
const links = ['#integration', '#architecture', '#protocol', '#protocol-tools'];
const headingIds = sections.flatMap(section => section.headings.map(heading => heading.id));
function download(trailing = '') {
  return '# Agent Event Gateway\n\n' + Object.entries(sources).map(([id, source]) => {
    const secondaryId = sections.find(section => section.id === id).headings[1].id;
    return `<a id="${id}"></a>\n\n<a id="${id}-title"></a>\n\n${source.replace('## ', `<a id="${secondaryId}"></a>\n\n## `)}`;
  }).join('\n\n---\n\n') + trailing;
}

test('EG-004 preserves its complete frozen journey with actual EG documentation binding', () => {
  assert.equal(documentationCase.id, 'C275:EG-004');
  assert.deepEqual(documentationCase.steps, ['step_1', 'step_2', 'expected_result']);
  assert.deepEqual(documentationCase.fixtures, ['browser', 'eg_browser']);
  assert.equal(documentationCase.evidence_scope, 'eg_only');
  const session = {documentation: binding, egEvidence() {}, switchEG() {}, networkIdle() {}};
  assert.equal(validateDocumentationBinding(session), binding);
  validateDocumentationBinding({...session, documentation: {...binding, publicBase: 'http://127.0.0.1:4321'}});
  for (const publicBase of ['https://cma.nyxid.ai/event-gateway', 'http://127.0.0.1:4321/?token=x',
    'http://forbidden-userinfo@127.0.0.1:4321/event-gateway', 'http://127.0.0.1:4321/event-gateway/'])
    assert.throws(() => validateDocumentationBinding({...session, documentation: {...binding, publicBase}}),
      error => error instanceof Blocked);
  assert.throws(() => validateDocumentationBinding({...session, egEvidence: undefined}), error => error instanceof Blocked);
});

test('canonical source has twelve diagrams and fences cannot create phantom headings', () => {
  const assets = new URL('../../../../../../api/crates/cmaeg/assets/docs/public/', import.meta.url);
  // Older source revisions keep these public guides at the repository root.
  // Select one layout so an incomplete asset directory cannot use stale guides.
  const root = existsSync(assets) ? assets : new URL('../../../../../../docs/', import.meta.url);
  for (const [name, count] of [['Integration', 0], ['Architecture', 2], ['Protocol', 10]]) {
    const outline = markdownOutline(readFileSync(new URL(`CMAEG_${name}.md`, root), 'utf8'));
    assert.equal(outline.diagrams, count); assert.ok(outline.headings.length > 5);
  }
  assert.deepEqual(markdownOutline('# Real\n\n````text\n## False\n```\n# False too\n````\n## Also real'),
    {headings: ['Real', 'Also real'], diagrams: 0});
  assert.throws(() => markdownOutline('```mermaid\nA-->B'), /unclosed_markdown_fence/);
});

test('documentation structure rejects zero diagrams, missing headings and broken anchors', () => {
  assertDocumentationStructure(ctx, sources, sections, links);
  for (const change of [v => v.pop(), v => v[1].diagrams = 0, v => v[1].unrenderedDiagrams = 1,
    v => v[0].headings.pop(), v => v[2].headings[1].id = 'integration-title',
    v => v[2].headings[1].text = 'Unrelated text']) {
    const value = structuredClone(sections); change(value);
    assert.throws(() => assertDocumentationStructure(ctx, sources, value, links));
  }
  assert.throws(() => assertDocumentationStructure(ctx, sources, sections, [...links, '#missing-heading']));
});

test('download must contain complete source, not just three section markers', () => {
  assertDocumentationDownload(ctx, download(), sources, binding, headingIds);
  assertDocumentationDownload(ctx, download('\n\n---\n\n'), sources, binding, headingIds);
  for (const value of [download().replace('The complete protocol body.', ''),
    download().replace('A complete integration paragraph.', 'Different prose.'),
    download().replace('<a id="protocol-tools"></a>\n\n', ''),
    download().replace('<a id="protocol"></a>', '<a id="architecture"></a>'),
    '# Agent Event Gateway\n' + Object.keys(sources).map(id => `<a id="${id}"></a>\n\n`).join('')])
    assert.throws(() => assertDocumentationDownload(ctx, value, sources, binding, headingIds));
  const linked = {...sources, protocol: sources.protocol + '\n[tool](#missing-heading)\n'};
  assert.throws(() => assertDocumentationDownload(ctx, download().replace('The complete protocol body.',
    'The complete protocol body.\n\n[tool](#missing-heading)'), linked, binding, headingIds));
});

test('known deployment URL rewriting preserves the complete document content', () => {
  const body = `${binding.publicBase}/console\n${binding.publicBase}/v1/channels`;
  assert.equal(canonicalMarkdown(body, binding),
    'https://cma.nyxid.ai/event-gateway/legacy-console\nhttps://cma.nyxid.ai/event-gateway/v1/channels');
  assert.equal(canonicalMarkdown(body, {...binding, advertiseCanonical: true}),
    'https://cma.nyxid.ai/event-gateway/console\nhttps://cma.nyxid.ai/event-gateway/v1/channels');
});

test('OpenAPI checks the actual gateway and its versioned server, not a generic JSON response', () => {
  const value = JSON.parse(readFileSync(new URL('../../../../../../api/crates/cmaeg/openapi/v1.json', import.meta.url)));
  value.servers = [{url: binding.publicBase + '/v1'}];
  assertDocumentationOpenApi(ctx, value, binding.publicBase);
  for (const change of [v => v.info.title = 'Agent Manager', v => v.servers = [{url: '/v1'}],
    v => delete v.paths['/channels/{channel_id}/activity'], v => v.paths['/channels'].get.operationId = '']) {
    const broken = structuredClone(value); change(broken);
    assert.throws(() => assertDocumentationOpenApi(ctx, broken, binding.publicBase));
  }
});

async function guardFixture() {
  const cleanups = [], calls = [];
  const page = {route: async (_pattern, handler) => {page.handler = handler;}, unroute: async () => calls.push('unroute')};
  const guard = await guardDocumentationReads({...ctx, defer: (_label, fn) => cleanups.push(fn)},
    {page, origin: 'http://127.0.0.1:1234'}, binding);
  const send = (url, method = 'GET', fail = false) => page.handler({request: () => ({url: () => url, method: () => method}),
    fallback: async () => {calls.push('forward'); if (fail) throw Error('forward_failed');},
    abort: async () => calls.push('abort')});
  return {guard, send, calls, cleanup: cleanups[0]};
}

test('documentation guard permits only admitted local reads and retains failures for cleanup', async () => {
  const f = await guardFixture();
  await f.send('http://127.0.0.1:1234/docs'); await f.send(binding.publicBase + '/docs/diagrams/architecture-0.svg');
  assert.equal(f.guard.refused(), 0); await f.cleanup();
  assert.deepEqual(f.calls, ['forward', 'forward', 'unroute']);
  const bad = await guardFixture();
  await bad.send('https://cma.nyxid.ai/event-gateway/docs'); await bad.send(binding.publicBase + '/channels', 'POST');
  assert.equal(bad.guard.refused(), 2); assert.deepEqual(bad.calls, ['abort', 'abort']);
  await assert.rejects(bad.cleanup(), /documentation_guard_failed/);
  const failed = await guardFixture(); await failed.send(binding.publicBase + '/docs', 'GET', true);
  await assert.rejects(failed.cleanup(), /documentation_guard_failed/);
});

function receipt() {
  const serving = {schema: 'c275-eg-serving-v1', scope: 'eg', replica: 'b', origin: 'http://127.0.0.1:4321',
    ready: true, pod_uid: 'eg-pod', instance_id: 'eg-instance', container_id: 'eg-container',
    source: 'a'.repeat(40), image: 'eg@sha256:' + 'b'.repeat(64), observed_at: '2026-11-16T10:00:00.000Z'};
  return {scope: 'eg', replica: 'b', request_id: 'public-request', method: 'GET', status: 200, serving,
    serving_after: {...serving, observed_at: '2026-11-16T10:00:00.003Z'},
    started_at: '2026-11-16T10:00:00.001Z', ended_at: '2026-11-16T10:00:00.002Z',
    attribution: {verdict: 'PASS', basis: 'pod_forward_before_after'}};
}

test('public reads retain actual response identity and reject wrong serving source or missing receipts', async t => {
  const requests = [];
  const server = http.createServer((request, response) => {
    requests.push({method: request.method, path: request.url});
    response.writeHead(200, {'content-type': 'text/markdown; charset=utf-8', 'x-request-id': 'public-request'});
    response.end(sources.integration);
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  t.after(async () => {server.closeAllConnections(); await new Promise(resolve => server.close(resolve));});
  const session = {origin: `http://127.0.0.1:${server.address().port}`, page: {evaluate: (fn, args) => fn(args)},
    networkIdle: async () => {}, egEvidence: async () => [receipt()]};
  const value = await readDocumentation(ctx, session, '/event-gateway/docs/integration.md', 'text/markdown', 'a'.repeat(40));
  assert.equal(value.body, sources.integration); assert.equal(value.receipt.replica, 'b');
  assert.deepEqual(requests[0], {method: 'GET', path: '/event-gateway/docs/integration.md'});
  await assert.rejects(readDocumentation(ctx, session, '/event-gateway/docs/integration.md', 'text/markdown', 'c'.repeat(40)),
    /documentation_web_gateway_source_join/);
  session.egEvidence = async () => [];
  await assert.rejects(readDocumentation(ctx, session, '/event-gateway/docs/integration.md', 'text/markdown', 'a'.repeat(40)),
    /channel_actual_eg_browser_receipt/);
});
