import test from 'node:test';
import assert from 'node:assert/strict';
import {Readable} from 'node:stream';
import {crossOwnerReads} from './cross-owner.mjs';
import {readOperatorDownloads} from './cross-read-browser.mjs';
import {digest} from './access-common.mjs';

// The model rejects download clicks until the real UI control sequence has
// selected the file. Runtime browser/serving proof is still the sole parent's.
function model(options = {}) {
  const content = 'Private local read contents\n';
  const f = {agentId: 'agent-fixture', content, filePath: 'folder/known.txt',
    fileUiPath: '/agents/agent-fixture', downloadLabel: 'Download file',
    transcriptMarker: 'assistant-marker-fixture', deniedMarker: 'denied-marker-fixture',
    deniedInput: {text: 'Reply denied-marker-fixture'}, deniedKey: 'denied-key',
    ownerSubject: 'owner-fixture', operatorSubject: 'reader-fixture',
    scope: 'LOCAL_DISPOSABLE_TWO_AM', createdByRun: true, cleanup: async () => {}};
  const sessions = [], cleanups = [], observations = [], requests = [], downloads = [];
  const adapter = {async open({actor, am, agentId}) {
    assert.equal(actor, 'read_operator'); assert.equal(agentId, f.agentId);
    let selected = am, panel = false, files = false, opened = [], responseWait, downloadWait, sequence = 0;
    const receipts = [];
    const record = (method = 'GET', status = 200) => {
      const actual = options.pinIngress ? 'a' : selected;
      const receipt = {am: actual, serving_am: actual, method, status,
        request_id: 'session-' + sessions.length + '-receipt-' + ++sequence,
        serving: {pod_uid: 'pod-' + actual}, serving_after: {pod_uid: 'pod-' + actual},
        started_at: '2026-11-16T16:00:00Z', ended_at: '2026-11-16T16:00:01Z', attribution: 'PASS'};
      receipts.push(receipt); return receipt;
    };
    const click = async (role, name) => {
      if (role === 'button' && name === 'Show Agent computer') {panel = true; return;}
      if (role === 'tab' && name === 'Files') {assert.ok(panel, 'computer panel is closed'); files = true; return;}
      if (role === 'file') {
        assert.ok(files, 'Files tab is closed');
        assert.equal(name, f.filePath.split('/')[opened.length]); opened.push(name); return;
      }
      assert.equal(name, 'Download file');
      assert.deepEqual(opened, f.filePath.split('/'), 'file preview must be open before download');
      const receipt = record(); downloads.push({requested: selected, receipt});
      const response = {request: () => ({method: () => 'GET'}),
        url: () => 'http://127.0.0.1:19001/api/agent-manager/v1/agents/agent-fixture/files/download?'
          + new URLSearchParams({path: f.filePath}),
        headerValue: async () => options.unjoined ? 'different-request' : receipt.request_id,
        status: () => options.failedResponse ? 503 : 200};
      if (responseWait) {assert.ok(responseWait.predicate(response)); responseWait.resolve(response);}
      const bytes = options.oversize ? Buffer.alloc(1048577)
        : Buffer.from(options.wrongBytes ? 'wrong' : content);
      downloadWait?.({suggestedFilename: () => options.wrongName ? 'another.txt' : 'known.txt',
        createReadStream: async () => options.noStream ? null : Readable.from([bytes])});
    };
    const page = {async goto(url) {
      assert.equal(new URL(url).pathname, '/agents/' + f.agentId);
      panel = false; files = false; opened = []; record();
    }, getByRole(role, {name}) {
      return {click: () => click(role, name), isVisible: async () => name !== 'Send message',
        isDisabled: async () => true,
        waitFor: async () => {assert.equal(name, 'Home files'); assert.ok(files);},
        getByRole(childRole, {name: childName}) {
          assert.equal(role, 'list'); assert.equal(name, 'Home files'); assert.equal(childRole, 'button');
          return {click: () => click('file', childName)};
        }};
    }, getByText(text) {assert.equal(text, f.transcriptMarker); return {waitFor: async () => {}};},
    waitForResponse: predicate => new Promise(resolve => {responseWait = {predicate, resolve};}),
    waitForEvent: event => {assert.equal(event, 'download'); return new Promise(resolve => {downloadWait = resolve;});}};
    const session = {page, origin: 'http://127.0.0.1:19001',
      switchAM: async am => {selected = am;}, close: async () => {
        session.closed = true; if (options.lateMutation) record('POST', 202);
      }, receipts};
    sessions.push(session); return session;
  }, evidence: async page => ({requests: sessions.find(s => s.page === page).receipts})};
  const ctx = {am: 'a', defer: (label, run) => cleanups.push({label, run}),
    assert: (value, code) => assert.ok(value, code), step: async (_name, run) => run(),
    observe: async (label, run) => {const value = await run(); observations.push({label, value}); return value;},
    fixture: name => name === 'browser' ? adapter : {take: async () => f},
    request: async args => {
      requests.push(args); const path = args.path;
      if (path.startsWith('/api/v1/resource-access/')) return {status: 200, body: {
        owner: {kind: 'person', is_current_person: false}, capabilities: ['view_details', 'view_history', 'view_files']}};
      if (args.method === 'POST' || args.actor === 'outsider') return {status: 403, body: {}};
      if (path.endsWith('/transcript')) return {status: 200, body: {turns: [{
        messages: [{role: 'assistant', content: f.transcriptMarker}]}]}};
      if (path.endsWith('/files/tree')) return {status: 200, body: {entries: [{path: f.filePath}]}};
      if (path.includes('/files/content?')) return {status: 200, body: {
        sha256: digest(content), content_base64: Buffer.from(content).toString('base64')}};
      return {status: 200, body: {id: f.agentId}};
    }};
  return {ctx, f, sessions, requests, downloads, observations, cleanups,
    async open() {return adapter.open({actor: 'read_operator', am: ctx.am, agentId: f.agentId});},
    async cleanup() {
      const errors = [];
      for (const {run} of cleanups.toReversed()) {try {await run();} catch (error) {errors.push(error);}}
      if (errors.length) throw new AggregateError(errors, 'browser_cleanup_failed');
    }};
}

