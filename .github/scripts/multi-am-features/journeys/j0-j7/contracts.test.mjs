import test from 'node:test';
import assert from 'node:assert/strict';
import {invokeJourney, runJourney, journey} from './index.mjs';
import {validateRegistrationCapture, registerFreshIdentity} from './identity.mjs';
import {installContext} from './context.mjs';
import {executeCase, Blocked} from '../../executable/runner.mjs';
import {signer, payload, harness, binding, release, browserSession, subject} from './test-support.mjs';
import {verifyBrowserSession} from './browser-proof.mjs';
import {digest} from './provenance.mjs';

const blocked = error => error instanceof Blocked;

test('raw journey.run cannot bypass the private parent binding using flags', async () => {
  let called = 0;
  await assert.rejects(runJourney({runState: {mfunc01_binding: binding},
    fixture() {called++;}, request() {called++;}}), blocked);
  assert.equal(called, 0);
  assert.equal(journey.steps.length, 8);
});

test('real C275 entry rejects all invalid bindings before registration or requests', async t => {
  const cases = {
    missing: h => {h.binding = undefined;},
    production: h => {h.binding.production_writes = true;},
    source: h => {h.binding.api_source = '0'.repeat(40);},
    image: h => {h.binding.api_image = 'cma-api@sha256:' + '0'.repeat(64);},
    release: h => {h.release.artifact_sha256 = '';},
    runner: h => {h.environment.image = 'cma-api@sha256:' + '0'.repeat(64);},
    single: h => {h.environment.adapter.topology = async () => ({ams: {a: {}}});},
    nonlocal: h => {h.binding.am_origins.b = 'https://example.com';},
    sameorigin: h => {h.binding.am_origins.b = h.binding.am_origins.a;},
    liveimage: h => {const get = h.environment.adapter.topology;
      h.environment.adapter.topology = async () => {const v = await get(); v.ams.b.image = 'wrong'; return v;};},
    livesource: h => {const get = h.environment.adapter.topology;
      h.environment.adapter.topology = async () => {const v = await get(); v.ams.b.source = '0'.repeat(40); return v;};},
    duplicateprocess: h => {const get = h.environment.adapter.topology;
      h.environment.adapter.topology = async () => {const v = await get(); v.ams.b.pod_uid = v.ams.a.pod_uid; return v;};},
  };
  for (const [name, change] of Object.entries(cases)) await t.test(name, async () => {
    const h = harness(); change(h);
    const result = await invokeJourney(h);
    assert.equal(result.status, 'BLOCKED', JSON.stringify(result));
    assert.equal(h.counts.registration, 0); assert.equal(h.counts.request, 0); assert.equal(h.counts.browser, 0);
  });
});

test('a registrar labelling itself real is BLOCKED and custody cleanup runs', async () => {
  const h = harness();
  h.environment.fixtures['registration-evidence'].collect = async () => ({verified: true,
    provider: 'nyxid', kind: 'real_registration', fresh: true, reused: false});
  const result = await invokeJourney(h);
  assert.equal(result.status, 'BLOCKED');
  assert.equal(h.counts.registration, 1); assert.equal(h.counts.request, 0); assert.equal(h.counts.cleanup, 1);
});

test('independent receipt is hash/signature bound, fresh and tied to the independent read', async t => {
  const signed = signer(), now = Date.now(), original = payload('challenge', now);
  const receipt = signed.capture(original);
  assert.equal(validateRegistrationCapture(receipt, signed.trust, 'challenge', now, now)
    .registration.subject_sha256, digest(subject));
  for (const [name, change] of Object.entries({
    issuer: v => {v.registration.issuer = 'https://other.invalid';},
    subject: v => {v.verification.subject_sha256 = '0'.repeat(64);},
    registration: v => {v.verification.registration_id = 'other';},
    sameRequest: v => {v.verification.request_id = v.registration.request_id;},
    oldIdentity: v => {v.registration.created_at = v.verification.created_at = new Date(now - 1).toISOString();},
    future: v => {v.verification.observed_at = new Date(now + 1).toISOString();},
    session: v => {v.session.subject_sha256 = '0'.repeat(64);},
    bearer: v => {v.session.authentication = 'bearer';},
    noProviderEvidence: v => {delete v.registration.response_sha256;},
    sessionDigest: v => {v.session.binding_sha256 = '0'.repeat(64);},
  })) await t.test(name, () => {
    const changed = structuredClone(original); change(changed);
    assert.throws(() => validateRegistrationCapture(signed.capture(changed), signed.trust, 'challenge', now, now), blocked);
  });
  const forged = signer().capture(original); forged.key_id = signed.trust.key_id;
  assert.throws(() => validateRegistrationCapture(forged, signed.trust, 'challenge', now, now), blocked);
  const tampered = structuredClone(receipt); tampered.payload.registration.status = 400;
  assert.throws(() => validateRegistrationCapture(tampered, signed.trust, 'challenge', now, now), blocked);
  assert.throws(() => validateRegistrationCapture(receipt, signed.trust, 'different', now, now), blocked);
});

