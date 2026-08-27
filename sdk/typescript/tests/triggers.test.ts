import { readFileSync } from 'node:fs';
import { describe, expect, it, vi } from 'vitest';
import { Cma } from '../src/client.js';
import { resolveConfig, type ResolvedConfig } from '../src/config.js';
import { ProtocolError } from '../src/errors.js';
import {
  beginTriggerConnection,
  cancelLaunch,
  confirmPullRequest,
  createNewLaunch,
  createOrResumeLaunch,
  createTrigger,
  decodeLaunch,
  preflightTriggerConnection,
  publishLaunchBranch,
  publishTrigger,
  readLaunch,
  readLaunchDiff,
  readLaunchProgress,
  readTriggerPreview,
  recoverInitialPrompt,
  reconnectTriggerConnection,
  revokeTriggerConnection,
  resolveTriggerRepository,
  requestLaunchDiff,
  retireTriggerRevision,
  reviewPullRequest,
  selectLaunchConnection,
  startLaunch,
  updateLaunchOverrides,
  updateTrigger,
  type TriggerContentDraft,
} from '../src/triggers.js';

/** @vitest-environment node */

const TRIGGER_CONTENT = {} as TriggerContentDraft;
const LAUNCH = {
  id: 'launch_fixture',
  trigger_public_id: 'public_fixture',
  revision_digest: 'revision-digest',
  state: 'draft',
  revision_number: 1,
  draft_expires_at: '2026-08-28T00:00:00Z',
  steps: [],
  revision_content: {},
  effective: { permission: 'autonomous' },
  pricing: null,
  runtime_admission: null,
  review_quota: null,
  start_admission: null,
};
const TRIGGER_DRAFT = {
  id: 'trigger_fixture',
  selected_service_id: 'service_fixture',
  version: 1,
  content: {},
};
const TRIGGER_REVISION = {
  id: 'revision_fixture',
  public_id: 'public_fixture',
  digest: 'revision-digest',
  availability: 'published',
  content: {},
};
const CONNECTION = { service_id: 'service_fixture', state_version: 1, state: 'active' };
const AUTHORIZATION = {
  service_id: 'service_fixture',
  authorization_url: 'https://identity.example.test/authorize',
  attempt_nonce: 'attempt-fixture',
};
const PR_DRAFT = {
  launch_id: 'launch_fixture',
  operation: 'open',
  intent_digest: 'intent-digest',
  expected_head_revision: 'head-revision',
};
const PREVIEW = {
  public_id: 'public_fixture',
  availability: 'published',
  revision_digest: 'revision-digest',
  public_summary: 'Fix one bounded issue',
  repository_id: 42,
  visibility: 'public',
  permission_ceiling: 'autonomous',
  required_capabilities: ['conversation'],
  billing_mode: 'contributor',
};

function fixture(name: string): unknown {
  return JSON.parse(readFileSync(new URL(`fixtures/${name}`, import.meta.url), 'utf8')) as unknown;
}

function responder(body: unknown, status: number) {
  const fetch = vi.fn((...args: [string | URL | Request, RequestInit?]) => {
    void args;
    return Promise.resolve(
      new Response(JSON.stringify(body), {
        status,
        headers: { 'content-type': 'application/json' },
      }),
    );
  });
  return { fetch: fetch as unknown as typeof globalThis.fetch, calls: fetch };
}

describe('Trigger authentication boundaries', () => {
  it('always reads a public preview anonymously, even from an authenticated client', async () => {
    const credential = vi.fn(() => 'private-bearer');
    const { fetch, calls } = responder(PREVIEW, 200);

    await expect(
      readTriggerPreview(resolveConfig({ credential, cookies: true, fetch }), 'public_fixture'),
    ).resolves.toMatchObject(PREVIEW);

    expect(credential).not.toHaveBeenCalled();
    const [, init] = calls.mock.calls[0] as unknown as [string, RequestInit];
    expect(new Headers(init.headers).get('authorization')).toBeNull();
    expect(init.credentials).toBe('omit');
  });
});

