import test from 'node:test';
import assert from 'node:assert/strict';
import {crossOwnerControls} from './cross-controls.mjs';
import {controlsModel} from './cross-controls-model.mjs';
import {restoreAgentSettings, restoreAgentPreference, removeControlFile} from './cross-controls-cleanup.mjs';

test('ADM-082 accepts the actual disabled shared Rename and enabled owner control', async () => {
  const h = controlsModel(); await crossOwnerControls.run(h.ctx);
  assert.deepEqual(await h.cleanup(), []);
  assert.ok(h.sessions.every(s => s.closed)); assert.equal(h.fixtureCleaned, 1);
  assert.equal(h.settings.permission_mode, 'guarded'); assert.equal(h.preferences.title, 'Original task');
  assert.equal(h.preferences.system_prompt, '');
});

test('ADM-082 rejects an enabled shared Rename control', async () => {
  const h = controlsModel({sharedRenameEnabled: true});
  await assert.rejects(crossOwnerControls.run(h.ctx), /admin_shared_ui_owner_rename_exposed/);
  assert.deepEqual(await h.cleanup(), []);
});

test('ADM-082 requires the positive owner Rename control to be enabled', async () => {
  const h = controlsModel({legacyAbsentRename: true, ownerRenameDisabled: true});
  await assert.rejects(crossOwnerControls.run(h.ctx), /admin_owner_rename_control_missing/);
  assert.deepEqual(await h.cleanup(), []);
});

test('ADM-082 peer readback follows actual mutation receipts, not the scheduled ingress label', async () => {
  const h = controlsModel({implicitAM: 'b'}); await crossOwnerControls.run(h.ctx);
  for (const suffix of ['/settings', '/preferences', '/files/content?']) {
    assert.ok(h.calls.some(c => c.method === 'GET' && c.path.includes(suffix) && c.am === 'a'));
  }
  assert.deepEqual(await h.cleanup(), []);
});

test('a failed attachment upload retains its response-wait rejection and still cleans up', async () => {
  const h = controlsModel({uploadFails: true});
  await assert.rejects(crossOwnerControls.run(h.ctx), /upload_failed/);
  assert.deepEqual(await h.cleanup(), []); assert.ok(h.sessions.every(s => s.closed));
  assert.equal(h.fixtureCleaned, 1);
});

for (const kind of ['settings', 'preference', 'file']) {
  test('cleanup protects foreign ' + kind + ' state', async () => {
    const h = controlsModel(), calls = [];
    const ctx = {defer() {}, request: async args => {calls.push(args); return {status: 200,
      body: {model: 'foreign-model', title: 'foreign-title', path: 'foreign-path', sha256: 'foreign-digest'}};}};
    const restore = kind === 'settings' ? restoreAgentSettings(ctx, h.f, h.settings)
      : kind === 'preference' ? restoreAgentPreference(ctx, h.f, 'title', 'Original task', h.f.title)
        : removeControlFile(ctx, h.f);
    await assert.rejects(restore(), /foreign/); await assert.rejects(restore(), /foreign/);
    assert.equal(calls.length, 1); assert.equal(calls[0].method, 'GET');
  });
}

for (const kind of ['settings', 'title', 'system_prompt', 'file']) for (const restoreCommitted of [false, true]) {
  test('uncertain ' + kind + ' restoration is retained without replay; committed=' + restoreCommitted, async () => {
    const h = controlsModel({legacyAbsentRename: true, unknownRestore: kind, restoreCommitted});
    await assert.rejects(crossOwnerControls.run(h.ctx), /unknown/);
    const matches = () => h.calls.filter(c => kind === 'file' ? c.method === 'DELETE'
      : c.method === 'PATCH' && c.key === (kind === 'settings' ? h.f.keys.restoreSettings : h.f.keys[kind] + '-restore'));
    assert.equal(matches().length, 1);
    assert.ok((await h.cleanup()).some(error => error.message.includes('unknown')));
    assert.equal(matches().length, 1);
    assert.ok(!h.calls.some(c => c.path.endsWith('/stop')));
    assert.equal(h.fixtureCleaned, 1);
  });
}
