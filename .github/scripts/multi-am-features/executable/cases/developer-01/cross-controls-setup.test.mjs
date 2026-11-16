import test from 'node:test';
import assert from 'node:assert/strict';
import {createCrossOwnerControlsFactory} from './cross-controls-setup.mjs';
import {controlsModel} from './cross-controls-model.mjs';
import {crossOwnerControls} from './cross-controls.mjs';

const setup = h => createCrossOwnerControlsFactory(h.bindings)(h.ctx, {config: h.config});
const failures = error => [error.message, ...(error.errors ?? []).map(failures)].join(' ');

test('ADM-082 setup uses the public hierarchy, real authority reads and independent control inputs', async () => {
  const h = controlsModel(), f = await setup(h);
  assert.equal(f.agentId, 'agent-fixture'); assert.equal(f.targetSettings.permission_mode, 'autonomous');
  assert.equal(f.targetSettings.model, h.settings.model);
  assert.notEqual(f.title, h.preferences.title); assert.notEqual(f.prompt, h.preferences.system_prompt);
  assert.ok(f.filePath.endsWith('.txt')); assert.ok(f.content); assert.ok(f.attachmentName);
  assert.equal(new Set(Object.values(f.keys)).size, 5);
  assert.deepEqual(h.intents.map(i => i.kind), ['sandbox', 'workspace', 'operation']);
  assert.deepEqual(h.retained.map(i => i.kind), ['sandbox', 'workspace', 'operation', 'agent']);
  assert.equal(h.calls.filter(c => c.method !== 'GET').length, 3);
  assert.equal(h.sessions.length, 2); assert.ok(h.sessions.every(s => s.closed));
  await f.cleanup(); await f.cleanup(); assert.deepEqual(await h.cleanup(), []);
  assert.equal(h.root.status, 'retired'); assert.equal(h.released, 1);
  assert.equal(h.calls.filter(c => c.method === 'DELETE').length, 1);
});

test('ADM-082 returned setup data drives every case step through cleanup', async () => {
  const h = controlsModel(), f = await setup(h);
  Object.assign(h.f, f);
  await crossOwnerControls.run(h.ctx);
  assert.deepEqual(await h.cleanup(), []); assert.equal(h.released, 1);
  assert.equal(h.root.status, 'retired'); assert.ok(h.sessions.every(s => s.closed));
  assert.equal(h.settings.permission_mode, 'guarded'); assert.equal(h.preferences.title, 'Original task');
});

for (const role of ['user', 'creator', 'control_user', 'control_operator', 'read_operator']) {
  test('ADM-082 exact role binding required before public creation: ' + role, async () => {
    const h = controlsModel(); delete h.config.actors[role];
    await assert.rejects(setup(h), /admin_cross_controls_exact_roles_required/);
    assert.equal(h.calls.length, 0);
  });
}

test('ADM-082 operators cannot alias each other or borrow the resource owner', async () => {
  const h = controlsModel(); h.config.actors.control_operator.subject = h.config.actors.control_user.subject;
  await assert.rejects(setup(h), /admin_cross_controls_exact_roles_required/);
  h.config.actors.control_operator.subject = 'operator-subject';
  h.bindings.identities.user.subject = 'foreign-owner';
  await assert.rejects(setup(h), /admin_cross_controls_resource_owner/); assert.equal(h.calls.length, 0);
});

test('ADM-082 requires independent parent cleanup custody', async () => {
  const h = controlsModel(); delete h.bindings.userResources.finality;
  await assert.rejects(setup(h), /user_resource_custody_required/); assert.equal(h.calls.length, 0);
});

test('ADM-082 chooses a changed permission mode from the actual current settings', async () => {
  const h = controlsModel({originalPermissionMode: 'autonomous'}), f = await setup(h);
  assert.equal(f.targetSettings.permission_mode, 'guarded');
  assert.deepEqual(await h.cleanup(), []);
});

for (const missing of ['compiledWeb', 'control_user', 'control_operator']) {
  test('ADM-082 requires exact compiled cookie identity before allocation: ' + missing, async () => {
    const h = controlsModel();
    if (missing === 'compiledWeb') delete h.bindings.compiledWeb;
    else h.bindings.identities[missing].subject = 'mismatched-subject';
    await assert.rejects(setup(h), /compiled_web_artifact_identity_required|admin_cross_controls_browser_identity/);
    assert.equal(h.calls.length, 0);
  });
}

for (const [option, code] of [['roleEscalation', 'role_authority'], ['pendingSettings', 'ready_settings'],
  ['unsupportedPrompt', 'ready_preferences'], ['busy', 'idle_required'],
  ['controlOperatorCanConfigure', 'configuration_authority'], ['wrongCookieSubject', 'setup_session_identity']]) {
  test('ADM-082 refuses an authentic prerequisite mismatch: ' + option, async () => {
    const h = controlsModel({[option]: true}); await assert.rejects(setup(h), new RegExp(code));
    assert.deepEqual(await h.cleanup(), []); assert.equal(h.released, 1);
    assert.ok(!h.calls.some(c => c.method === 'PATCH' || c.path.endsWith('/stop')));
  });
}

test('a permitted control-only preference read fails the checklist instead of blocking setup', async () => {
  const h = controlsModel({controlReadAllowed: true}), f = await setup(h); Object.assign(h.f, f);
  await assert.rejects(crossOwnerControls.run(h.ctx), /status_200/);
  assert.ok(h.calls.some(c => c.actor === 'control_operator' && c.method === 'GET' && c.path.endsWith('/preferences')));
  assert.deepEqual(await h.cleanup(), []); assert.equal(h.released, 1);
});

for (const [option, code] of [['unknownRetire', 'retire_unknown'], ['pendingAfterRetirement', 'cleanup_finality']]) {
  test('ADM-082 retains failed native or public cleanup without another retirement: ' + option, async () => {
    const h = controlsModel({[option]: true}), f = await setup(h);
    for (let i = 0; i < 2; i++) await assert.rejects(f.cleanup(), error => failures(error).includes(code));
    assert.equal(h.calls.filter(c => c.method === 'DELETE').length, 1); assert.equal(h.released, 0);
  });
}