describe('Trigger command idempotency', () => {
  it('sends the caller key exactly on every state-changing command', async () => {
    const cases: readonly {
      readonly name: string;
      readonly status: number;
      readonly response: unknown;
      readonly invoke: (config: ResolvedConfig) => Promise<unknown>;
    }[] = [
      {
        name: 'create Trigger',
        status: 201,
        response: TRIGGER_DRAFT,
        invoke: (config) =>
          createTrigger(config, 'service_fixture', TRIGGER_CONTENT, {
            idempotencyKey: 'exact-key',
          }),
      },
      {
        name: 'update Trigger',
        status: 200,
        response: TRIGGER_DRAFT,
        invoke: (config) =>
          updateTrigger(config, 'trigger_fixture', 1, 'service_fixture', TRIGGER_CONTENT, {
            idempotencyKey: 'exact-key',
          }),
      },
      {
        name: 'publish Trigger',
        status: 201,
        response: TRIGGER_REVISION,
        invoke: (config) =>
          publishTrigger(config, 'trigger_fixture', 1, 'service_fixture', {
            idempotencyKey: 'exact-key',
          }),
      },
      {
        name: 'retire Trigger revision',
        status: 200,
        response: TRIGGER_REVISION,
        invoke: (config) =>
          retireTriggerRevision(config, 'revision_fixture', { idempotencyKey: 'exact-key' }),
      },
      {
        name: 'begin connection',
        status: 200,
        response: AUTHORIZATION,
        invoke: (config) =>
          beginTriggerConnection(config, 'intent_fixture', { idempotencyKey: 'exact-key' }),
      },
      {
        name: 'reconnect',
        status: 200,
        response: AUTHORIZATION,
        invoke: (config) =>
          reconnectTriggerConnection(config, 'service_fixture', { idempotencyKey: 'exact-key' }),
      },
      {
        name: 'revoke connection',
        status: 200,
        response: { ...CONNECTION, state_version: 2, state: 'inactive' },
        invoke: (config) =>
          revokeTriggerConnection(config, 'service_fixture', { idempotencyKey: 'exact-key' }),
      },
      {
        name: 'create or resume Launch',
        status: 201,
        response: LAUNCH,
        invoke: (config) =>
          createOrResumeLaunch(config, 'public_fixture', { idempotencyKey: 'exact-key' }),
      },
      {
        name: 'create new Launch',
        status: 201,
        response: LAUNCH,
        invoke: (config) =>
          createNewLaunch(config, 'public_fixture', { idempotencyKey: 'exact-key' }),
      },
      {
        name: 'update Launch overrides',
        status: 200,
        response: LAUNCH,
        invoke: (config) =>
          updateLaunchOverrides(
            config,
            'launch_fixture',
            1,
            { task_inputs: {}, optional_checks: [] },
            { idempotencyKey: 'exact-key' },
          ),
      },
      {
        name: 'select Launch connection',
        status: 200,
        response: LAUNCH,
        invoke: (config) =>
          selectLaunchConnection(config, 'launch_fixture', 1, 'service_fixture', {
            idempotencyKey: 'exact-key',
          }),
      },
      {
        name: 'start Launch',
        status: 202,
        response: LAUNCH,
        invoke: (config) =>
          startLaunch(config, 'launch_fixture', 1, 'snapshot-digest', false, {
            idempotencyKey: 'exact-key',
          }),
      },
      {
        name: 'request Launch diff',
        status: 202,
        response: LAUNCH,
        invoke: (config) =>
          requestLaunchDiff(config, 'launch_fixture', 1, 'snapshot-digest', {
            idempotencyKey: 'exact-key',
          }),
      },
      {
        name: 'publish Launch branch',
        status: 202,
        response: LAUNCH,
        invoke: (config) =>
          publishLaunchBranch(config, 'launch_fixture', 1, 'snapshot-digest', {
            idempotencyKey: 'exact-key',
          }),
      },
      {
        name: 'confirm pull request Open',
        status: 202,
        response: LAUNCH,
        invoke: (config) =>
          confirmPullRequest(
            config,
            'launch_fixture',
            'open',
            1,
            'intent-digest',
            {},
            {
              idempotencyKey: 'exact-key',
            },
          ),
      },
      {
        name: 'confirm pull request Update',
        status: 202,
        response: LAUNCH,
        invoke: (config) =>
          confirmPullRequest(
            config,
            'launch_fixture',
            'update',
            1,
            'intent-digest',
            {},
            {
              idempotencyKey: 'exact-key',
            },
          ),
      },
      {
        name: 'cancel Launch',
        status: 200,
        response: LAUNCH,
        invoke: (config) =>
          cancelLaunch(config, 'launch_fixture', 1, 'snapshot-digest', {
            idempotencyKey: 'exact-key',
          }),
      },
      {
        name: 'recover initial prompt',
        status: 202,
        response: LAUNCH,
        invoke: (config) =>
          recoverInitialPrompt(config, 'launch_fixture', 12, 'retained-payload-hash', {
            idempotencyKey: 'exact-key',
          }),
      },
    ];

    for (const entry of cases) {
      const { fetch, calls } = responder(entry.response, entry.status);
      await entry.invoke(resolveConfig({ fetch }));
      const [, init] = calls.mock.calls[0] as unknown as [string, RequestInit];
      expect(new Headers(init.headers).get('idempotency-key'), entry.name).toBe('exact-key');
    }
  });

  it('binds prompt recovery to the retained payload and duplicate-risk acknowledgement', async () => {
    const { fetch, calls } = responder(LAUNCH, 202);
    const client = new Cma({ fetch });

    await client.recoverInitialPrompt('launch_fixture', 12, 'retained-payload-hash', {
      idempotencyKey: 'recovery-command',
    });

    const [url, init] = calls.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe('/api/v1/launches/launch_fixture/recover-initial-prompt');
    expect(init.method).toBe('POST');
    expect(new Headers(init.headers).get('idempotency-key')).toBe('recovery-command');
    expect(JSON.parse(String(init.body))).toEqual({
      expected_revision: 12,
      snapshot_digest: 'retained-payload-hash',
      acknowledge_duplicate_execution_risk: true,
    });
  });

  it('uses an authenticated GET without an idempotency key for connection preflight', async () => {
    const { fetch, calls } = responder(CONNECTION, 200);
    await preflightTriggerConnection(resolveConfig({ fetch }), 'service_fixture');

    const [, init] = calls.mock.calls[0] as unknown as [string, RequestInit];
    expect(init.method).toBe('GET');
    expect(new Headers(init.headers).has('idempotency-key')).toBe(false);
  });

  it('revokes the exact service with DELETE and an empty frozen command body', async () => {
    const revoked = { ...CONNECTION, state_version: 2, state: 'inactive' };
    const { fetch, calls } = responder(revoked, 200);

    await expect(
      revokeTriggerConnection(resolveConfig({ fetch }), 'service_fixture', {
        idempotencyKey: 'revoke-exact-key',
      }),
    ).resolves.toMatchObject(revoked);

    const [url, init] = calls.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe('/api/v1/trigger-connections/service_fixture');
    expect(init.method).toBe('DELETE');
    expect(new Headers(init.headers).get('idempotency-key')).toBe('revoke-exact-key');
    expect(JSON.parse(String(init.body))).toEqual({});
  });

  it('rejects revocation state for a different service', async () => {
    const { fetch } = responder(
      { ...CONNECTION, service_id: 'service_other', state_version: 2, state: 'inactive' },
      200,
    );

    await expect(
      revokeTriggerConnection(resolveConfig({ fetch }), 'service_fixture', {
        idempotencyKey: 'revoke-exact-key',
      }),
    ).rejects.toThrow('different GitHub connection');
  });

  it('resolves a repository through the exact authenticated connection without a command key', async () => {
    const credential = vi.fn(() => 'private-bearer');
    const { fetch, calls } = responder(fixture('repository_selection.json'), 200);

    await expect(
      resolveTriggerRepository(
        resolveConfig({ credential, fetch }),
        'service_fixture',
        'upstream-owner',
        '.github',
      ),
    ).resolves.toMatchObject({ source_sha: 'a'.repeat(40) });

    expect(credential).toHaveBeenCalledOnce();
    const [url, init] = calls.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe(
      '/api/v1/trigger-connections/service_fixture/repositories/resolve?owner=upstream-owner&name=.github',
    );
    expect(init.method).toBe('GET');
    expect(new Headers(init.headers).get('authorization')).toBe('Bearer private-bearer');
    expect(new Headers(init.headers).has('idempotency-key')).toBe(false);
  });

  it('rejects non-canonical repository locators before transport', async () => {
    const fetch = vi.fn();
    const config = resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch });

    for (const [owner, name] of [
      ['', 'project'],
      ['upstream/owner', 'project'],
      ['upstream-owner', '..'],
      ['upstream-owner', 'project?private=true'],
      [' upstream-owner', 'project'],
    ] as const) {
      await expect(
        resolveTriggerRepository(config, 'service_fixture', owner, name),
      ).rejects.toThrow('canonical owner or repository name');
    }
    expect(fetch).not.toHaveBeenCalled();
  });

  it('keeps pull-request review side-effect-free and puts a key only on confirmation', async () => {
    const fetch = vi
      .fn()
      .mockResolvedValueOnce(
        new Response(JSON.stringify(PR_DRAFT), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        }),
      )
      .mockResolvedValueOnce(
        new Response(JSON.stringify(LAUNCH), {
          status: 202,
          headers: { 'content-type': 'application/json' },
        }),
      );
    const config = resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch });

    await reviewPullRequest(config, 'launch_fixture', 'open');
    await confirmPullRequest(
      config,
      'launch_fixture',
      'open',
      1,
      'intent-digest',
      {},
      {
        idempotencyKey: 'confirmation-key',
      },
    );

    const reviewInit = fetch.mock.calls[0]?.[1] as RequestInit;
    const confirmInit = fetch.mock.calls[1]?.[1] as RequestInit;
    expect(new Headers(reviewInit.headers).has('idempotency-key')).toBe(false);
    expect(new Headers(confirmInit.headers).get('idempotency-key')).toBe('confirmation-key');
  });
});

