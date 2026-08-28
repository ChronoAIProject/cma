import { readFileSync } from 'node:fs';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type {
  TriggerConnection,
  TriggerContentDraft,
  TriggerDraft,
  TriggerRepositorySelection,
  TriggerRevision,
} from '@chronoai/cma-sdk';
import { ApiError, TransportError } from '../api/client';
import { BrowserCommandLedger } from '../trigger/useIdempotentCommand';
import { PublisherSurface } from './PublisherSurface';
import { applyRepository, editorFromDraft, newPublisherEditor } from './model';

const api = vi.hoisted(() => ({
  beginTriggerConnection: vi.fn(),
  createTrigger: vi.fn(),
  listTriggerConnections: vi.fn(),
  listTriggerRevisions: vi.fn(),
  listTriggers: vi.fn(),
  preflightTriggerConnection: vi.fn(),
  publishTrigger: vi.fn(),
  readTrigger: vi.fn(),
  reconnectTriggerConnection: vi.fn(),
  revokeTriggerConnection: vi.fn(),
  resolveTriggerRepository: vi.fn(),
  retireTriggerRevision: vi.fn(),
  updateTrigger: vi.fn(),
}));

const flow = vi.hoisted(() => ({ authorizeTriggerConnection: vi.fn() }));

vi.mock('../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../api/client')>()),
  ...api,
}));

vi.mock('../trigger/connectionFlow', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../trigger/connectionFlow')>()),
  authorizeTriggerConnection: flow.authorizeTriggerConnection,
}));

const REPOSITORY: TriggerRepositorySelection = {
  repository: {
    repository_id: 10,
    repository_node_id: 'R_upstream',
    owner_id: 20,
    owner_node_id: 'U_owner',
    canonical_owner: 'upstream-owner',
    canonical_name: 'project',
    visibility: 'public',
    allow_forking: true,
    default_branch: 'main',
    selected_ref: 'main',
  },
  source_sha: 'a'.repeat(40),
};

const ACTIVE: TriggerConnection = {
  service_id: 'service-exact',
  state_version: 3,
  state: 'active',
  account_id: 42,
  account_node_id: 'U_contributor',
  account_login: 'contributor',
  avatar_url: null,
  granted_scopes: ['public_repo'],
  consent_observed_at: '2026-08-27T00:00:00Z',
};

const INACTIVE: TriggerConnection = {
  ...ACTIVE,
  state_version: ACTIVE.state_version + 1,
  state: 'inactive',
};

const HANDOFF = {
  service_id: ACTIVE.service_id,
  authorization_url:
    'https://github.com/login/oauth/authorize?client_id=fixture&state=12345678-1234-4234-8234-123456789abc',
  attempt_nonce: '12345678-1234-4234-8234-123456789abc',
};

function controlled<T>() {
  let resolve!: (value: T) => void;
  let reject!: (cause: unknown) => void;
  const promise = new Promise<T>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}

function revisionContent(content: TriggerContentDraft, triggerId: string) {
  return {
    schema_version: content.schema_version,
    trigger_id: triggerId,
    revision_number: 2,
    publisher_subject_id: 'publisher',
    publisher_display_identity: content.publisher_display_identity,
    upstream: {
      repository: REPOSITORY.repository,
      source_mode: content.repository.source_mode,
      observed_source_sha: REPOSITORY.source_sha,
      contribution_base_ref: content.repository.contribution_base_ref,
    },
    task: content.task,
    session: content.session,
    repository_access: content.repository_access,
    setup: content.setup,
    contribution: content.contribution,
    commercial_policy: content.commercial_policy,
    override_policy: content.override_policy,
    public_projection: content.public_projection,
  } as const;
}

function draft(
  summary = 'Fix one bounded issue',
  version = 3,
  selectedServiceId = ACTIVE.service_id,
): TriggerDraft {
  const base = applyRepository(
    { ...newPublisherEditor('Chrono Maintainer'), selectedServiceId },
    REPOSITORY,
  );
  const content = { ...base.content, task: { ...base.content.task, public_summary: summary } };
  return {
    id: '11111111-1111-4111-8111-111111111111',
    selected_service_id: selectedServiceId,
    version,
    next_revision_number: 2,
    content: revisionContent(content, '11111111-1111-4111-8111-111111111111'),
    created_at: '2026-08-27T00:00:00Z',
    updated_at: '2026-08-27T01:00:00Z',
  };
}

