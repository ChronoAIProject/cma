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
  steps: [],
  revision_content: {},
  effective: {},
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
  revision_digest: 'revision-digest',
  public_summary: 'Fix one bounded issue',
  repository_id: 42,
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

  it('rejects malformed Launches and internal fields at any nesting depth', () => {
    expect(() => decodeLaunch({ ...LAUNCH, steps: {} })).toThrow(ProtocolError);
    expect(() =>
      decodeLaunch({
        ...LAUNCH,
        revision_content: { setup: { backing_key_id: 'must-not-cross-layer-one' } },
      }),
    ).toThrow('leaked backing_key_id');
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
      [{ launch_id: 'launch_fixture', next_cursor: 0, events: [] }, 200],
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