test('C275 executes J0 registration and cookie proof with cleanup (synthetic unit evidence only)', async () => {
  const h = harness();
  const definition = {id: 'UNIT:J0', scope: 'global', steps: ['register'], fixtures: journey.fixtures,
    run: async ctx => {
      installContext(ctx, h);
      await ctx.step('register', async () => {
        const identity = await registerFreshIdentity(ctx);
        const proof = await verifyBrowserSession(ctx, browserSession(), identity);
        ctx.assert(proof.subject_sha256 === identity.registration.subject_sha256, 'unit_subject_join');
        await ctx.observe('unit_session_proof', async () => proof);
      });
    }};
  const result = await executeCase(definition, {case_id: definition.id, family: 'unit', required_steps: definition.steps}, h.environment);
  assert.equal(result.status, 'PASS', JSON.stringify(result));
  assert.equal(h.counts.registration, 1); assert.equal(h.counts.cleanup, 1);
  assert.ok(h.events.some(e => e.label === 'fresh_identity_registration' && e.value.capture.signature));
});

test('binding evidence precedes J0, a different cookie subject blocks before CMA', async () => {
  const h = harness();
  h.environment.fixtures.browser.open = async () => browserSession({authSubject: 'retained-owner'});
  const result = await invokeJourney(h);
  assert.equal(result.status, 'BLOCKED'); assert.equal(result.error_code, 'browser_subject_mismatch');
  assert.equal(h.counts.request, 0); assert.equal(h.counts.cleanup, 1);
  const labels = h.events.filter(e => e.kind === 'observation').map(e => e.label);
  assert.equal(labels[0], 'mfunc01_local_kind_binding');
});

test('J2/J7 browser proof rejects bearer, another cookie jar and another build', async t => {
  for (const [name, mutate] of Object.entries({
    bearer: h => {h.environment.fixtures.browser.evidence = async () => ({authentication: 'bearer'});},
    cookie: (_h, session) => {session.page.context = () => ({cookies: async () => []});},
    build: h => {h.release.web.build_info_sha256 = '0'.repeat(64);},
  })) await t.test(name, async () => {
    const h = harness(), ctx = {fixture: name => h.environment.fixtures[name], remainingMs: () => 1000};
    installContext(ctx, h);
    const value = payload('unit'), identity = {registration: value.registration, session: value.session};
    const session = browserSession(); mutate(h, session);
    await assert.rejects(verifyBrowserSession(ctx, session, identity), blocked);
  });
});

test('unknown registration outcome still invokes challenge-scoped cleanup', async () => {
  const h = harness();
  h.environment.fixtures['fresh-identity'].register = async () => {throw Error('registration_transport_lost');};
  const result = await invokeJourney(h);
  assert.equal(result.status, 'FAIL'); assert.equal(h.counts.request, 0);
  assert.equal(h.cleanups.length, 1); assert.ok(h.cleanups[0].challenge);
});


test('signed claims still require the independent provider bytes to hash and match', async () => {
  const h = harness();
  h.environment.fixtures['registration-evidence'].readCapture = async () => '{"subject":"retained-owner"}';
  const result = await invokeJourney(h);
  assert.equal(result.status, 'BLOCKED');
  assert.equal(result.error_code, 'nyxid_provider_artifact_hash_mismatch');
  assert.equal(h.counts.request, 0); assert.equal(h.counts.cleanup, 1);
});