function savedFrom(content: TriggerContentDraft, version = 4): TriggerDraft {
  return {
    ...draft(content.task.public_summary, version),
    content: revisionContent(content, '11111111-1111-4111-8111-111111111111'),
  };
}

function revision(availability: 'published' | 'retired' = 'published'): TriggerRevision {
  const current = draft();
  return {
    id: '22222222-2222-4222-8222-222222222222',
    public_id: '33333333-3333-4333-8333-333333333333',
    digest: 'd'.repeat(64),
    availability,
    content: { ...current.content, revision_number: 1 },
    authority: {
      service_id: ACTIVE.service_id,
      service_state_version: ACTIVE.state_version,
      account_id: 42,
      account_login: 'upstream-owner',
      role: 'owner',
      verified_at: '2026-08-27T02:00:00Z',
    },
    published_at: '2026-08-27T02:00:00Z',
    retired_at: availability === 'retired' ? '2026-08-27T03:00:00Z' : null,
  };
}

function commandStorageText(): string {
  return Array.from({ length: window.localStorage.length }, (_, index) => {
    const key = window.localStorage.key(index);
    return key === null ? '' : (window.localStorage.getItem(key) ?? '');
  }).join('\n');
}

beforeEach(() => {
  vi.clearAllMocks();
  api.listTriggers.mockResolvedValue([draft()]);
  api.listTriggerRevisions.mockResolvedValue([]);
  api.listTriggerConnections.mockResolvedValue([ACTIVE]);
  api.preflightTriggerConnection.mockResolvedValue(ACTIVE);
  api.resolveTriggerRepository.mockResolvedValue(REPOSITORY);
  api.readTrigger.mockResolvedValue(draft());
  api.createTrigger.mockImplementation(async (_serviceId: string, content: TriggerContentDraft) =>
    savedFrom(content, 1),
  );
  api.updateTrigger.mockImplementation(
    async (_id: string, _version: number, _serviceId: string, content: TriggerContentDraft) =>
      savedFrom(content),
  );
  api.publishTrigger.mockResolvedValue(revision());
  api.retireTriggerRevision.mockResolvedValue(revision('retired'));
  api.revokeTriggerConnection.mockResolvedValue(INACTIVE);
  flow.authorizeTriggerConnection.mockResolvedValue(ACTIVE);
});

describe('publisher loading and empty states', () => {
  it('shows bounded skeletons while the initial reads are pending', () => {
    api.listTriggers.mockReturnValue(new Promise(() => {}));
    render(<PublisherSurface ownerId="test-owner" displayIdentity="Chrono Maintainer" />);
    expect(screen.getByLabelText('GitHub Triggers')).toHaveAttribute('aria-busy', 'true');
    expect(document.querySelectorAll('.publisher__loading .skeleton')).toHaveLength(3);
  });

  it('offers a retry after an initial read fails', async () => {
    api.listTriggers.mockRejectedValueOnce(new Error('draft catalogue unavailable'));
    render(<PublisherSurface ownerId="test-owner" displayIdentity="Chrono Maintainer" />);

    expect(await screen.findByRole('alert')).toHaveTextContent('draft catalogue unavailable');
    await userEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(
      await screen.findByRole('heading', { name: 'Fix one bounded issue' }),
    ).toBeInTheDocument();
  });

  it('draws the new editor and a real empty-state sentence with no drafts', async () => {
    api.listTriggers.mockResolvedValue([]);
    render(<PublisherSurface ownerId="test-owner" displayIdentity="Chrono Maintainer" />);
    expect(await screen.findByText('No Trigger drafts yet.')).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Fix one bounded issue' })).toBeInTheDocument();
  });

  it('recovers a retained command whose draft is beyond the first catalogue page', async () => {
    const drafts = Array.from({ length: 60 }, (_, index) => {
      const id = `00000000-0000-4000-8000-${String(index + 1).padStart(12, '0')}`;
      const item = draft(`Catalogue draft ${String(index + 1)}`);
      return { ...item, id, content: { ...item.content, trigger_id: id } };
    });
    const target = drafts[55]!;
    api.listTriggers.mockResolvedValue(drafts);
    api.listTriggerRevisions.mockResolvedValue(
      Array.from({ length: 60 }, (_, index) => ({
        ...revision(),
        id: `10000000-0000-4000-8000-${String(index + 1).padStart(12, '0')}`,
        public_id: `20000000-0000-4000-8000-${String(index + 1).padStart(12, '0')}`,
        content: { ...revision().content, trigger_id: drafts[index]!.id },
      })),
    );
    const editor = editorFromDraft(target);
    const ledger = new BrowserCommandLedger({ ownerId: 'test-owner' });
    await expect(
      ledger.execute(
        {
          slot: `trigger:update:${target.id}`,
          resource: `trigger:${target.id}`,
          action: 'update-trigger',
          request: {
            draftId: target.id,
            expectedVersion: target.version,
            serviceId: target.selected_service_id,
            content: editor.content,
          },
          retained: {
            draftId: target.id,
            expectedVersion: target.version,
            serviceId: target.selected_service_id,
          },
        },
        'Trigger draft save',
        async () => Promise.reject(new TransportError('response lost')),
      ),
    ).rejects.toBeInstanceOf(TransportError);

    render(<PublisherSurface ownerId="test-owner" displayIdentity="Chrono Maintainer" />);

    expect(await screen.findByText(/did not retain the private draft body/i)).toBeInTheDocument();
    await waitFor(() => {
      expect(screen.getByLabelText('Public summary')).toHaveValue('Catalogue draft 56');
    });
  });
});