test('ADM-080 opens the real Files controls and downloads through both AMs', async () => {
  const h = model(); await crossOwnerReads.run(h.ctx); await h.cleanup();
  assert.deepEqual(h.downloads.map(r => r.receipt.am), ['a', 'b']);
  assert.ok(h.sessions.every(s => s.closed));
  assert.deepEqual(h.requests.filter(r => r.path.includes('/files/content?')).map(r => r.am), ['a', 'b']);
  assert.equal(h.requests.filter(r => r.method === 'POST').length, 2);
  const retained = h.observations.filter(o => o.label === 'admin_browser_evidence').flatMap(o => o.value.requests);
  assert.ok(retained.every(r => r.serving && r.serving_after && r.started_at && r.ended_at && r.attribution));
  assert.equal(h.observations.filter(o => o.label === 'admin_cross_read_download').length, 2);
});

test('requested AM labels cannot hide a download pinned to one serving AM', async () => {
  const h = model({pinIngress: true});
  await assert.rejects(crossOwnerReads.run(h.ctx), /admin_cross_read_download_ingress/);
  await h.cleanup(); assert.deepEqual(h.downloads.map(r => r.receipt.am), ['a', 'a']);
});

for (const [option, code] of [['wrongBytes', 'admin_cross_owner_download_bytes'],
  ['wrongName', 'admin_cross_read_download_identity'], ['noStream', 'admin_cross_read_download_identity'],
  ['oversize', 'admin_download_bound'], ['unjoined', 'admin_cross_read_download_ingress'],
  ['failedResponse', 'admin_cross_read_download_response']]) {
  test('download refuses an invalid observation: ' + option, async () => {
    const h = model({[option]: true});
    await assert.rejects(crossOwnerReads.run(h.ctx), new RegExp(code));
    await h.cleanup(); assert.ok(h.sessions.every(s => s.closed));
    assert.equal(h.downloads.length, 1);
  });
}

test('download path traversal blocks before navigation', async () => {
  const h = model(), session = await h.open(); h.f.filePath = '../outside.txt';
  await assert.rejects(readOperatorDownloads(h.ctx, h.f, session), /admin_cross_read_relative_file_required/);
  assert.equal(h.downloads.length, 0); assert.equal(session.receipts.length, 0);
});

test('late browser mutation discovered during close prevents read-only acceptance', async () => {
  const h = model({lateMutation: true}); await crossOwnerReads.run(h.ctx);
  await assert.rejects(h.cleanup(), error => {
    assert.ok(error.errors.some(e => /admin_cross_read_browser_mutation/.test(e.message))); return true;
  });
  assert.ok(h.sessions.every(s => s.closed));
  assert.ok(h.observations.some(o => o.label === 'admin_browser_evidence'
    && o.value.requests.some(r => r.method === 'POST')));
});