describe('Trigger response validation', () => {
  it('decodes every server-owned Trigger response fixture through the public transport', async () => {
    const previewTransport = responder(fixture('trigger_preview.json'), 200);
    const preview = await readTriggerPreview(
      resolveConfig({ fetch: previewTransport.fetch }),
      '33333333-3333-4333-8333-333333333333',
    );
    expect(preview.repository_id).toBe(10);

    const repositoryTransport = responder(fixture('repository_selection.json'), 200);
    const repository = await resolveTriggerRepository(
      resolveConfig({ fetch: repositoryTransport.fetch }),
      'service_fixture',
      'upstream-owner',
      'project',
    );
    expect(repository.repository.repository_node_id).toBe('R_upstream');
    expect(repository.source_sha).toBe('a'.repeat(40));

    const launchTransport = responder(fixture('launch.json'), 200);
    const launch = await readLaunch(
      resolveConfig({ fetch: launchTransport.fetch }),
      '44444444-4444-4444-8444-444444444444',
    );
    expect(launch.steps).toHaveLength(15);

    const progressTransport = responder(fixture('launch_progress.json'), 200);
    const progress = await readLaunchProgress(
      resolveConfig({ fetch: progressTransport.fetch }),
      launch.id,
    );
    expect(progress.next_cursor).toBe(3);
    expect(progress.events[0]?.progress.state).toBe('draft');
    const created = progress.events[0]?.kind;
    expect(created?.type).toBe('created');
    if (created?.type === 'created') expect(created.steps).toHaveLength(15);

    const diffTransport = responder(fixture('diff_review.json'), 200);
    const diff = await readLaunchDiff(resolveConfig({ fetch: diffTransport.fetch }), launch.id);
    expect(diff.artifact?.files).toHaveLength(1);

    const pullRequestTransport = responder(fixture('pull_request_draft.json'), 200);
    const pullRequest = await reviewPullRequest(
      resolveConfig({ fetch: pullRequestTransport.fetch }),
      launch.id,
      'open',
    );
    expect(pullRequest.operation).toBe('open');
  });

  it('accepts omitted quota ceilings and rejects malformed present ceilings', () => {
    const raw = fixture('launch.json') as Record<string, unknown>;
    const quota = { ...(raw.review_quota as Record<string, unknown>) };
    delete quota.live_session_limit;
    delete quota.free_cpu_millis;
    delete quota.free_memory_mib;

    const launch = decodeLaunch({ ...raw, review_quota: quota });
    expect(launch.review_quota?.live_session_limit).toBeUndefined();
    expect(launch.review_quota?.free_cpu_millis).toBeUndefined();
    expect(launch.review_quota?.free_memory_mib).toBeUndefined();

    for (const field of ['live_session_limit', 'free_cpu_millis', 'free_memory_mib']) {
      expect(() => decodeLaunch({ ...raw, review_quota: { ...quota, [field]: null } })).toThrow(
        `Launch review quota carried an invalid ${field}`,
      );
    }
  });

  it('rejects malformed Launches and internal fields at any nesting depth', () => {
    expect(() => decodeLaunch({ ...LAUNCH, steps: {} })).toThrow(ProtocolError);
    for (const field of [
      'idempotency_key_hash',
      'intent_key',
      'provider_request_id',
      'object_reference',
      'baseline_object_reference',
      'backing_key_id',
      'worker_fencing_token',
      'fencing_counter',
      'subject_token_generation',
      'session_credential_generation',
      'session_credential_authority',
      'custody_generation',
      'provider_key_id',
      'provider_state_version',
      'grant_fingerprint',
      'checkpoint_object',
      'diff_artifact_object',
      'prompt_origin_key',
      'prompt_payload_hash',
    ]) {
      expect(() =>
        decodeLaunch({
          ...LAUNCH,
          revision_content: { setup: { [field]: 'must-not-cross-layer-one' } },
        }),
      ).toThrow(`leaked ${field}`);
    }
    const delivery = {
      launch_id: 'launch_fixture',
      session_id: 'session_fixture',
      revision_digest: 'revision-digest',
      origin_key: 'cma-trigger:launch_fixture:initial',
      payload_hash: 'a'.repeat(64),
      state: 'recovery_required',
      attempt: 1,
      recovery_generation: 0,
      dispatch_fence_sequence: 1,
      queued_at: '2026-08-27T00:00:00Z',
      updated_at: '2026-08-27T00:01:00Z',
    };
    expect(() =>
      decodeLaunch({ ...LAUNCH, delivery: { ...delivery, state: 'delivered' } }),
    ).toThrow('Prompt delivery carried an invalid state');
    expect(() =>
      decodeLaunch({ ...LAUNCH, delivery: { ...delivery, recovery_generation: -1 } }),
    ).toThrow('Prompt delivery carried an invalid recovery_generation');
  });

  it('rejects unknown values for every representative closed Launch vocabulary', () => {
    const cases: Record<string, unknown>[] = [];

    const state = fixture('launch.json') as Record<string, unknown>;
    state.state = 'future_state';
    cases.push(state);

    const permission = fixture('launch.json') as Record<string, unknown>;
    (permission.effective as Record<string, unknown>).permission = 'unrestricted';
    cases.push(permission);

    const step = fixture('launch.json') as Record<string, unknown>;
    ((step.steps as Record<string, unknown>[])[0] as Record<string, unknown>).key = 'future_step';
    cases.push(step);

    const failure = fixture('launch.json') as Record<string, unknown>;
    failure.failure = {
      class: 'future_failure',
      code: 'future',
      correlation_id: 'correlation',
    };
    cases.push(failure);

    const connection = fixture('launch.json') as Record<string, unknown>;
    (connection.connection as Record<string, unknown>).state = 'future_connection';
    cases.push(connection);

    const capability = fixture('launch.json') as Record<string, unknown>;
    capability.reach = { state: 'refresh_required' };
    cases.push(capability);

    const runtime = fixture('launch.json') as Record<string, unknown>;
    (runtime.runtime_admission as Record<string, unknown>).supported_capabilities = [
      'future_capability',
    ];
    cases.push(runtime);

    const kernel = fixture('launch.json') as Record<string, unknown>;
    (kernel.runtime_admission as Record<string, unknown>).kernel = 'future_kernel';
    cases.push(kernel);

    const diff = fixture('launch.json') as Record<string, unknown>;
    const review = fixture('diff_review.json') as Record<string, unknown>;
    diff.diff = review.artifact;
    (diff.diff as Record<string, unknown>).validation = 'future_validation';
    cases.push(diff);

    for (const raw of cases) expect(() => decodeLaunch(raw)).toThrow(ProtocolError);
  });

  it('normalizes identical duplicates and rejects ambiguous Launch progress', async () => {
    const identical = fixture('launch_progress.json') as Record<string, unknown>;
    const launchId = String(identical.launch_id);
    const identicalEvents = identical.events as Record<string, unknown>[];
    identicalEvents.splice(1, 0, structuredClone(identicalEvents[0] as Record<string, unknown>));
    const acceptedTransport = responder(identical, 200);
    const accepted = await readLaunchProgress(
      resolveConfig({ fetch: acceptedTransport.fetch }),
      launchId,
    );
    expect(accepted.events).toHaveLength(3);
    expect(accepted.next_cursor).toBe(3);

    const conflicting = fixture('launch_progress.json') as Record<string, unknown>;
    const conflictingEvents = conflicting.events as Record<string, unknown>[];
    const duplicate = structuredClone(conflictingEvents[0] as Record<string, unknown>);
    duplicate.correlation_id = 'conflicting-correlation';
    conflictingEvents.splice(1, 0, duplicate);

    const gap = fixture('launch_progress.json') as Record<string, unknown>;
    (gap.events as Record<string, unknown>[]).splice(1, 1);

    const mixed = fixture('launch_progress.json') as Record<string, unknown>;
    ((mixed.events as Record<string, unknown>[])[1] as Record<string, unknown>).launch_id =
      'launch_other';

    const badCursor = fixture('launch_progress.json') as Record<string, unknown>;
    badCursor.next_cursor = 4;

    const wrongPage = fixture('launch_progress.json') as Record<string, unknown>;
    wrongPage.launch_id = 'launch_other';

    for (const [raw, message] of [
      [conflicting, 'conflicting duplicate'],
      [gap, 'sequence gap'],
      [mixed, 'belonged to launch_other'],
      [badCursor, 'next_cursor 4'],
      [wrongPage, 'response belonged to launch_other'],
    ] as const) {
      const transport = responder(raw, 200);
      await expect(
        readLaunchProgress(resolveConfig({ fetch: transport.fetch }), launchId),
      ).rejects.toThrow(message);
    }

    const fetch = vi.fn();
    await expect(
      readLaunchProgress(
        resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch }),
        launchId,
        0,
        501,
      ),
    ).rejects.toThrow('1 through 500');
    expect(fetch).not.toHaveBeenCalled();
  });

  it('rejects malformed, unordered, and internally leaky progress', async () => {
    for (const progress of [
      { launch_id: 'launch_fixture', next_cursor: 2, events: {} },
      {
        launch_id: 'launch_fixture',
        next_cursor: 2,
        events: [{ sequence: 2 }, { sequence: 1 }],
      },
      {
        launch_id: 'launch_fixture',
        next_cursor: 1,
        events: [{ sequence: 1, detail: { provider_request_id: 'private' } }],
      },
    ]) {
      const { fetch } = responder(progress, 200);
      await expect(
        readLaunchProgress(resolveConfig({ fetch }), 'launch_fixture'),
      ).rejects.toBeInstanceOf(ProtocolError);
    }
  });
});

