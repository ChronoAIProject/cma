import test from 'node:test';
import assert from 'node:assert/strict';
import {assertCapabilityShape, sourceChoices, egBrowserReceipt, guardDiscoveryWrites,
  channelCapabilitiesCase} from './channel-capabilities.mjs';
import {Blocked} from '../../runner.mjs';

const ctx = {assert: (condition, code) => assert.ok(condition, code)};
const capability = () => ({platforms: [{id: 'telegram', label: 'Telegram',
  connection_methods: ['existing_credential', 'managed_create'], optional_capabilities: ['manage_bots']}],
relay_authoring: {dynamic_sources_writes_enabled: false, relay_raw_verification_writes_enabled: false,
  multi_consumer_writes_enabled: false, native_bot_mentions_writes_enabled: false, thread_contract_writes_enabled: false,
  platforms: [{id: 'discord', label: 'Discord', verification_modes: [
    {id: 'raw_event', writes_enabled: false}, {id: 'trusted_normalized', writes_enabled: true}]}]}});

test('channel discovery keeps managed connection capabilities distinct from relay verification gates', () => {
  const value = capability(); assert.equal(assertCapabilityShape(ctx, value), value.relay_authoring);
  assert.equal(channelCapabilitiesCase.evidence_scope, 'eg_only');
  assert.throws(() => assertCapabilityShape(ctx, {platforms: value.platforms}), error => error instanceof Blocked);
  for (const change of [v => {v.relay_authoring.platforms = v.platforms;},
    v => {v.relay_authoring.platforms[0].verification_modes[0].writes_enabled = 'false';},
    v => {v.relay_authoring.platforms.push(v.relay_authoring.platforms[0]);},
    v => {v.relay_authoring.platforms[0].verification_modes.push({id: 'raw_event', writes_enabled: true});},
    v => {v.relay_authoring.thread_contract_writes_enabled = undefined;}]) {
    const changed = capability(); change(changed); assert.throws(() => assertCapabilityShape(ctx, changed), /channel_/);
  }
});

test('new native source setup needs activation gates; an unsupported provider remains disabled', () => {
  const disabled = sourceChoices(capability().relay_authoring);
  assert.deepEqual(disabled.filter(choice => choice.present).map(choice => choice.label),
    ['NyxID messaging connection', 'Direct Telegram bot', 'Custom event']);
  const authoringOnly = {native_lark_writes_enabled: true, native_whatsapp_writes_enabled: true,
    native_discord_writes_enabled: true};
  assert.equal(sourceChoices(authoringOnly).filter(c => c.present).length, 3);
  const ready = {...authoringOnly, native_lark_intake_enabled: true, native_whatsapp_auto_activation_enabled: true,
    native_discord_auto_activation_enabled: true, multi_consumer_writes_enabled: true, response_recovery_providers: ['cma_codex']};
  assert.ok(sourceChoices(ready).every(c => c.present && c.enabled));
  const mismatched = sourceChoices(ready, 'unsupported-provider').find(c => c.label === 'Direct Discord bot');
  assert.deepEqual(mismatched, {label: 'Direct Discord bot', present: true, enabled: false});
});

test('channel browser evidence requires real EG attribution and never accepts an AM label', () => {
  const serving = {schema: 'c275-eg-serving-v1', scope: 'eg', replica: 'b', origin: 'http://127.0.0.1:1234',
    ready: true, pod_uid: 'eg-pod', instance_id: 'eg-instance', container_id: 'eg-container', source: 'a'.repeat(40),
    image: 'local-eg@sha256:' + 'b'.repeat(64), observed_at: '2026-11-16T10:00:00.000Z'};
  const receipt = {scope: 'eg', replica: 'b', request_id: 'actual-eg-request', method: 'GET', status: 200,
    serving, serving_after: {...serving, observed_at: '2026-11-16T10:00:00.003Z'},
    started_at: '2026-11-16T10:00:00.001Z', ended_at: '2026-11-16T10:00:00.002Z',
    attribution: {verdict: 'PASS', basis: 'pod_forward_before_after'}};
  assert.equal(egBrowserReceipt(ctx, [receipt], receipt.request_id).replica, 'b');
  for (const receipts of [[{...receipt, am: 'b'}], [{...receipt, scope: 'am'}], [{...receipt, replica: 'a'}],
    [{...receipt, method: 'POST'}], [{...receipt, request_id: 'different'}], [receipt, receipt],
    [{...receipt, serving_after: {...receipt.serving_after, source: 'c'.repeat(40)}}]])
    assert.throws(() => egBrowserReceipt(ctx, receipts, receipt.request_id), /actual_eg_receipt/);
});

async function guardFixture(options = {}) {
  const calls = [], deferred = []; let handler;
  const page = {url: () => 'http://127.0.0.1:1234/event-gateway', async route(matcher, work) {
    assert.ok(matcher(new URL('http://127.0.0.1:1234/api/event-gateway/v1/channels')));
    assert.equal(matcher(new URL('http://127.0.0.1:4321/api/event-gateway/v1/channels')), false);
    assert.equal(matcher(new URL('http://127.0.0.1:1234/api/v1/bots')), false); handler = work;
  }, async unroute() {calls.push('unroute'); if (options.unrouteFailure) throw Error('unroute_failed');}};
  const guard = await guardDiscoveryWrites({...ctx, defer: (_label, work) => deferred.push(work)}, page);
  return {calls, guard, cleanup: () => deferred[0](), dispatch: method => handler({request: () => ({method: () => method}),
    async fallback() {calls.push('forward:' + method);}, async abort(reason) {
      assert.equal(reason, 'blockedbyclient'); calls.push('abort:' + method);
      if (options.waitAbort) await options.waitAbort;
      if (options.abortFailure) throw Error('abort_failed');
    }})};
}

test('capability discovery refuses accidental writes before forwarding and cannot hide them at cleanup', async () => {
  const reads = await guardFixture();
  for (const method of ['GET', 'HEAD', 'OPTIONS']) await reads.dispatch(method);
  assert.equal(reads.guard.attempts(), 0); await reads.cleanup();
  const writes = await guardFixture();
  for (const method of ['POST', 'PUT', 'PATCH', 'DELETE']) await writes.dispatch(method);
  assert.equal(writes.guard.attempts(), 4);
  assert.ok(!writes.calls.some(call => call.startsWith('forward:')));
  await assert.rejects(writes.cleanup(), error => error.errors.some(e => e.message === 'channel_discovery_attempted_write'));
});

test('discovery guard cleanup joins pending aborts and preserves every failure', async () => {
  let finish; const waitAbort = new Promise(resolve => {finish = resolve;});
  const fixture = await guardFixture({waitAbort, unrouteFailure: true, abortFailure: true});
  const pending = fixture.dispatch('PUT'); let complete = false;
  const cleaning = fixture.cleanup().catch(error => {complete = true; return error;});
  await new Promise(resolve => setImmediate(resolve)); assert.equal(complete, false);
  finish(); await pending; const error = await cleaning;
  assert.deepEqual(error.errors.map(e => e.message), ['unroute_failed', 'abort_failed', 'channel_discovery_attempted_write']);
});