describe('exact connection and repository selection', () => {
  it('preflights only the selected service and resolves a canonical repository URL', async () => {
    api.listTriggers.mockResolvedValue([]);
    render(<PublisherSurface ownerId="test-owner" displayIdentity="Chrono Maintainer" />);
    const user = userEvent.setup();

    const selector = await screen.findByLabelText('UserService');
    await user.selectOptions(selector, ACTIVE.service_id);
    await waitFor(() => {
      expect(api.preflightTriggerConnection).toHaveBeenCalledWith(
        ACTIVE.service_id,
        expect.any(AbortSignal),
      );
    });
    await user.type(
      screen.getByLabelText('GitHub repository URL'),
      'https://github.com/upstream-owner/project',
    );
    await user.click(screen.getByRole('button', { name: 'Verify repository' }));

    expect(api.resolveTriggerRepository).toHaveBeenCalledWith(
      ACTIVE.service_id,
      'upstream-owner',
      'project',
      expect.any(AbortSignal),
    );
    expect(await screen.findByText('upstream-owner/project')).toBeInTheDocument();
  });

  it('discards a delayed repository verification after the locator changes', async () => {
    const resolution = controlled<TriggerRepositorySelection>();
    api.listTriggers.mockResolvedValue([]);
    api.resolveTriggerRepository.mockImplementation(
      (_serviceId: string, _owner: string, _name: string, signal: AbortSignal) => {
        expect(signal.aborted).toBe(false);
        return resolution.promise;
      },
    );
    render(<PublisherSurface ownerId="test-owner" displayIdentity="Chrono Maintainer" />);
    const user = userEvent.setup();

    await user.selectOptions(await screen.findByLabelText('UserService'), ACTIVE.service_id);
    await waitFor(() => expect(api.preflightTriggerConnection).toHaveBeenCalled());
    const input = screen.getByLabelText('GitHub repository URL');
    await user.type(input, 'https://github.com/upstream-owner/project');
    await user.click(screen.getByRole('button', { name: 'Verify repository' }));
    const signal = api.resolveTriggerRepository.mock.calls[0]?.[3] as AbortSignal;

    await user.clear(input);
    await user.type(input, 'https://github.com/new-owner/new-project');
    expect(signal.aborted).toBe(true);
    resolution.resolve(REPOSITORY);

    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Verify repository' })).toBeEnabled(),
    );
    expect(input).toHaveValue('https://github.com/new-owner/new-project');
    expect(screen.queryByText('upstream-owner/project')).toBeNull();
    expect(screen.queryByText(/Verified upstream-owner\/project/)).toBeNull();
  });

  it('keeps the form but never offers OAuth as inactive-service reactivation', async () => {
    const inactive: TriggerConnection = { ...ACTIVE, state: 'inactive' };
    api.listTriggers.mockResolvedValue([draft(undefined, 3, inactive.service_id)]);
    api.listTriggerConnections.mockResolvedValue([inactive]);
    api.preflightTriggerConnection.mockRejectedValue(
      new ApiError(409, 'inactive', 'trigger_connection_inactive'),
    );
    render(<PublisherSurface ownerId="test-owner" displayIdentity="Chrono Maintainer" />);

    expect(
      await screen.findByText(/cannot reactivate this inactive UserService/i),
    ).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Reconnect' })).toBeNull();
    await userEvent.click(screen.getByRole('button', { name: 'Check connection' }));
    expect(api.reconnectTriggerConnection).not.toHaveBeenCalled();
    expect(flow.authorizeTriggerConnection).not.toHaveBeenCalled();
    expect(screen.getByLabelText('Public summary')).toHaveValue('Fix one bounded issue');
    expect(screen.queryByRole('button', { name: 'Revoke connection' })).toBeNull();
  });

  it('keeps reconnect available for an exact disconnected service', async () => {
    const disconnected: TriggerConnection = { ...ACTIVE, state: 'disconnected' };
    api.listTriggers.mockResolvedValue([draft(undefined, 3, disconnected.service_id)]);
    api.listTriggerConnections.mockResolvedValue([disconnected]);
    api.preflightTriggerConnection.mockRejectedValue(
      new ApiError(409, 'reauthorization required', 'trigger_reauthorization_required'),
    );
    flow.authorizeTriggerConnection.mockImplementation(async (options) => {
      await options.begin();
      return ACTIVE;
    });
    render(<PublisherSurface ownerId="test-owner" displayIdentity="Chrono Maintainer" />);

    await userEvent.click(await screen.findByRole('button', { name: 'Reconnect' }));

    await waitFor(() =>
      expect(api.reconnectTriggerConnection).toHaveBeenCalledWith(
        disconnected.service_id,
        expect.objectContaining({ idempotencyKey: expect.stringMatching(/^cma-web-/) }),
      ),
    );
  });

  it('requires confirmation before revoking and retains the Trigger draft', async () => {
    render(<PublisherSurface ownerId="test-owner" displayIdentity="Chrono Maintainer" />);
    const user = userEvent.setup();

    await user.click(await screen.findByRole('button', { name: 'Revoke connection' }));
    let dialog = screen.getByRole('dialog', { name: 'Revoke GitHub connection?' });
    expect(dialog).toHaveTextContent('Trigger drafts, published revisions, Launches');
    expect(api.revokeTriggerConnection).not.toHaveBeenCalled();
    await user.click(within(dialog).getByRole('button', { name: 'Keep connection' }));
    expect(api.revokeTriggerConnection).not.toHaveBeenCalled();

    await user.click(screen.getByRole('button', { name: 'Revoke connection' }));
    dialog = screen.getByRole('dialog', { name: 'Revoke GitHub connection?' });
    await user.click(within(dialog).getByRole('button', { name: 'Revoke connection' }));

    await waitFor(() =>
      expect(api.revokeTriggerConnection).toHaveBeenCalledWith(
        ACTIVE.service_id,
        expect.objectContaining({ idempotencyKey: expect.stringMatching(/^cma-web-/) }),
      ),
    );
    expect(screen.getByLabelText('Public summary')).toHaveValue('Fix one bounded issue');
    expect(await screen.findByText('GitHub connection revoked')).toBeInTheDocument();
    expect(screen.getByText(/cannot reactivate this inactive UserService/i)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Reconnect' })).toBeNull();
    expect(screen.getByRole('button', { name: 'Verify repository' })).toBeDisabled();
  });

  it('reconciles an ambiguous OAuth begin with one intent and command key', async () => {
    api.listTriggers.mockResolvedValue([]);
    api.listTriggerConnections.mockResolvedValue([]);
    api.beginTriggerConnection.mockRejectedValue(new TransportError('response lost'));
    flow.authorizeTriggerConnection.mockImplementation(async (options) => {
      const handoff = await options.begin();
      options.onService?.(handoff.service_id);
      return ACTIVE;
    });
    render(<PublisherSurface ownerId="test-owner" displayIdentity="Chrono Maintainer" />);
    const user = userEvent.setup();

    await user.click(await screen.findByRole('button', { name: 'Connect GitHub' }));
    await waitFor(() => expect(api.beginTriggerConnection).toHaveBeenCalledTimes(2));
    expect(api.beginTriggerConnection.mock.calls[1]).toEqual(
      api.beginTriggerConnection.mock.calls[0],
    );

    api.beginTriggerConnection.mockResolvedValue(HANDOFF);
    const reconcile = screen.getByRole('button', { name: 'Reconcile connection' });
    await waitFor(() => expect(reconcile).toBeEnabled());
    await user.click(reconcile);
    await waitFor(() => expect(api.beginTriggerConnection).toHaveBeenCalledTimes(3));
    expect(api.beginTriggerConnection.mock.calls[2]).toEqual(
      api.beginTriggerConnection.mock.calls[0],
    );
  });
});