describe('configured Trigger client paths', () => {
  it('uses the canonical public, progress, review, confirmation, and artifact routes', async () => {
    const responses = [
      [PREVIEW, 200],
      [LAUNCH, 201],
      [{ launch_id: 'launch_fixture', next_cursor: 7, events: [] }, 200],
      [PR_DRAFT, 200],
      [LAUNCH, 202],
      [new Uint8Array([1, 2, 3]), 200],
    ] as const;
    const fetch = vi.fn((...args: [string | URL | Request, RequestInit?]) => {
      void args;
      const next = responses[fetch.mock.calls.length - 1];
      if (next === undefined) throw new Error('unexpected Trigger request');
      const [body, status] = next;
      return Promise.resolve(
        body instanceof Uint8Array
          ? new Response(body, { status })
          : new Response(JSON.stringify(body), {
              status,
              headers: { 'content-type': 'application/json' },
            }),
      );
    });
    const cma = new Cma({ fetch: fetch as unknown as typeof globalThis.fetch });

    await cma.readTriggerPreview('public_fixture');
    await cma.createOrResumeLaunch('public_fixture', { idempotencyKey: 'create-key' });
    await cma.readLaunchProgress('launch_fixture', 7, 25);
    await cma.reviewPullRequest('launch_fixture', 'open');
    await cma.confirmPullRequest(
      'launch_fixture',
      'open',
      1,
      'intent-digest',
      {},
      {
        idempotencyKey: 'confirm-key',
      },
    );
    await cma.downloadDiffArtifact('launch_fixture', 'artifact_fixture');

    expect(fetch.mock.calls.map(([url]) => String(url))).toEqual([
      '/api/v1/triggers/public/public_fixture',
      '/api/v1/triggers/public/public_fixture/launches',
      '/api/v1/launches/launch_fixture/events?after=7&limit=25',
      '/api/v1/launches/launch_fixture/pull-request/review',
      '/api/v1/launches/launch_fixture/pull-request/open',
      '/api/v1/launches/launch_fixture/diff-artifacts/artifact_fixture/download',
    ]);
  });
});
