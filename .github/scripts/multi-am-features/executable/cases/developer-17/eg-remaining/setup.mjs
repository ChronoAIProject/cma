import { validServingReceipt } from '../../../serving.mjs';
import { retainBrowserEvidence } from './receipts.mjs';
import { randomBytes } from 'node:crypto';
import { createTransport } from './transport.mjs';
import {
  enc,
  other,
  unique,
  requireInput,
  poll,
  assertNoSecret,
  validateBrowserEvidence,
  closeAndCapture,
} from './core.mjs';

const check = (condition, code) => {
  if (!condition) throw Error(code);
};

export const names = [
  'capabilities',
  'create',
  'recover',
  'sources',
  'admission',
  'profile',
  'routing',
  'activity',
  'outbound-prepare',
  'outbound-send',
  'connections',
  'connection-create',
  'connection-cancel',
  'consumers',
  'consumer-create',
  'father-profile',
  'father-actions',
  'console-auth',
  'console-inspect',
  'console-sources',
];

// Parent primitives grant access to a real local fixture. They never implement
// a checklist step or return its expected verdict. All resource setup uses API.
export async function prepare(ctx, primitives, name) {
  requireInput(names.includes(name), 'eg20_unknown_setup');
  const parent = requireInput(primitives.egCoverage, 'eg20_parent_primitives_required');
  requireInput(
    parent.scope === 'LOCAL_KIND_DISPOSABLE_TWO_AM' &&
      parent.synthetic === true &&
      typeof parent.reserve === 'function' &&
      typeof parent.openBrowser === 'function',
    'eg20_parent_local_kind_required',
  );
  const eg = await createTransport(ctx, primitives.eg);
  const needs = {
    channel: ![
      'consumers',
      'consumer-create',
      'father-profile',
      'father-actions',
      'console-auth',
    ].includes(name),
    platform: [
      'admission',
      'routing',
      'activity',
      'outbound-prepare',
      'outbound-send',
      'connections',
      'connection-create',
      'connection-cancel',
      'consumers',
      'consumer-create',
      'father-profile',
      'father-actions',
      'console-inspect',
      'console-sources',
    ].includes(name),
    agentProof: [
      'create',
      'recover',
      'admission',
      'profile',
      'routing',
      'activity',
      'connections',
      'consumer-create',
      'father-profile',
      'father-actions',
      'console-inspect',
    ].includes(name),
    console: name.startsWith('console-'),
    configVariant: name === 'capabilities',
  };
  const lease = await parent.reserve({
    caseId: ctx.row.case_id,
    phase: ctx.phase,
    iteration: ctx.iteration,
    needs,
  });
  let finishLease = async () => {
    requireInput(
      typeof lease?.finality === 'function' &&
        typeof lease?.release === 'function' &&
        typeof lease?.dispose === 'function',
      'eg20_partial_lease_cleanup_required',
    );
    await lease.dispose();
    const final = await lease.finality();
    check(
      final.unresolved === 0 &&
        final.in_flight === 0 &&
        final.resources_remaining === 0 &&
        final.private_custody_retained === 0,
      'eg20_partial_lease_unresolved',
    );
    await lease.release();
  };
  ctx.defer('cleanup_partial_eg20_setup', () => finishLease());
  for (const method of ['retainIntent', 'capture', 'dispose', 'finality', 'release'])
    requireInput(
      typeof lease?.[method] === 'function',
      `eg20_custody_${method.toLowerCase()}_required`,
    );
  requireInput(
    lease.local_kind === true && lease.exclusive === true && lease.parent_cleanup_barrier === true,
    'eg20_exclusive_parent_custody_required',
  );
  const authority = lease.authority;
  requireInput(
    authority?.ownerSubject &&
      authority.agentKey &&
      authority.provider?.slug &&
      authority.profile?.metadata?.owner,
    'eg20_real_authority_required',
  );
  if (needs.platform)
    requireInput(
      lease.platform?.synthetic === true &&
        typeof lease.platform.emit === 'function' &&
        typeof lease.platform.journal === 'function',
      'eg20_actual_platform_journal_required',
    );
  if (needs.console)
    requireInput(
      lease.console?.login &&
        lease.console.allowedOwner &&
        Number.isSafeInteger(lease.console.expiryWaitMs),
      'eg20_real_console_identity_required',
    );
  if (needs.agentProof)
    requireInput(
      typeof lease.agentRequests === 'function',
      'eg20_actual_agent_request_observer_required',
    );
  const channels = new Map(),
    bots = new Set(),
    connections = new Set(),
    profiles = new Set(),
    pending = new Set();
  let session,
    cleanupPromise,
    interception,
    listener,
    browserStarted = false;
  const restores = [],
    browserIntents = [],
    responses = [],
    observerErrors = [];
  const fx = {
    authority,
    lease,
    eg,
    platform: lease.platform,
    name,
    session: null,
    secrets: [
      authority.agentKey,
      authority.channelKey,
      lease.console?.login?.password,
      ...(authority.syntheticSecrets ?? []),
    ].filter(Boolean),
    keys: { create: unique('eg20-create'), prepare: unique('eg20-prepare') },
    recordId: unique(''),
    channelName: unique('EG20'),
    capture: async (value) => {
      responses.push(value);
      const body = value.body;
      const accepted = value.status >= 200 && value.status < 300;
      if (accepted && body?.record_id) channels.set(body.record_id, body.channel_id ?? null);
      if (accepted && body?.bot_id) bots.add(body.bot_id);
      if (accepted && body?.result?.bot_id) bots.add(body.result.bot_id);
      if (accepted && body?.connection_ref) connections.add(body.connection_ref);
      if (accepted && value.kind === 'profile' && body?.id) profiles.add(body.id);
      await lease.capture(value);
    },
    async call(args) {
      const mutation = !['GET', 'HEAD'].includes(args.method ?? 'GET');
      const intent = { scope: 'eg', ...args };
      if (mutation) await lease.retainIntent(intent);
      try {
        const response = await eg(args);
        if (mutation) await fx.capture({ ...intent, status: response.status, body: response.body });
        return response;
      } catch (error) {
        if (error.response) await fx.capture({ ...intent, ...error.response });
        throw error;
      }
    },
    async am(args) {
      const mutation = !['GET', 'HEAD'].includes(args.method ?? 'GET');
      const intent = { scope: 'am', ...args };
      if (mutation) await lease.retainIntent(intent);
      try {
        const response = await ctx.request({ ...args, am: args.am ?? ctx.am });
        if (mutation) await fx.capture({ ...intent, status: response.status, body: response.body });
        return response;
      } catch (error) {
        if (error.response) await fx.capture({ ...intent, ...error.response });
        throw error;
      }
    },
    async read(channel = fx.channel, replica = other(ctx.am)) {
      return (await eg({ path: `/v1/channel-records/${enc(channel.record_id)}`, replica })).body;
    },
    async seedChannel({ source, actor = 'owner', routing, profile, provider } = {}) {
      const recordId = cryptoId(),
        body = {
          schema_version: 1,
          record_id: recordId,
          name: unique('EG20'),
          profile: profile ?? authority.profile,
          provider: provider ?? authority.provider,
          credentials: {
            agent_key: authority.agentKey,
            channel_key: authority.channelKey ?? randomBytes(32).toString('hex'),
            hmac_secrets: [{ key_id: 'eg20-hmac', secret: fx.hmac }],
          },
          sources: [source ?? fx.source],
          partition: { version: 1, key: 'conversation_and_sender' },
          reply: { auto_final: true, ack_after_ms: 2000, sinks: [{ type: 'source' }] },
        };
      channels.set(recordId, null);
      const response = await fx.call({
        method: 'POST',
        path: '/v1/channels',
        body,
        key: recordId,
        actor,
        allowed: [201],
      });
      let current = response.body;
      if (routing)
        current = (
          await fx.call({
            method: 'PUT',
            path: `/v1/channels/${enc(current.channel_id)}/routing`,
            body: { expected_version: current.version, routing },
          })
        ).body;
      ctx.assert(
        current.record_id === recordId &&
          current.channel_id &&
          current.definition?.creation?.ready === true &&
          current.version > 0,
        'eg20_seed_channel_receipt',
      );
      const peer = await fx.read(current);
      ctx.assert(
        peer.channel_id === current.channel_id && peer.version === current.version,
        'eg20_seed_channel_peer_read',
      );
      return current;
    },
    async emit(event) {
      const receipt = await fx.platform.emit({
        channelId: fx.channel.channel_id,
        recordId: fx.channel.record_id,
        replica: ctx.am,
        ...event,
      });
      requireInput(
        receipt?.eventId && receipt.input_sha256,
        'eg20_platform_input_receipt_required',
      );
      return receipt;
    },
    async fault(kind, args) {
      requireInput(
        typeof parent.faults?.[kind] === 'function',
        `eg20_fault_${kind.toLowerCase()}_required`,
      );
      const control = await parent.faults[kind]({ session, caseId: ctx.row.case_id, ...args });
      requireInput(typeof control?.restore === 'function', 'eg20_fault_cleanup_required');
      let closing;
      const restore = () => (closing ??= control.restore());
      restores.push(restore);
      ctx.defer('restore_eg20_fault', restore);
      return control;
    },
    async providerEvidence(value) {
      if (needs.agentProof) {
        const rows = await lease.agentRequests();
        requireInput(Array.isArray(rows) && rows.length > 0, 'eg20_actual_agent_requests_missing');
        for (const receipt of rows)
          check(
            validServingReceipt(receipt) &&
              receipt.scope === 'am' &&
              receipt.case_id === ctx.row.case_id &&
              receipt.owner_proof?.verdict === 'PASS' &&
              receipt.owner_proof.request_id === receipt.request_id &&
              receipt.owner_proof.agent_sha256 === receipt.agent_id_sha256,
            'eg20_agent_request_join_invalid',
          );
        check(
          rows.some((r) => r.owner_proof.kind === 'non_owner' && r.owner_proof.owner !== r.am),
          'eg20_non_owner_agent_request_required',
        );
        await ctx.observe('eg20_provider_am_requests', async () => ({
          ...value,
          requests: rows,
          schema: 'multi-am-browser-evidence-v1',
        }));
      }
    },
    async evidence() {
      await Promise.all([...pending]);
      if (observerErrors.length)
        throw new AggregateError(observerErrors, 'eg20_browser_receipt_capture_failed');
      const value = validateBrowserEvidence(await session.evidence());
      await fx.providerEvidence(value);
      await retainBrowserEvidence(ctx, value, 'eg20_browser_evidence');
      const values = await session.page
        .locator('input:not([type=password]),textarea')
        .evaluateAll((nodes) => nodes.map((n) => n.value));
      assertNoSecret(ctx, [await session.page.locator('body').innerText(), values], fx.secrets);
    },
    async cleanup() {
      return (cleanupPromise ??= (async () => {
        const failures = [...observerErrors];
        for (const restore of restores.reverse())
          try {
            await restore();
          } catch (error) {
            failures.push(error);
          }
        if (session) {
          try {
            if (browserStarted) {
              await closeAndCapture(ctx, session);
              await fx.providerEvidence(validateBrowserEvidence(await session.evidence()));
            } else await session.close();
          } catch (error) {
            failures.push(error);
          }
        }
        await Promise.allSettled([...pending]);
        if (listener) session.page.off('response', listener);
        failures.push(...observerErrors.filter((error) => !failures.includes(error)));
        for (const id of bots)
          try {
            const current = (
              await ctx.request({
                method: 'GET',
                path: `/api/v1/bots/${enc(id)}`,
                am: other(ctx.am),
              })
            ).body;
            check(current.owner === authority.ownerSubject, 'eg20_cleanup_bot_owner');
            if (current.state !== 'deleted')
              await fx.am({
                method: 'DELETE',
                path: `/api/v1/bots/${enc(id)}`,
                allowed: [202],
              });
            await poll(
              { ...ctx, assert: check },
              async () =>
                (
                  await ctx.request({
                    method: 'GET',
                    path: `/api/v1/bots/${enc(id)}`,
                    am: other(ctx.am),
                  })
                ).body,
              (current) => current.bot_id === id && current.state === 'deleted',
              'eg20_cleanup_bot_pending',
            );
          } catch (error) {
            failures.push(error);
          }
        for (const [recordId] of channels)
          try {
            const response = await eg({
              path: `/v1/channel-records/${enc(recordId)}`,
              replica: other(ctx.am),
              allowed: [200, 404],
            });
            if (response.status === 200) {
              const current = response.body;
              check(current.record_id === recordId, 'eg20_cleanup_record_identity');
              await fx.call({
                method: 'DELETE',
                path: `/v1/channels/${enc(current.channel_id)}?expected_version=${current.version}`,
                replica: ctx.am,
                allowed: [204],
              });
              await eg({
                path: `/v1/channel-records/${enc(recordId)}`,
                replica: other(ctx.am),
                allowed: [404],
              });
            }
          } catch (error) {
            failures.push(error);
          }
        for (const ref of connections)
          try {
            await fx.call({
              method: 'DELETE',
              path: `/v1/channel-connections/${enc(ref)}`,
              allowed: [204, 404],
            });
          } catch (error) {
            failures.push(error);
          }
        for (const id of profiles)
          try {
            await fx.am({
              method: 'DELETE',
              path: `/api/v1/agent-profiles/${enc(id)}`,
              allowed: [204],
            });
          } catch (error) {
            failures.push(error);
          }
        try {
          await lease.dispose();
        } catch (error) {
          failures.push(error);
        }
        // A 404 or elapsed deadline cannot settle a provider effect accepted late.
        // Independent parent custody/journals are the final cleanup barrier.
        try {
          const final = await lease.finality();
          await ctx.observe('eg20_fixture_finality', async () => final);
          check(
            final.unresolved === 0 &&
              final.in_flight === 0 &&
              final.resources_remaining === 0 &&
              final.private_custody_retained === 0,
            'eg20_cleanup_effects_unresolved',
          );
        } catch (error) {
          failures.push(error);
        }
        if (failures.length) throw new AggregateError(failures, 'eg20_cleanup_unresolved');
        await lease.release();
      })());
    },
    browserIntents,
    responses,
  };
  finishLease = fx.cleanup;
  fx.hmac = randomBytes(32).toString('base64');
  fx.secrets.push(fx.hmac);
  fx.recordId = cryptoId();
  fx.source = { type: 'custom', auth: { type: 'hmac', key_id: 'eg20-hmac' } };
  // The browser must have genuine EG session cookies and an independently
  // switchable EG backend. AM-only proxies cannot satisfy this boundary.
  session = await parent.openBrowser({
    actor: 'user',
    am: ctx.am,
    eg: ctx.am,
    caseId: ctx.row.case_id,
  });
  fx.session = session;
  for (const method of ['switchAM', 'switchEG', 'evidence', 'close'])
    requireInput(
      typeof session?.[method] === 'function',
      `eg20_browser_${method.toLowerCase()}_required`,
    );
  requireInput(session.page?.route && session.origin, 'eg20_real_playwright_required');
  interception = async (route) => {
    const request = route.request(),
      path = new URL(request.url()).pathname;
    if (
      ['GET', 'HEAD', 'OPTIONS'].includes(request.method()) ||
      !/^\/(?:api\/(event-gateway\/v1|(?:agent-manager\/)?v1)\/|console\/|connect\/submit$)/.test(
        path,
      )
    )
      return route.fallback();
    const intent = {
      scope: !path.startsWith('/api/') || path.startsWith('/api/event-gateway/') ? 'eg' : 'am',
      method: request.method(),
      path,
      key: request.headers()['idempotency-key'] ?? null,
      body: (request.headers()['content-type'] ?? '').includes('application/x-www-form-urlencoded')
        ? Object.fromEntries(new URLSearchParams(request.postData()))
        : request.postDataJSON(),
    };
    await lease.retainIntent(intent);
    browserIntents.push(intent);
    if (intent.body?.record_id) channels.set(intent.body.record_id, null);
    return route.fallback();
  };
  await session.page.route('**/*', interception);
  listener = (response) => {
    const path = new URL(response.url()).pathname;
    if (
      ['GET', 'HEAD', 'OPTIONS'].includes(response.request().method()) ||
      !path.startsWith('/api/')
    )
      return;
    const job = (async () => {
      const body = response.status() === 204 ? null : await response.json();
      await fx.capture({
        path,
        status: response.status(),
        body,
        kind: path.includes('/agent-profiles') ? 'profile' : undefined,
      });
    })();
    pending.add(job);
    job.catch((error) => observerErrors.push(error)).finally(() => pending.delete(job));
  };
  session.page.on('response', listener);
  await session.page.goto(new URL('/event-gateway', session.origin).href);
  const sessionEvidence = await session.evidence();
  requireInput(
    sessionEvidence.authentication === 'browser_session',
    'eg20_real_browser_session_required',
  );
  browserStarted = true;
  if (
    needs.channel &&
    ![
      'create',
      'recover',
      'connections',
      'connection-create',
      'connection-cancel',
      'capabilities',
    ].includes(name)
  ) {
    const source = ['outbound-prepare', 'outbound-send', 'console-sources'].includes(name)
      ? requireInput(authority.relaySource, 'eg20_saved_relay_route_required')
      : fx.source;
    fx.channel = await fx.seedChannel({ source });
  }
  if (['connection-create', 'connection-cancel', 'connections', 'father-actions'].includes(name)) {
    requireInput(
      typeof lease.platform?.admitSetupEvent === 'function',
      'eg20_verified_originating_event_required',
    );
    fx.originEvent = await lease.platform.admitSetupEvent({ owner: authority.ownerSubject });
    requireInput(
      fx.originEvent.record_id && fx.originEvent.event_ref && fx.originEvent.activity,
      'eg20_attested_event_receipt_required',
    );
  }
  return fx;
}
const cryptoId = () => unique('').slice(1);
export const setups = Object.fromEntries(
  names.map((name) => [`eg20-${name}`, (ctx, primitives) => prepare(ctx, primitives, name)]),
);