describe('draft mutations', () => {
  it('creates a draft with canonical numeric repository identity', async () => {
    api.listTriggers.mockResolvedValue([]);
    render(<PublisherSurface ownerId="test-owner" displayIdentity="Chrono Maintainer" />);
    const user = userEvent.setup();
    await user.selectOptions(await screen.findByLabelText('UserService'), ACTIVE.service_id);
    await waitFor(() => expect(api.preflightTriggerConnection).toHaveBeenCalled());
    await user.type(
      screen.getByLabelText('GitHub repository URL'),
      'https://github.com/upstream-owner/project',
    );
    await user.click(screen.getByRole('button', { name: 'Verify repository' }));
    await user.click(screen.getByRole('button', { name: 'Save draft' }));

    await waitFor(() => expect(api.createTrigger).toHaveBeenCalledOnce());
    const [, content] = api.createTrigger.mock.calls[0] as [string, TriggerContentDraft];
    expect(content.repository.repository_id).toBe(10);
    expect(screen.getByText('Trigger draft created')).toBeInTheDocument();
  });

  it('updates under the version read and retains validation failures', async () => {
    render(<PublisherSurface ownerId="test-owner" displayIdentity="Chrono Maintainer" />);
    const user = userEvent.setup();
    const summary = await screen.findByLabelText('Public summary');
    await user.clear(summary);
    await user.click(screen.getByRole('button', { name: 'Save draft' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('public summary');
    expect(summary).toHaveValue('');
    expect(api.updateTrigger).not.toHaveBeenCalled();

    await user.type(summary, 'A retained edit');
    await user.click(screen.getByRole('button', { name: 'Save draft' }));
    await waitFor(() => expect(api.updateTrigger).toHaveBeenCalledOnce());
    expect(api.updateTrigger.mock.calls[0]?.[1]).toBe(3);
  });

  it('retains edits on conflict and retries explicitly against the current version', async () => {
    api.updateTrigger
      .mockRejectedValueOnce(new ApiError(409, 'version conflict', 'revision_conflict'))
      .mockImplementationOnce(
        async (_id: string, _version: number, _service: string, content: TriggerContentDraft) =>
          savedFrom(content, 5),
      );
    api.readTrigger.mockResolvedValue(draft('Changed elsewhere', 4));
    render(<PublisherSurface ownerId="test-owner" displayIdentity="Chrono Maintainer" />);
    const user = userEvent.setup();
    const summary = await screen.findByLabelText('Public summary');
    await user.clear(summary);
    await user.type(summary, 'My retained edit');
    await user.click(screen.getByRole('button', { name: 'Save draft' }));

    expect(await screen.findByText(/changed elsewhere/i)).toBeInTheDocument();
    expect(summary).toHaveValue('My retained edit');
    await user.click(screen.getByRole('button', { name: 'Retry with current version' }));
    await waitFor(() => expect(api.updateTrigger).toHaveBeenCalledTimes(2));
    expect(api.updateTrigger.mock.calls[1]?.[1]).toBe(4);
    expect((api.updateTrigger.mock.calls[1]?.[3] as TriggerContentDraft).task.public_summary).toBe(
      'My retained edit',
    );
  });

  it('freezes and reconciles the exact draft after repeated transport loss', async () => {
    api.updateTrigger.mockRejectedValue(new TransportError('response lost'));
    render(<PublisherSurface ownerId="test-owner" displayIdentity="Chrono Maintainer" />);
    const user = userEvent.setup();
    const summary = await screen.findByLabelText('Public summary');
    await user.clear(summary);
    await user.type(summary, 'Retained publisher edit');
    await user.click(screen.getByRole('button', { name: 'Save draft' }));

    await waitFor(() => expect(api.updateTrigger).toHaveBeenCalledTimes(2));
    expect(summary).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Reconcile save' })).toBeEnabled();
    expect(api.updateTrigger.mock.calls[1]).toEqual(api.updateTrigger.mock.calls[0]);

    api.updateTrigger.mockResolvedValue(draft('Retained publisher edit', 4));
    await user.click(screen.getByRole('button', { name: 'Reconcile save' }));
    await waitFor(() => expect(api.updateTrigger).toHaveBeenCalledTimes(3));
    expect(api.updateTrigger.mock.calls[2]).toEqual(api.updateTrigger.mock.calls[0]);
  });

  it('reconstructs an unresolved private save after reload without sending a changed body', async () => {
    api.updateTrigger.mockRejectedValue(new TransportError('response lost'));
    const first = render(
      <PublisherSurface ownerId="test-owner" displayIdentity="Chrono Maintainer" />,
    );
    const user = userEvent.setup();
    const summary = await screen.findByLabelText('Public summary');
    await user.clear(summary);
    await user.type(summary, 'Private retained publisher edit');
    await user.click(screen.getByRole('button', { name: 'Save draft' }));

    await waitFor(() => expect(api.updateTrigger).toHaveBeenCalledTimes(2));
    const original = api.updateTrigger.mock.calls[0];
    expect(commandStorageText()).not.toContain('Private retained publisher edit');
    first.unmount();

    api.updateTrigger.mockImplementation(
      async (_id: string, _version: number, _service: string, content: TriggerContentDraft) =>
        savedFrom(content),
    );
    render(<PublisherSurface ownerId="test-owner" displayIdentity="Chrono Maintainer" />);

    expect(await screen.findByText(/did not retain the private draft body/i)).toBeInTheDocument();
    const reconstructed = screen.getByLabelText('Public summary');
    expect(reconstructed).toBeEnabled();
    expect(screen.getByRole('button', { name: 'Reconcile save' })).toBeEnabled();

    await user.clear(reconstructed);
    await user.type(reconstructed, 'Different private edit');
    await user.click(screen.getByRole('button', { name: 'Reconcile save' }));
    expect(await screen.findByRole('alert')).toHaveTextContent(
      /does not match the retained private save.*sent nothing/i,
    );
    expect(api.updateTrigger).toHaveBeenCalledTimes(2);
    expect(reconstructed).toBeEnabled();

    await user.clear(reconstructed);
    await user.type(reconstructed, 'Private retained publisher edit');
    await user.click(screen.getByRole('button', { name: 'Reconcile save' }));

    await waitFor(() => expect(api.updateTrigger).toHaveBeenCalledTimes(3));
    expect(api.updateTrigger.mock.calls[2]).toEqual(original);
    expect(await screen.findByRole('status')).toHaveTextContent('Draft saved');
  });
});

describe('immutable publication and retirement', () => {
  it('confirms immutable publication and rereads the advanced mutable draft', async () => {
    const refreshed = { ...draft(), version: 4, next_revision_number: 3 };
    api.readTrigger.mockResolvedValue(refreshed);
    render(<PublisherSurface ownerId="test-owner" displayIdentity="Chrono Maintainer" />);
    await waitFor(() => expect(api.preflightTriggerConnection).toHaveBeenCalled());

    await userEvent.click(await screen.findByRole('button', { name: 'Publish' }));
    const dialog = screen.getByRole('dialog', { name: 'Publish immutable revision' });
    expect(
      within(dialog).getByText('https://github.com/upstream-owner/project'),
    ).toBeInTheDocument();
    await userEvent.click(
      within(dialog).getByRole('button', { name: 'Publish immutable revision' }),
    );

    await waitFor(() => expect(api.publishTrigger).toHaveBeenCalledOnce());
    await waitFor(() => expect(api.readTrigger).toHaveBeenCalledWith(draft().id));
    expect(await screen.findByText('Revision 1 published')).toBeInTheDocument();
    expect(screen.getByText('v4')).toBeInTheDocument();
  });

  it('copies public and README destinations and retires only after inline confirmation', async () => {
    api.listTriggerRevisions.mockResolvedValue([revision()]);
    render(<PublisherSurface ownerId="test-owner" displayIdentity="Chrono Maintainer" />);
    const user = userEvent.setup();
    const row = (await screen.findByText('Revision 1')).closest('li');
    expect(row).not.toBeNull();

    await user.click(within(row!).getByRole('button', { name: 'Copy public URL for revision 1' }));
    expect(await navigator.clipboard.readText()).toBe(
      `${window.location.origin}/triggers/${revision().public_id}`,
    );
    await user.click(
      within(row!).getByRole('button', { name: 'Copy README Markdown for revision 1' }),
    );
    expect(await navigator.clipboard.readText()).toBe(
      `[Open in CMA](${window.location.origin}/triggers/${revision().public_id})`,
    );

    await user.click(within(row!).getByRole('button', { name: 'Retire revision 1' }));
    expect(within(row!).getByText(/Existing Launches continue/i)).toBeInTheDocument();
    await user.click(within(row!).getByRole('button', { name: 'Retire revision' }));
    await waitFor(() => expect(api.retireTriggerRevision).toHaveBeenCalledOnce());
    expect(await screen.findByText('retired')).toBeInTheDocument();
  });

  it('retains publication confirmation until the exact command reconciles', async () => {
    api.publishTrigger.mockRejectedValue(new TransportError('response lost'));
    render(<PublisherSurface ownerId="test-owner" displayIdentity="Chrono Maintainer" />);
    const user = userEvent.setup();
    await waitFor(() => expect(api.preflightTriggerConnection).toHaveBeenCalled());
    await user.click(await screen.findByRole('button', { name: 'Publish' }));
    const dialog = screen.getByRole('dialog', { name: 'Publish immutable revision' });
    await user.click(within(dialog).getByRole('button', { name: 'Publish immutable revision' }));

    await waitFor(() => expect(api.publishTrigger).toHaveBeenCalledTimes(2));
    await waitFor(() =>
      expect(within(dialog).getByRole('button', { name: 'Reconcile publication' })).toBeEnabled(),
    );
    expect(within(dialog).getByRole('button', { name: 'Keep editing' })).toBeDisabled();
    expect(api.publishTrigger.mock.calls[1]).toEqual(api.publishTrigger.mock.calls[0]);

    api.publishTrigger.mockResolvedValue(revision());
    await user.click(within(dialog).getByRole('button', { name: 'Reconcile publication' }));
    await waitFor(() => expect(api.publishTrigger).toHaveBeenCalledTimes(3));
    expect(api.publishTrigger.mock.calls[2]).toEqual(api.publishTrigger.mock.calls[0]);
  });

  it('reopens and reconciles an unresolved publication after reload', async () => {
    api.publishTrigger.mockRejectedValue(new TransportError('response lost'));
    const first = render(
      <PublisherSurface ownerId="test-owner" displayIdentity="Chrono Maintainer" />,
    );
    const user = userEvent.setup();
    await waitFor(() => expect(api.preflightTriggerConnection).toHaveBeenCalled());
    await user.click(await screen.findByRole('button', { name: 'Publish' }));
    let dialog = screen.getByRole('dialog', { name: 'Publish immutable revision' });
    await user.click(within(dialog).getByRole('button', { name: 'Publish immutable revision' }));
    await waitFor(() => expect(api.publishTrigger).toHaveBeenCalledTimes(2));
    const original = api.publishTrigger.mock.calls[0];
    first.unmount();

    api.publishTrigger.mockResolvedValue(revision());
    render(<PublisherSurface ownerId="test-owner" displayIdentity="Chrono Maintainer" />);

    dialog = await screen.findByRole('dialog', { name: 'Publish immutable revision' });
    expect(within(dialog).getByText('2')).toBeInTheDocument();
    await user.click(within(dialog).getByRole('button', { name: 'Reconcile publication' }));

    await waitFor(() => expect(api.publishTrigger).toHaveBeenCalledTimes(3));
    expect(api.publishTrigger.mock.calls[2]).toEqual(original);
    expect(await screen.findByText('Revision 1 published')).toBeInTheDocument();
  });

  it('retains a published revision and locks stale publication when draft reread fails', async () => {
    const refreshed = { ...draft(), version: 4, next_revision_number: 3 };
    api.readTrigger.mockRejectedValue(new Error('advanced draft unavailable'));
    render(<PublisherSurface ownerId="test-owner" displayIdentity="Chrono Maintainer" />);
    const user = userEvent.setup();
    await waitFor(() => expect(api.preflightTriggerConnection).toHaveBeenCalled());
    await user.click(await screen.findByRole('button', { name: 'Publish' }));
    const dialog = screen.getByRole('dialog', { name: 'Publish immutable revision' });
    await user.click(within(dialog).getByRole('button', { name: 'Publish immutable revision' }));

    expect(await screen.findByText(/Revision 1 was published/i)).toBeInTheDocument();
    expect(screen.getByText('Revision 1')).toBeInTheDocument();
    expect(screen.getByText('published')).toBeInTheDocument();
    expect(screen.queryByText('The Trigger was not changed.')).toBeNull();
    expect(screen.queryByRole('dialog', { name: 'Publish immutable revision' })).toBeNull();
    expect(screen.getByRole('button', { name: 'Publish' })).toBeDisabled();
    expect(api.publishTrigger).toHaveBeenCalledOnce();

    api.readTrigger.mockResolvedValue(refreshed);
    await user.click(screen.getByRole('button', { name: 'Retry draft refresh' }));

    expect(await screen.findByText('v4')).toBeInTheDocument();
    expect(screen.queryByText(/Revision 1 was published/i)).toBeNull();
    expect(screen.getByRole('button', { name: 'Publish' })).toBeEnabled();
    expect(api.publishTrigger).toHaveBeenCalledOnce();
  });

  it('pins retirement confirmation while its outcome is ambiguous', async () => {
    api.listTriggerRevisions.mockResolvedValue([revision()]);
    api.retireTriggerRevision.mockRejectedValue(new TransportError('response lost'));
    render(<PublisherSurface ownerId="test-owner" displayIdentity="Chrono Maintainer" />);
    const user = userEvent.setup();
    const row = (await screen.findByText('Revision 1')).closest('li');
    expect(row).not.toBeNull();
    await user.click(within(row!).getByRole('button', { name: 'Retire revision 1' }));
    await user.click(within(row!).getByRole('button', { name: 'Retire revision' }));

    await waitFor(() => expect(api.retireTriggerRevision).toHaveBeenCalledTimes(2));
    expect(within(row!).getByRole('button', { name: 'Keep it' })).toBeDisabled();
    await waitFor(() =>
      expect(within(row!).getByRole('button', { name: 'Reconcile retirement' })).toBeEnabled(),
    );

    api.retireTriggerRevision.mockResolvedValue(revision('retired'));
    await user.click(within(row!).getByRole('button', { name: 'Reconcile retirement' }));
    await waitFor(() => expect(api.retireTriggerRevision).toHaveBeenCalledTimes(3));
    expect(api.retireTriggerRevision.mock.calls[2]).toEqual(
      api.retireTriggerRevision.mock.calls[0],
    );
  });

  it('reopens and reconciles an unresolved retirement after reload', async () => {
    api.listTriggerRevisions.mockResolvedValue([revision()]);
    api.retireTriggerRevision.mockRejectedValue(new TransportError('response lost'));
    const first = render(
      <PublisherSurface ownerId="test-owner" displayIdentity="Chrono Maintainer" />,
    );
    const user = userEvent.setup();
    let row = (await screen.findByText('Revision 1')).closest('li');
    expect(row).not.toBeNull();
    await user.click(within(row!).getByRole('button', { name: 'Retire revision 1' }));
    await user.click(within(row!).getByRole('button', { name: 'Retire revision' }));
    await waitFor(() => expect(api.retireTriggerRevision).toHaveBeenCalledTimes(2));
    const original = api.retireTriggerRevision.mock.calls[0];
    first.unmount();

    api.retireTriggerRevision.mockResolvedValue(revision('retired'));
    render(<PublisherSurface ownerId="test-owner" displayIdentity="Chrono Maintainer" />);

    row = (await screen.findByText('Revision 1')).closest('li');
    expect(row).not.toBeNull();
    const reconcile = await within(row!).findByRole('button', { name: 'Reconcile retirement' });
    expect(reconcile).toBeEnabled();
    await user.click(reconcile);

    await waitFor(() => expect(api.retireTriggerRevision).toHaveBeenCalledTimes(3));
    expect(api.retireTriggerRevision.mock.calls[2]).toEqual(original);
    expect(await within(row!).findByText('retired')).toBeInTheDocument();
  });
});

describe('rendering and responsive contracts', () => {
  it('renders repository and prompt-derived labels as text, never markup', async () => {
    api.listTriggers.mockResolvedValue([draft('<img src=x onerror=alert(1)>')]);
    render(<PublisherSurface ownerId="test-owner" displayIdentity="Chrono Maintainer" />);
    expect(await screen.findAllByText('<img src=x onerror=alert(1)>')).not.toHaveLength(0);
    expect(document.querySelector('img[src="x"]')).toBeNull();
  });

  it('ships the native narrow draft selector and single-column breakpoint', async () => {
    render(<PublisherSurface ownerId="test-owner" displayIdentity="Chrono Maintainer" />);
    expect(await screen.findByLabelText('Draft')).toBeInstanceOf(HTMLSelectElement);
    const css = readFileSync('src/publisher/PublisherSurface.css', 'utf8');
    expect(css).toMatch(/@media \(max-width: 48rem\)/);
    expect(css).toMatch(/\.publisher__draft-picker\s*\{[^}]*display: flex/s);
    expect(css).toMatch(/\.publisher-fields[\s\S]*grid-template-columns: minmax\(0, 1fr\)/);
  });
});
