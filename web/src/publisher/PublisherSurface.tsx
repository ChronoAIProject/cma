import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { ChangeEvent } from 'react';
import type {
  TriggerConnection,
  TriggerContentDraft,
  TriggerDraft,
  TriggerEditableField,
  TriggerRepositoryOperationClass,
  TriggerRevision,
} from '@chronoai/cma-sdk';
import {
  ApiError,
  beginTriggerConnection,
  createTrigger,
  listTriggerConnections,
  listTriggerRevisions,
  listTriggers,
  preflightTriggerConnection,
  publishTrigger,
  readTrigger,
  reconnectTriggerConnection,
  revokeTriggerConnection,
  resolveTriggerRepository,
  retireTriggerRevision,
  updateTrigger,
} from '../api/client';
import {
  INACTIVE_TRIGGER_CONNECTION_GUIDANCE,
  authorizeTriggerConnection,
  isActiveTriggerConnection,
  isReconnectableTriggerConnection,
} from '../trigger/connectionFlow';
import {
  commandObject,
  exactString,
  nonEmptyString,
  nonNegativeInteger,
} from '../trigger/commandValidation';
import {
  CommandRequestUnavailableError,
  commandOutcomeIsUnresolved,
  retainedCommandMetadata,
  retainedCommandRequest,
  useIdempotentCommand,
} from '../trigger/useIdempotentCommand';
import type { PendingBrowserCommand } from '../trigger/useIdempotentCommand';
import { Button } from '../ui/Button';
import { InlineConfirm } from '../ui/InlineConfirm';
import { Input } from '../ui/Input';
import { Modal } from '../ui/Modal';
import {
  IconBranchOutline16,
  IconCheckOutline16,
  IconCopyOutline16,
  IconLinkOutline16,
  IconPlusOutline16,
  IconRefreshOutline16,
  IconTrashOutline16,
  IconWarningOutline16,
} from '../ui/icons';
import { TaskInputEditor } from './TaskInputEditor';
import {
  EDITABLE_FIELDS,
  REPOSITORY_OPERATIONS,
  REQUIRED_CAPABILITIES,
  applyRepository,
  canonicalRepositoryUrl,
  editorFromDraft,
  editorIsDirty,
  firstValidationMessage,
  newPublisherEditor,
  parseGithubRepositoryUrl,
  validatePublisherEditor,
  type PublisherEditor,
} from './model';
import './PublisherSurface.css';

type Operation =
  | 'loading'
  | 'connecting'
  | 'checking'
  | 'revoking'
  | 'resolving'
  | 'saving'
  | 'publishing'
  | 'retiring'
  | 'refreshing';

interface Conflict {
  readonly current: TriggerDraft;
}

interface PendingDraftSave {
  readonly slot: string;
  readonly resource: string;
  readonly action: 'create-trigger' | 'update-trigger';
  readonly draftId: string | null;
  readonly expectedVersion: number | null;
  readonly serviceId: string | null;
}

interface PendingPublication {
  readonly slot: string;
  readonly resource: string;
  readonly triggerId: string;
  readonly expectedVersion: number;
  readonly serviceId: string;
  readonly revisionNumber: number;
}

interface PendingRetirement {
  readonly slot: string;
  readonly resource: string;
  readonly revisionId: string;
}

interface PublishedDraftRefresh {
  readonly triggerId: string;
  readonly revision: TriggerRevision;
  readonly failure: string;
}

type ConnectionIssue = 'inactive' | 'reauthorization' | null;

function connectionIssue(cause: unknown): ConnectionIssue {
  if (!(cause instanceof ApiError)) return null;
  if (cause.code === 'trigger_connection_inactive') return 'inactive';
  if (cause.code === 'trigger_reauthorization_required') return 'reauthorization';
  return null;
}

function pendingDraftSave(command: PendingBrowserCommand | null): PendingDraftSave | null {
  if (
    command === null ||
    (command.action !== 'create-trigger' && command.action !== 'update-trigger')
  ) {
    return null;
  }
  const retained = retainedCommandMetadata<{
    readonly draftId: string | null;
    readonly expectedVersion: number | null;
    readonly serviceId: string | null;
  }>(command, command, (value) => {
    if (typeof value !== 'object' || value === null || Array.isArray(value)) return null;
    const record = value as Record<string, unknown>;
    const draftId = record.draftId;
    const expectedVersion = record.expectedVersion;
    const serviceId = record.serviceId;
    if (draftId !== null && nonEmptyString(draftId) === null) return null;
    if (
      expectedVersion !== null &&
      expectedVersion !== undefined &&
      nonNegativeInteger(expectedVersion) === null
    ) {
      return null;
    }
    if (serviceId !== null && serviceId !== undefined && nonEmptyString(serviceId) === null) {
      return null;
    }
    return {
      draftId: draftId as string | null,
      expectedVersion: (expectedVersion ?? null) as number | null,
      serviceId: (serviceId ?? null) as string | null,
    };
  });
  if (retained === undefined) return null;
  if (command.action === 'create-trigger') {
    if (
      command.slot !== 'trigger:create' ||
      command.resource !== 'publisher' ||
      retained.draftId !== null ||
      retained.expectedVersion !== null
    ) {
      return null;
    }
  } else if (
    retained.draftId === null ||
    retained.expectedVersion === null ||
    command.slot !== `trigger:update:${retained.draftId}` ||
    command.resource !== `trigger:${retained.draftId}`
  ) {
    return null;
  }
  return { ...retained, slot: command.slot, resource: command.resource, action: command.action };
}

function pendingPublication(command: PendingBrowserCommand | null): PendingPublication | null {
  if (command === null || command.action !== 'publish-trigger') return null;
  const request = commandObject<{
    readonly triggerId: string;
    readonly expectedVersion: number;
    readonly serviceId: string;
  }>(command.request, {
    triggerId: nonEmptyString,
    expectedVersion: nonNegativeInteger,
    serviceId: nonEmptyString,
  });
  if (request === null || command.resource !== `trigger:${request.triggerId}`) return null;
  const prefix = `trigger:publish:${request.triggerId}:`;
  if (!command.slot.startsWith(prefix)) return null;
  const revisionText = command.slot.slice(prefix.length);
  if (!/^(0|[1-9][0-9]*)$/.test(revisionText)) return null;
  const revisionNumber = Number(revisionText);
  if (!Number.isSafeInteger(revisionNumber)) return null;
  return { ...request, slot: command.slot, resource: command.resource, revisionNumber };
}

function pendingRetirement(command: PendingBrowserCommand | null): PendingRetirement | null {
  if (command === null || command.action !== 'retire-trigger') return null;
  const request = commandObject<{ readonly revisionId: string }>(command.request, {
    revisionId: nonEmptyString,
  });
  if (
    request === null ||
    command.slot !== `trigger:retire:${request.revisionId}` ||
    command.resource !== `revision:${request.revisionId}`
  ) {
    return null;
  }
  return { ...request, slot: command.slot, resource: command.resource };
}

interface PublisherSurfaceProps {
  readonly ownerId: string;
  readonly displayIdentity: string;
}

function message(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

function short(value: string, length = 8): string {
  return value.length <= length ? value : value.slice(0, length);
}

function replaceDraft(
  items: readonly TriggerDraft[],
  draft: TriggerDraft,
): readonly TriggerDraft[] {
  return [draft, ...items.filter((item) => item.id !== draft.id)];
}

function connectionLabel(connection: TriggerConnection): string {
  const account = connection.account_login ?? 'Not connected';
  return `${account} · ${short(connection.service_id)} · ${connection.state}`;
}

function publicUrl(revision: TriggerRevision): string {
  return new URL(`/triggers/${encodeURIComponent(revision.public_id)}`, window.location.origin)
    .href;
}

function readmeLink(revision: TriggerRevision): string {
  return `[Open in CMA](${publicUrl(revision)})`;
}

function inputNumber(event: ChangeEvent<HTMLInputElement>): number {
  return Number.isNaN(event.target.valueAsNumber) ? 0 : event.target.valueAsNumber;
}

export function PublisherSurface({ ownerId, displayIdentity }: PublisherSurfaceProps) {
  const [drafts, setDrafts] = useState<readonly TriggerDraft[]>([]);
  const [revisions, setRevisions] = useState<readonly TriggerRevision[]>([]);
  const [connections, setConnections] = useState<readonly TriggerConnection[]>([]);
  const [editor, setEditor] = useState<PublisherEditor>(() => newPublisherEditor(displayIdentity));
  const [checkedConnection, setCheckedConnection] = useState<TriggerConnection | null>(null);
  const [operation, setOperation] = useState<Operation | null>('loading');
  const [failure, setFailure] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [conflict, setConflict] = useState<Conflict | null>(null);
  const [publishOpen, setPublishOpen] = useState(false);
  const [confirmingRevoke, setConfirmingRevoke] = useState<string | null>(null);
  const [retiringId, setRetiringId] = useState<string | null>(null);
  const [discardEditor, setDiscardEditor] = useState<PublisherEditor | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  const [readAttempt, setReadAttempt] = useState(0);
  const [loaded, setLoaded] = useState(false);
  const [selectedConnectionIssue, setSelectedConnectionIssue] = useState<ConnectionIssue>(null);
  const [publishedDraftRefresh, setPublishedDraftRefresh] = useState<PublishedDraftRefresh | null>(
    null,
  );
  const authorization = useRef<AbortController | null>(null);
  const repositoryResolution = useRef<{
    readonly generation: number;
    readonly controller: AbortController;
    readonly serviceId: string;
    readonly owner: string;
    readonly name: string;
  } | null>(null);
  const repositoryResolutionGeneration = useRef(0);
  const editorRef = useRef(editor);
  const heading = useRef<HTMLHeadingElement>(null);
  const alignedSaveSlot = useRef<string | null>(null);
  const alignedPublicCommandSlot = useRef<string | null>(null);
  const commandResources = useMemo(
    () => [
      'publisher',
      ...drafts.map((draft) => `trigger:${draft.id}`),
      ...revisions.map((revision) => `revision:${revision.id}`),
    ],
    [drafts, revisions],
  );
  const {
    execute: executeCommand,
    pending,
    failure: commandFailure,
  } = useIdempotentCommand(ownerId, commandResources);

  const validation = useMemo(() => validatePublisherEditor(editor), [editor]);
  const dirty = editorIsDirty(editor);
  const busy = operation !== 'loading' && operation !== null;
  const selectedConnection =
    connections.find((connection) => connection.service_id === editor.selectedServiceId) ?? null;
  const activeConnection =
    checkedConnection?.service_id === editor.selectedServiceId &&
    isActiveTriggerConnection(checkedConnection);
  const reconnectableConnection =
    selectedConnectionIssue === 'reauthorization' ||
    isReconnectableTriggerConnection(selectedConnection);
  const inactiveConnection =
    selectedConnectionIssue === 'inactive' || selectedConnection?.state === 'inactive';
  const pendingBegin = pending?.slot === 'connection:begin';
  const pendingReconnect = pending?.slot === `connection:reconnect:${editor.selectedServiceId}`;
  const pendingRevoke = pending?.action === 'revoke-connection';
  const pendingRevokeRequest = pendingRevoke
    ? commandObject(pending.request, { serviceId: nonEmptyString })
    : null;
  const revokeServiceId = pendingRevokeRequest?.serviceId ?? confirmingRevoke;
  const revokeConnectionAccount =
    connections.find((connection) => connection.service_id === revokeServiceId)?.account_login ??
    null;
  const saveCommand = useMemo(() => pendingDraftSave(pending), [pending]);
  const publicationCommand = useMemo(() => pendingPublication(pending), [pending]);
  const retirementCommand = useMemo(() => pendingRetirement(pending), [pending]);
  const pendingSave = saveCommand !== null;
  const pendingPublish = publicationCommand !== null;
  const reconstructingPrivateSave = pendingSave && pending?.requiresRequestReconstruction === true;
  const refreshRequired = publishedDraftRefresh !== null;

  const currentRevisions = useMemo(() => {
    if (editor.server === null) return [];
    return revisions
      .filter((revision) => revision.content.trigger_id === editor.server?.id)
      .sort((left, right) => right.content.revision_number - left.content.revision_number);
  }, [editor.server, revisions]);
  const publicationDraft =
    publicationCommand === null
      ? editor.server
      : (drafts.find((draft) => draft.id === publicationCommand.triggerId) ?? null);
  const publicationEditor = publicationDraft === null ? editor : editorFromDraft(publicationDraft);

  useEffect(() => {
    heading.current?.focus();
  }, []);

  useEffect(() => {
    editorRef.current = editor;
  }, [editor]);

  useEffect(() => {
    let live = true;
    void Promise.all([listTriggers(), listTriggerRevisions(), listTriggerConnections()])
      .then(([loadedDrafts, loadedRevisions, loadedConnections]) => {
        if (!live) return;
        setDrafts(loadedDrafts);
        setRevisions(loadedRevisions);
        setConnections(loadedConnections);
        setLoaded(true);
        setCheckedConnection(null);
        setSelectedConnectionIssue(null);
        const first = loadedDrafts[0];
        setEditor(
          first === undefined ? newPublisherEditor(displayIdentity) : editorFromDraft(first),
        );
      })
      .catch((cause: unknown) => {
        if (live) setFailure(message(cause));
      })
      .finally(() => {
        if (live) setOperation((current) => (current === 'loading' ? null : current));
      });
    return () => {
      live = false;
    };
  }, [displayIdentity, readAttempt]);

  useEffect(() => {
    if (saveCommand === null || pending?.requiresRequestReconstruction !== true) {
      alignedSaveSlot.current = null;
      return;
    }
    if (!loaded || alignedSaveSlot.current === saveCommand.slot) return;
    let next: PublisherEditor;
    if (saveCommand.draftId === null) {
      next = newPublisherEditor(displayIdentity);
    } else {
      const target = drafts.find((draft) => draft.id === saveCommand.draftId);
      if (target === undefined) return;
      next = editorFromDraft(target);
    }
    if (saveCommand.serviceId !== null) {
      next = { ...next, selectedServiceId: saveCommand.serviceId };
    }
    let live = true;
    void Promise.resolve().then(() => {
      if (!live) return;
      alignedSaveSlot.current = saveCommand.slot;
      setCheckedConnection(null);
      setSelectedConnectionIssue(null);
      setEditor(next);
      setConflict(null);
      setFailure(null);
      setStatus(null);
    });
    return () => {
      live = false;
    };
  }, [displayIdentity, drafts, loaded, pending?.requiresRequestReconstruction, saveCommand]);

  useEffect(() => {
    const slot = publicationCommand?.slot ?? retirementCommand?.slot ?? null;
    if (slot === null) {
      alignedPublicCommandSlot.current = null;
      return;
    }
    if (!loaded || alignedPublicCommandSlot.current === slot) return;
    const triggerId =
      publicationCommand?.triggerId ??
      revisions.find((revision) => revision.id === retirementCommand?.revisionId)?.content
        .trigger_id;
    if (triggerId === undefined) return;
    const target = drafts.find((draft) => draft.id === triggerId);
    if (target === undefined) return;
    if (editor.server?.id === target.id) {
      alignedPublicCommandSlot.current = slot;
      return;
    }
    let live = true;
    void Promise.resolve().then(() => {
      if (!live) return;
      alignedPublicCommandSlot.current = slot;
      setCheckedConnection(null);
      setSelectedConnectionIssue(null);
      setEditor(editorFromDraft(target));
      setConflict(null);
      setFailure(null);
      setStatus(null);
    });
    return () => {
      live = false;
    };
  }, [drafts, editor.server?.id, loaded, publicationCommand, retirementCommand, revisions]);

  useEffect(() => {
    const serviceId = editor.selectedServiceId;
    if (serviceId === '') return;
    const controller = new AbortController();
    void preflightTriggerConnection(serviceId, controller.signal)
      .then((connection) => {
        setSelectedConnectionIssue(null);
        setCheckedConnection(connection);
        setConnections((items) => [
          connection,
          ...items.filter((item) => item.service_id !== connection.service_id),
        ]);
      })
      .catch((cause: unknown) => {
        const issue = connectionIssue(cause);
        if (!controller.signal.aborted && issue !== null) setSelectedConnectionIssue(issue);
        if (
          !controller.signal.aborted &&
          (!(cause instanceof ApiError) ||
            (cause.code !== 'trigger_connection_inactive' &&
              cause.code !== 'trigger_reauthorization_required'))
        ) {
          setFailure(message(cause));
        }
      });
    return () => {
      controller.abort();
    };
  }, [editor.selectedServiceId]);

  useEffect(
    () => () => {
      authorization.current?.abort();
      repositoryResolution.current?.controller.abort();
    },
    [],
  );

  const invalidateRepositoryResolution = useCallback(() => {
    repositoryResolutionGeneration.current += 1;
    repositoryResolution.current?.controller.abort();
    repositoryResolution.current = null;
    setOperation((current) => (current === 'resolving' ? null : current));
  }, []);

  const changeContent = useCallback(
    (change: (content: TriggerContentDraft) => TriggerContentDraft) => {
      setEditor((current) => ({ ...current, content: change(current.content) }));
      setConflict(null);
      setFailure(null);
      setStatus(null);
    },
    [],
  );

  function chooseEditor(next: PublisherEditor): void {
    if (pending !== null) return;
    if (dirty) {
      setDiscardEditor(next);
      return;
    }
    setCheckedConnection(null);
    setSelectedConnectionIssue(null);
    setEditor(next);
    setConflict(null);
    setFailure(null);
    setStatus(null);
  }

  async function refreshConnections(): Promise<void> {
    const latest = await listTriggerConnections();
    setConnections(latest);
  }

  async function authorize(mode: 'begin' | 'reconnect'): Promise<void> {
    if (mode === 'reconnect' && !reconnectableConnection) {
      setFailure(INACTIVE_TRIGGER_CONNECTION_GUIDANCE);
      return;
    }
    authorization.current?.abort();
    const controller = new AbortController();
    authorization.current = controller;
    setOperation('connecting');
    setFailure(null);
    setStatus('Waiting for GitHub authorization');
    try {
      const connection = await authorizeTriggerConnection({
        signal: controller.signal,
        inactiveIsPending: mode === 'begin',
        begin: () => {
          if (mode === 'begin') {
            const identity = {
              slot: 'connection:begin',
              resource: 'publisher',
              action: 'begin-connection',
            } as const;
            const request = retainedCommandRequest(
              pending,
              identity,
              {
                intentId: crypto.randomUUID(),
              },
              (value) => commandObject(value, { intentId: nonEmptyString }),
            );
            return executeCommand(
              { ...identity, request, persistRequest: true },
              'GitHub connection',
              (idempotencyKey, frozen) =>
                beginTriggerConnection(frozen.intentId, { idempotencyKey }),
            );
          }
          const identity = {
            slot: `connection:reconnect:${editor.selectedServiceId}`,
            resource: 'publisher',
            action: 'reconnect-connection',
          } as const;
          const request = retainedCommandRequest(
            pending,
            identity,
            {
              serviceId: editor.selectedServiceId,
            },
            (value) =>
              commandObject(value, {
                serviceId: exactString(editor.selectedServiceId),
              }),
          );
          return executeCommand(
            { ...identity, request, persistRequest: true },
            'GitHub reconnection',
            (idempotencyKey, frozen) =>
              reconnectTriggerConnection(frozen.serviceId, { idempotencyKey }),
          );
        },
        preflight: preflightTriggerConnection,
        onService: (serviceId) => {
          setCheckedConnection(null);
          setSelectedConnectionIssue(null);
          setEditor((current) => ({
            ...current,
            selectedServiceId: serviceId,
            repository: null,
          }));
        },
      });
      setSelectedConnectionIssue(null);
      setCheckedConnection(connection);
      await refreshConnections();
      setStatus(`Connected as ${connection.account_login ?? 'GitHub user'}`);
    } catch (cause) {
      if (!controller.signal.aborted) {
        setFailure(
          commandOutcomeIsUnresolved(cause)
            ? 'The connection outcome is unknown. CMA retained the exact command for reconciliation.'
            : message(cause),
        );
      }
    } finally {
      if (authorization.current === controller) authorization.current = null;
      setOperation((current) => (current === 'connecting' ? null : current));
    }
  }

  async function checkConnection(): Promise<void> {
    if (editor.selectedServiceId === '') return;
    setOperation('checking');
    setFailure(null);
    try {
      const connection = await preflightTriggerConnection(editor.selectedServiceId);
      setSelectedConnectionIssue(null);
      setCheckedConnection(connection);
      await refreshConnections();
      setStatus(`Connection verified for ${connection.account_login ?? 'GitHub user'}`);
    } catch (cause) {
      const issue = connectionIssue(cause);
      setSelectedConnectionIssue(issue);
      setCheckedConnection(null);
      setFailure(issue === 'inactive' ? INACTIVE_TRIGGER_CONNECTION_GUIDANCE : message(cause));
    } finally {
      setOperation((current) => (current === 'checking' ? null : current));
    }
  }

  async function revokeConnection(serviceId: string | null): Promise<void> {
    if (serviceId === null) return;
    setOperation('revoking');
    setFailure(null);
    setStatus(null);
    const identity = {
      slot: `connection:revoke:${serviceId}`,
      resource: 'publisher',
      action: 'revoke-connection',
    } as const;
    try {
      const request = retainedCommandRequest(pending, identity, { serviceId }, (value) =>
        commandObject(value, { serviceId: exactString(serviceId) }),
      );
      const revoked = await executeCommand(
        { ...identity, request, persistRequest: true },
        'GitHub connection revocation',
        (idempotencyKey, frozen) => revokeTriggerConnection(frozen.serviceId, { idempotencyKey }),
      );
      if (revoked.service_id !== serviceId) {
        throw new Error('CMA returned revocation state for a different GitHub connection.');
      }
      setConnections((items) => [
        revoked,
        ...items.filter((item) => item.service_id !== revoked.service_id),
      ]);
      setCheckedConnection(null);
      setSelectedConnectionIssue(revoked.state === 'inactive' ? 'inactive' : null);
      setEditor((current) =>
        current.selectedServiceId === serviceId ? { ...current, repository: null } : current,
      );
      setConfirmingRevoke(null);
      setStatus('GitHub connection revoked');
    } catch (cause) {
      if (!commandOutcomeIsUnresolved(cause)) setConfirmingRevoke(null);
      setFailure(
        commandOutcomeIsUnresolved(cause)
          ? 'The revocation outcome is unknown. CMA retained the exact connection and command key for reconciliation.'
          : message(cause),
      );
    } finally {
      setOperation((current) => (current === 'revoking' ? null : current));
    }
  }

  async function resolveRepository(): Promise<void> {
    if (!activeConnection) {
      setFailure('Verify an active GitHub connection before selecting a repository.');
      return;
    }
    let locator;
    try {
      locator = parseGithubRepositoryUrl(editor.repositoryUrl);
    } catch (cause) {
      setFailure(message(cause));
      return;
    }
    repositoryResolution.current?.controller.abort();
    const generation = repositoryResolutionGeneration.current + 1;
    repositoryResolutionGeneration.current = generation;
    const serviceId = editor.selectedServiceId;
    const controller = new AbortController();
    repositoryResolution.current = {
      generation,
      controller,
      serviceId,
      owner: locator.owner,
      name: locator.name,
    };
    setOperation('resolving');
    setFailure(null);
    try {
      const selection = await resolveTriggerRepository(
        serviceId,
        locator.owner,
        locator.name,
        controller.signal,
      );
      const current = editorRef.current;
      let currentLocator;
      try {
        currentLocator = parseGithubRepositoryUrl(current.repositoryUrl);
      } catch {
        return;
      }
      if (
        controller.signal.aborted ||
        repositoryResolutionGeneration.current !== generation ||
        current.selectedServiceId !== serviceId ||
        currentLocator.owner !== locator.owner ||
        currentLocator.name !== locator.name
      ) {
        return;
      }
      setEditor((current) => applyRepository(current, selection));
      setStatus(
        `Verified ${selection.repository.canonical_owner}/${selection.repository.canonical_name}`,
      );
    } catch (cause) {
      if (!controller.signal.aborted && repositoryResolutionGeneration.current === generation) {
        setFailure(message(cause));
      }
    } finally {
      if (repositoryResolutionGeneration.current === generation) {
        repositoryResolution.current = null;
        setOperation((current) => (current === 'resolving' ? null : current));
      }
    }
  }

  function acceptDraft(saved: TriggerDraft, notice: string): void {
    setDrafts((items) => replaceDraft(items, saved));
    setEditor(editorFromDraft(saved));
    setConflict(null);
    setFailure(null);
    setPublishedDraftRefresh(null);
    setStatus(notice);
  }

  async function refreshPublishedDraft(): Promise<void> {
    const retained = publishedDraftRefresh;
    if (retained === null) return;
    setOperation('refreshing');
    try {
      const refreshed = await readTrigger(retained.triggerId);
      acceptDraft(
        refreshed,
        `Revision ${String(retained.revision.content.revision_number)} published`,
      );
    } catch (cause) {
      setPublishedDraftRefresh({ ...retained, failure: message(cause) });
    } finally {
      setOperation((current) => (current === 'refreshing' ? null : current));
    }
  }

  async function save(expectedVersion?: number): Promise<TriggerDraft | null> {
    const problem = firstValidationMessage(validation);
    if (problem !== undefined) {
      setFailure(problem);
      return null;
    }
    if (saveCommand === null && !activeConnection) {
      setFailure('Check the selected GitHub connection before saving.');
      return null;
    }
    setOperation('saving');
    setFailure(null);
    const server = editor.server;
    const draftId = saveCommand?.draftId ?? server?.id ?? null;
    const retainedVersion =
      saveCommand === null
        ? server === null
          ? null
          : (expectedVersion ?? server.version)
        : saveCommand.expectedVersion;
    const serviceId = saveCommand?.serviceId ?? editor.selectedServiceId;
    const slot =
      saveCommand?.slot ?? (draftId === null ? 'trigger:create' : `trigger:update:${draftId}`);
    const resource =
      saveCommand?.resource ?? (draftId === null ? 'publisher' : `trigger:${draftId}`);
    const action = saveCommand?.action ?? (draftId === null ? 'create-trigger' : 'update-trigger');
    try {
      const request = {
        draftId,
        expectedVersion: retainedVersion,
        serviceId,
        content: editor.content,
      };
      const saved = await executeCommand(
        {
          slot,
          resource,
          action,
          request,
          retained: {
            draftId,
            expectedVersion: retainedVersion,
            serviceId,
          },
        },
        'Trigger draft save',
        (idempotencyKey, frozen) => {
          if (frozen.draftId === null) {
            return createTrigger(frozen.serviceId, frozen.content, { idempotencyKey });
          }
          if (frozen.expectedVersion === null) {
            throw new Error('The retained Trigger update has no draft version.');
          }
          return updateTrigger(
            frozen.draftId,
            frozen.expectedVersion,
            frozen.serviceId,
            frozen.content,
            { idempotencyKey },
          );
        },
      );
      acceptDraft(saved, draftId === null ? 'Trigger draft created' : 'Draft saved');
      return saved;
    } catch (cause) {
      if (commandOutcomeIsUnresolved(cause)) {
        setFailure(
          'The save outcome is still unknown. Your edits and command key are retained; retry to reconcile it.',
        );
      } else if (cause instanceof CommandRequestUnavailableError) {
        setFailure(
          'The re-entered draft does not match the retained private save. Restore the exact original values and choose Reconcile save again; CMA sent nothing.',
        );
      } else if (draftId !== null && cause instanceof ApiError && cause.status === 409) {
        try {
          const current = await readTrigger(draftId);
          setConflict({ current });
          setFailure('This draft changed elsewhere. Your edits are retained.');
        } catch (readCause) {
          setFailure(`${message(cause)} ${message(readCause)}`);
        }
      } else {
        setFailure(message(cause));
      }
      return null;
    } finally {
      setOperation((current) => (current === 'saving' ? null : current));
    }
  }

  async function publish(): Promise<void> {
    const server = publicationDraft;
    if (
      refreshRequired ||
      (publicationCommand === null && (server === null || dirty || !activeConnection))
    ) {
      return;
    }
    const triggerId = publicationCommand?.triggerId ?? server?.id;
    const expectedVersion = publicationCommand?.expectedVersion ?? server?.version;
    const serviceId = publicationCommand?.serviceId ?? editor.selectedServiceId;
    const revisionNumber = publicationCommand?.revisionNumber ?? server?.next_revision_number;
    if (triggerId === undefined || expectedVersion === undefined || revisionNumber === undefined) {
      return;
    }
    setOperation('publishing');
    setFailure(null);
    try {
      const identity = {
        slot: publicationCommand?.slot ?? `trigger:publish:${triggerId}:${String(revisionNumber)}`,
        resource: publicationCommand?.resource ?? `trigger:${triggerId}`,
        action: 'publish-trigger',
      } as const;
      const request = retainedCommandRequest(
        pending,
        identity,
        {
          triggerId,
          expectedVersion,
          serviceId,
        },
        (value) =>
          commandObject(value, {
            triggerId: exactString(triggerId),
            expectedVersion: nonNegativeInteger,
            serviceId: exactString(serviceId),
          }),
      );
      const revision = await executeCommand(
        { ...identity, request, persistRequest: true },
        'Trigger publication',
        (idempotencyKey, frozen) =>
          publishTrigger(frozen.triggerId, frozen.expectedVersion, frozen.serviceId, {
            idempotencyKey,
          }),
      );
      setRevisions((items) => [revision, ...items.filter((item) => item.id !== revision.id)]);
      setStatus(`Revision ${String(revision.content.revision_number)} published`);
      setPublishOpen(false);
      try {
        const refreshed = await readTrigger(triggerId);
        acceptDraft(refreshed, `Revision ${String(revision.content.revision_number)} published`);
      } catch (readCause) {
        setPublishedDraftRefresh({
          triggerId,
          revision,
          failure: message(readCause),
        });
      }
    } catch (cause) {
      setFailure(
        commandOutcomeIsUnresolved(cause)
          ? 'The publish outcome is still unknown. The command key is retained for reconciliation.'
          : message(cause),
      );
    } finally {
      setOperation((current) => (current === 'publishing' ? null : current));
    }
  }

  async function retire(revision: TriggerRevision): Promise<void> {
    const revisionId = retirementCommand?.revisionId ?? revision.id;
    setOperation('retiring');
    setFailure(null);
    try {
      const identity = {
        slot: retirementCommand?.slot ?? `trigger:retire:${revisionId}`,
        resource: retirementCommand?.resource ?? `revision:${revisionId}`,
        action: 'retire-trigger',
      } as const;
      const request = retainedCommandRequest(pending, identity, { revisionId }, (value) =>
        commandObject(value, { revisionId: exactString(revisionId) }),
      );
      const retired = await executeCommand(
        { ...identity, request, persistRequest: true },
        'Trigger retirement',
        (idempotencyKey, frozen) => retireTriggerRevision(frozen.revisionId, { idempotencyKey }),
      );
      setRevisions((items) => items.map((item) => (item.id === retired.id ? retired : item)));
      setRetiringId(null);
      setStatus(`Revision ${String(retired.content.revision_number)} retired`);
    } catch (cause) {
      setFailure(
        commandOutcomeIsUnresolved(cause)
          ? 'The retirement outcome is still unknown. Retry to reconcile the retained command.'
          : message(cause),
      );
    } finally {
      setOperation((current) => (current === 'retiring' ? null : current));
    }
  }

  async function copy(label: string, value: string): Promise<void> {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(label);
      setFailure(null);
    } catch (cause) {
      setFailure(`Copy failed. ${message(cause)}`);
    }
  }

  const setRepositoryOperations = (
    operationClass: TriggerRepositoryOperationClass,
    on: boolean,
  ) => {
    changeContent((content) => ({
      ...content,
      repository_access: {
        ...content.repository_access,
        allowed_operation_classes: on
          ? [...content.repository_access.allowed_operation_classes, operationClass]
          : content.repository_access.allowed_operation_classes.filter(
              (item) => item !== operationClass,
            ),
      },
    }));
  };

  const setEditableField = (field: TriggerEditableField, on: boolean) => {
    changeContent((content) => ({
      ...content,
      override_policy: {
        ...content.override_policy,
        editable_fields: on
          ? [...content.override_policy.editable_fields, field]
          : content.override_policy.editable_fields.filter((item) => item !== field),
      },
    }));
  };

  if (operation === 'loading') {
    return (
      <section className="publisher" aria-label="GitHub Triggers" aria-busy="true">
        <div className="sessionheader publisher__head">
          <h1 ref={heading} tabIndex={-1}>
            GitHub Triggers
          </h1>
        </div>
        <div className="publisher__loading">
          <div className="skeleton" />
          <div className="skeleton" />
          <div className="skeleton" />
        </div>
      </section>
    );
  }

  if (failure !== null && !loaded) {
    return (
      <section className="publisher" aria-label="GitHub Triggers">
        <div className="sessionheader publisher__head">
          <h1 ref={heading} tabIndex={-1}>
            GitHub Triggers
          </h1>
        </div>
        <div className="publisher__fatal" role="alert">
          <IconWarningOutline16 />
          <div>
            <p>{failure}</p>
            <Button
              size="sm"
              icon={<IconRefreshOutline16 />}
              onClick={() => {
                setOperation('loading');
                setFailure(null);
                setReadAttempt((attempt) => attempt + 1);
              }}
            >
              Retry
            </Button>
          </div>
        </div>
      </section>
    );
  }

  return (
    <section className="publisher" aria-label="GitHub Triggers">
      <div className="sessionheader publisher__head">
        <h1 ref={heading} tabIndex={-1}>
          GitHub Triggers
        </h1>
        <Button
          size="sm"
          icon={<IconPlusOutline16 />}
          disabled={pending !== null || refreshRequired}
          onClick={() => {
            chooseEditor(newPublisherEditor(displayIdentity));
          }}
        >
          New Trigger
        </Button>
      </div>

      {publishedDraftRefresh === null ? null : (
        <div className="notice notice--quiet publisher__notice" role="alert">
          <IconWarningOutline16 />
          <div>
            <p className="notice__title">
              Revision {String(publishedDraftRefresh.revision.content.revision_number)} was
              published.
            </p>
            <p className="notice__detail">
              CMA retained the returned immutable revision, but could not refresh the advanced
              mutable draft: {publishedDraftRefresh.failure}
            </p>
            <p className="notice__detail">
              Draft edits and another publication stay disabled until the mutable draft is read
              again.
            </p>
            <Button
              size="sm"
              variant="outline"
              icon={<IconRefreshOutline16 />}
              disabled={operation === 'refreshing'}
              onClick={() => void refreshPublishedDraft()}
            >
              {operation === 'refreshing' ? 'Refreshing draft' : 'Retry draft refresh'}
            </Button>
          </div>
        </div>
      )}
      {failure === null ? null : (
        <div className="notice notice--fail publisher__notice" role="alert">
          <IconWarningOutline16 />
          <div>
            <p className="notice__title">The Trigger was not changed.</p>
            <p className="notice__detail">{failure}</p>
          </div>
        </div>
      )}
      {commandFailure === null ? null : (
        <div className="notice notice--fail publisher__notice" role="alert">
          <IconWarningOutline16 />
          <div>
            <p className="notice__title">Command recovery is unavailable.</p>
            <p className="notice__detail">{commandFailure}</p>
          </div>
        </div>
      )}
      {pending === null ? null : (
        <div className="notice notice--quiet publisher__notice" role="status">
          <IconRefreshOutline16 />
          <div>
            <p className="notice__title">Reconciling {pending.label}</p>
            <p className="notice__detail">
              {reconstructingPrivateSave
                ? 'CMA did not retain the private draft body. Re-enter the exact original values below, then choose Reconcile save. CMA compares the complete request fingerprint and sends nothing unless it matches.'
                : 'Only the retained same-key command can run until CMA returns a finite result.'}
            </p>
          </div>
        </div>
      )}
      {conflict === null ? null : (
        <div className="notice notice--quiet publisher__notice publisher__conflict" role="alert">
          <p className="notice__detail">
            Current server version: {String(conflict.current.version)}
          </p>
          <div className="publisher-actions">
            <Button
              size="sm"
              onClick={() => {
                setCheckedConnection(null);
                setEditor(editorFromDraft(conflict.current));
                setConflict(null);
                setFailure(null);
              }}
            >
              Load current draft
            </Button>
            <Button size="sm" variant="outline" onClick={() => void save(conflict.current.version)}>
              Retry with current version
            </Button>
          </div>
        </div>
      )}

      <div className="publisher__scroll">
        <div className="publisher__layout">
          <aside className="publisher__rail" aria-label="Trigger drafts">
            <span className="publisher__rail-label">Drafts</span>
            <nav className="publisher__drafts">
              {drafts.map((draft) => (
                <button
                  key={draft.id}
                  type="button"
                  className={`publisher__draft${editor.server?.id === draft.id ? ' publisher__draft--on' : ''}`}
                  aria-current={editor.server?.id === draft.id ? 'page' : undefined}
                  disabled={pending !== null || refreshRequired}
                  onClick={() => {
                    chooseEditor(editorFromDraft(draft));
                  }}
                >
                  <strong>{draft.content.task.public_summary}</strong>
                  <span>
                    {draft.content.upstream.repository.canonical_owner}/
                    {draft.content.upstream.repository.canonical_name}
                  </span>
                  <code>Draft v{String(draft.version)}</code>
                </button>
              ))}
              {drafts.length === 0 ? <p>No Trigger drafts yet.</p> : null}
            </nav>
            <label className="publisher__draft-picker">
              <span>Draft</span>
              <select
                value={editor.server?.id ?? 'new'}
                disabled={pending !== null || refreshRequired}
                onChange={(event) => {
                  const selected = drafts.find((draft) => draft.id === event.target.value);
                  chooseEditor(
                    selected === undefined
                      ? newPublisherEditor(displayIdentity)
                      : editorFromDraft(selected),
                  );
                }}
              >
                <option value="new">New Trigger</option>
                {drafts.map((draft) => (
                  <option key={draft.id} value={draft.id}>
                    {draft.content.task.public_summary}
                  </option>
                ))}
              </select>
            </label>
          </aside>

          <div className="publisher__editor">
            <div className="publisher__editor-head">
              <div>
                <p className="eyebrow">{editor.server === null ? 'New draft' : 'Mutable draft'}</p>
                <h2>{editor.content.task.public_summary || 'Untitled Trigger'}</h2>
              </div>
              {editor.server === null ? null : <code>v{String(editor.server.version)}</code>}
            </div>

            {status === null ? null : (
              <p className="publisher__status" role="status">
                <IconCheckOutline16 /> {status}
              </p>
            )}
            <span className="publisher__copy-status" aria-live="polite">
              {copied === null ? '' : `${copied} copied`}
            </span>

            <section className="publisher-section" aria-labelledby="publisher-connection">
              <h3 id="publisher-connection">GitHub connection</h3>
              <div className="publisher-fields publisher-fields--connection">
                <label className="publisher-field">
                  <span>UserService</span>
                  <select
                    className="publisher-select"
                    value={editor.selectedServiceId}
                    disabled={
                      refreshRequired ||
                      (pending !== null &&
                        !(reconstructingPrivateSave && saveCommand?.serviceId === null))
                    }
                    onChange={(event) => {
                      invalidateRepositoryResolution();
                      setCheckedConnection(null);
                      setSelectedConnectionIssue(null);
                      setEditor((current) => ({
                        ...current,
                        selectedServiceId: event.target.value,
                        repository: null,
                      }));
                      setFailure(null);
                      setStatus(null);
                    }}
                  >
                    <option value="">Select a GitHub connection</option>
                    {connections.map((connection) => (
                      <option key={connection.service_id} value={connection.service_id}>
                        {connectionLabel(connection)}
                      </option>
                    ))}
                  </select>
                </label>
                <div className="publisher-connection-actions">
                  {editor.selectedServiceId === '' ? (
                    <Button
                      icon={<IconLinkOutline16 />}
                      disabled={operation === 'connecting' || (pending !== null && !pendingBegin)}
                      onClick={() => void authorize('begin')}
                    >
                      {pendingBegin ? 'Reconcile connection' : 'Connect GitHub'}
                    </Button>
                  ) : reconnectableConnection ? (
                    <Button
                      icon={<IconRefreshOutline16 />}
                      disabled={
                        operation === 'connecting' || (pending !== null && !pendingReconnect)
                      }
                      onClick={() => void authorize('reconnect')}
                    >
                      {pendingReconnect ? 'Reconcile connection' : 'Reconnect'}
                    </Button>
                  ) : (
                    <Button
                      icon={<IconCheckOutline16 />}
                      disabled={operation === 'checking' || pending !== null}
                      onClick={() => void checkConnection()}
                    >
                      Check connection
                    </Button>
                  )}
                  {editor.selectedServiceId === '' ? null : (
                    <Button
                      size="sm"
                      icon={<IconPlusOutline16 />}
                      disabled={operation === 'connecting' || (pending !== null && !pendingBegin)}
                      onClick={() => void authorize('begin')}
                    >
                      {pendingBegin ? 'Reconcile connection' : 'Connect another'}
                    </Button>
                  )}
                  {selectedConnection?.state === 'active' || pendingRevoke ? (
                    <Button
                      size="sm"
                      variant="danger"
                      icon={<IconTrashOutline16 />}
                      disabled={
                        busy ||
                        (revokeServiceId === null && editor.selectedServiceId === '') ||
                        (pending !== null && !pendingRevoke)
                      }
                      onClick={() =>
                        setConfirmingRevoke(revokeServiceId ?? editor.selectedServiceId)
                      }
                    >
                      {pendingRevoke ? 'Reconcile revocation' : 'Revoke connection'}
                    </Button>
                  ) : null}
                </div>
              </div>
              {inactiveConnection ? (
                <p className="publisher-connection-guidance">
                  {INACTIVE_TRIGGER_CONNECTION_GUIDANCE}
                </p>
              ) : null}
              {selectedConnection === null ? null : (
                <dl className="publisher-facts">
                  <div>
                    <dt>Account</dt>
                    <dd>{selectedConnection.account_login ?? 'Not connected'}</dd>
                  </div>
                  <div>
                    <dt>State</dt>
                    <dd>{activeConnection ? 'Verified active' : selectedConnection.state}</dd>
                  </div>
                  <div>
                    <dt>Scopes</dt>
                    <dd>{selectedConnection.granted_scopes.join(', ') || 'None observed'}</dd>
                  </div>
                </dl>
              )}
            </section>

            <fieldset
              className="publisher__form"
              disabled={refreshRequired || (pending !== null && !reconstructingPrivateSave)}
            >
              <legend className="sr-only">Trigger configuration</legend>
              <section className="publisher-section" aria-labelledby="publisher-repository">
                <h3 id="publisher-repository">Repository</h3>
                <div className="publisher-repository-row">
                  <label className="publisher-field">
                    <span>GitHub repository URL</span>
                    <Input
                      type="url"
                      icon={<IconBranchOutline16 />}
                      placeholder="https://github.com/owner/repository"
                      value={editor.repositoryUrl}
                      onChange={(event) => {
                        const repositoryUrl = event.target.value;
                        invalidateRepositoryResolution();
                        setEditor((current) => ({ ...current, repositoryUrl, repository: null }));
                        setFailure(null);
                        setStatus(null);
                      }}
                    />
                  </label>
                  <Button
                    variant="outline"
                    disabled={!activeConnection || operation === 'resolving'}
                    onClick={() => void resolveRepository()}
                  >
                    Verify repository
                  </Button>
                </div>
                {editor.repository === null ? null : (
                  <dl className="publisher-facts publisher-facts--repository">
                    <div>
                      <dt>Canonical repository</dt>
                      <dd>
                        {editor.repository.repository.canonical_owner}/
                        {editor.repository.repository.canonical_name}
                      </dd>
                    </div>
                    <div>
                      <dt>Default branch</dt>
                      <dd>{editor.repository.repository.default_branch}</dd>
                    </div>
                    <div>
                      <dt>Source revision</dt>
                      <dd title={editor.repository.source_sha}>
                        {short(editor.repository.source_sha, 12)}
                      </dd>
                    </div>
                  </dl>
                )}
                <div className="publisher-fields publisher-fields--three">
                  <label className="publisher-field">
                    <span>Source mode</span>
                    <select
                      className="publisher-select"
                      value={editor.content.repository.source_mode}
                      onChange={(event) => {
                        changeContent((content) => ({
                          ...content,
                          repository: {
                            ...content.repository,
                            source_mode: event.target
                              .value as TriggerContentDraft['repository']['source_mode'],
                          },
                        }));
                      }}
                    >
                      <option value="pinned_commit">Pinned commit</option>
                      <option value="branch_head_at_launch">Branch head at launch</option>
                    </select>
                  </label>
                  <label className="publisher-field">
                    <span>Selected branch</span>
                    <Input
                      value={editor.content.repository.selected_ref}
                      onChange={(event) => {
                        changeContent((content) => ({
                          ...content,
                          repository: { ...content.repository, selected_ref: event.target.value },
                        }));
                      }}
                    />
                  </label>
                  <label className="publisher-field">
                    <span>Contribution base</span>
                    <Input
                      value={editor.content.repository.contribution_base_ref}
                      onChange={(event) => {
                        changeContent((content) => ({
                          ...content,
                          repository: {
                            ...content.repository,
                            contribution_base_ref: event.target.value,
                          },
                        }));
                      }}
                    />
                  </label>
                </div>
              </section>

              <section className="publisher-section" aria-labelledby="publisher-task">
                <h3 id="publisher-task">Task</h3>
                <div className="publisher-fields">
                  <label className="publisher-field">
                    <span>Publisher display identity</span>
                    <Input
                      value={editor.content.publisher_display_identity}
                      onChange={(event) => {
                        changeContent((content) => ({
                          ...content,
                          publisher_display_identity: event.target.value,
                        }));
                      }}
                    />
                  </label>
                  <label className="publisher-field">
                    <span>Public summary</span>
                    <Input
                      value={editor.content.task.public_summary}
                      onChange={(event) => {
                        changeContent((content) => ({
                          ...content,
                          task: { ...content.task, public_summary: event.target.value },
                        }));
                      }}
                    />
                  </label>
                </div>
                <label className="publisher-field">
                  <span>Prompt template</span>
                  <textarea
                    className="field__input publisher-textarea publisher-textarea--prompt"
                    value={editor.content.task.prompt_template}
                    onChange={(event) => {
                      changeContent((content) => ({
                        ...content,
                        task: { ...content.task, prompt_template: event.target.value },
                      }));
                    }}
                  />
                </label>
                <TaskInputEditor
                  fields={editor.content.task.input_fields}
                  validation={validation}
                  onChange={(inputFields) => {
                    changeContent((content) => ({
                      ...content,
                      task: { ...content.task, input_fields: inputFields },
                    }));
                  }}
                />
              </section>

              <section className="publisher-section" aria-labelledby="publisher-session">
                <h3 id="publisher-session">Session</h3>
                <div className="publisher-fields">
                  <div className="publisher-field">
                    <span>Permission ceiling</span>
                    <div
                      className="publisher-segmented"
                      role="group"
                      aria-label="Permission ceiling"
                    >
                      {(['guarded', 'autonomous'] as const).map((permission) => (
                        <button
                          key={permission}
                          type="button"
                          aria-pressed={editor.content.session.permission_ceiling === permission}
                          onClick={() => {
                            changeContent((content) => ({
                              ...content,
                              session: { ...content.session, permission_ceiling: permission },
                            }));
                          }}
                        >
                          {permission === 'guarded' ? 'Guarded' : 'Autonomous'}
                        </button>
                      ))}
                    </div>
                  </div>
                  <label className="publisher-field">
                    <span>Sandbox profile</span>
                    <Input
                      value={editor.content.session.sandbox_profile}
                      onChange={(event) => {
                        changeContent((content) => ({
                          ...content,
                          session: { ...content.session, sandbox_profile: event.target.value },
                        }));
                      }}
                    />
                  </label>
                </div>
                <div className="publisher-fields publisher-fields--four">
                  {(
                    [
                      ['Default CPU (m)', 'default', 'cpu_millis'],
                      ['Default memory (MiB)', 'default', 'memory_mib'],
                      ['CPU ceiling (m)', 'ceiling', 'cpu_millis'],
                      ['Memory ceiling (MiB)', 'ceiling', 'memory_mib'],
                    ] as const
                  ).map(([label, tier, key]) => (
                    <label className="publisher-field" key={`${tier}:${key}`}>
                      <span>{label}</span>
                      <Input
                        type="number"
                        min={1}
                        value={editor.content.session.resources[tier][key]}
                        onChange={(event) => {
                          changeContent((content) => ({
                            ...content,
                            session: {
                              ...content.session,
                              resources: {
                                ...content.session.resources,
                                [tier]: {
                                  ...content.session.resources[tier],
                                  [key]: inputNumber(event),
                                },
                              },
                            },
                          }));
                        }}
                      />
                    </label>
                  ))}
                </div>
                <div className="publisher-fields">
                  <label className="publisher-field">
                    <span>Default timeout (seconds)</span>
                    <Input
                      type="number"
                      min={1}
                      max={86400}
                      value={editor.content.session.timeout_default_seconds}
                      onChange={(event) => {
                        changeContent((content) => ({
                          ...content,
                          session: {
                            ...content.session,
                            timeout_default_seconds: inputNumber(event),
                          },
                        }));
                      }}
                    />
                  </label>
                  <label className="publisher-field">
                    <span>Timeout ceiling (seconds)</span>
                    <Input
                      type="number"
                      min={1}
                      max={86400}
                      value={editor.content.session.timeout_ceiling_seconds}
                      onChange={(event) => {
                        changeContent((content) => ({
                          ...content,
                          session: {
                            ...content.session,
                            timeout_ceiling_seconds: inputNumber(event),
                          },
                        }));
                      }}
                    />
                  </label>
                </div>
                <ul className="publisher-fixed-list" aria-label="Required session capabilities">
                  {REQUIRED_CAPABILITIES.map((capability) => (
                    <li key={capability}>
                      <IconCheckOutline16 /> {capability.replaceAll('_', ' ')}
                    </li>
                  ))}
                </ul>
              </section>

              <section className="publisher-section" aria-labelledby="publisher-access">
                <h3 id="publisher-access">GitHub access</h3>
                <dl className="publisher-fixed">
                  <div>
                    <dt>Credential route</dt>
                    <dd>Contributor connection</dd>
                  </div>
                  <div>
                    <dt>Required scope class</dt>
                    <dd>Public repository</dd>
                  </div>
                  <div>
                    <dt>Setup executor</dt>
                    <dd>Disabled</dd>
                  </div>
                </dl>
                <fieldset className="publisher-checks">
                  <legend>Allowed operation classes</legend>
                  {REPOSITORY_OPERATIONS.map((operationClass) => (
                    <label className="publisher-toggle" key={operationClass}>
                      <input
                        type="checkbox"
                        checked={editor.content.repository_access.allowed_operation_classes.includes(
                          operationClass,
                        )}
                        onChange={(event) => {
                          setRepositoryOperations(operationClass, event.target.checked);
                        }}
                      />
                      <span>{operationClass.replaceAll('_', ' ')}</span>
                    </label>
                  ))}
                </fieldset>
                <label className="publisher-field publisher-field--compact">
                  <span>Workflow changes</span>
                  <select
                    className="publisher-select"
                    value={editor.content.repository_access.workflow_change_policy}
                    onChange={(event) => {
                      changeContent((content) => ({
                        ...content,
                        repository_access: {
                          ...content.repository_access,
                          workflow_change_policy: event.target
                            .value as TriggerContentDraft['repository_access']['workflow_change_policy'],
                        },
                      }));
                    }}
                  >
                    <option value="refuse">Refuse</option>
                    <option value="require_scope_and_confirmation">
                      Require scope and confirmation
                    </option>
                  </select>
                </label>
              </section>

              <section className="publisher-section" aria-labelledby="publisher-contribution">
                <h3 id="publisher-contribution">Contribution</h3>
                <dl className="publisher-fixed">
                  <div>
                    <dt>Branch prefix</dt>
                    <dd>
                      <code>cma/</code>
                    </dd>
                  </div>
                  <div>
                    <dt>Commit attribution</dt>
                    <dd>Contributor no-reply identity</dd>
                  </div>
                  <div>
                    <dt>Billing</dt>
                    <dd>Zero cost</dd>
                  </div>
                </dl>
                <label className="publisher-field">
                  <span>Pull-request title template</span>
                  <Input
                    value={editor.content.contribution.pull_request.title_template}
                    onChange={(event) => {
                      changeContent((content) => ({
                        ...content,
                        contribution: {
                          ...content.contribution,
                          pull_request: {
                            ...content.contribution.pull_request,
                            title_template: event.target.value,
                          },
                        },
                      }));
                    }}
                  />
                </label>
                <label className="publisher-field">
                  <span>Pull-request body template</span>
                  <textarea
                    className="field__input publisher-textarea"
                    value={editor.content.contribution.pull_request.body_template}
                    onChange={(event) => {
                      changeContent((content) => ({
                        ...content,
                        contribution: {
                          ...content.contribution,
                          pull_request: {
                            ...content.contribution.pull_request,
                            body_template: event.target.value,
                          },
                        },
                      }));
                    }}
                  />
                </label>
                <p className="publisher-unavailable">
                  Sponsored credentials and billing · Unavailable in this release
                </p>
              </section>

              <section className="publisher-section" aria-labelledby="publisher-overrides">
                <h3 id="publisher-overrides">Contributor overrides</h3>
                <fieldset className="publisher-checks publisher-checks--overrides">
                  <legend>Editable fields</legend>
                  {EDITABLE_FIELDS.map((field) => (
                    <label className="publisher-toggle" key={field}>
                      <input
                        type="checkbox"
                        checked={editor.content.override_policy.editable_fields.includes(field)}
                        onChange={(event) => {
                          setEditableField(field, event.target.checked);
                        }}
                      />
                      <span>{field.replaceAll('_', ' ')}</span>
                    </label>
                  ))}
                </fieldset>
              </section>
            </fieldset>

            <div className="publisher-savebar">
              <span>
                {pending !== null
                  ? reconstructingPrivateSave
                    ? 'Re-enter the exact private draft'
                    : `Reconciling ${pending.label}`
                  : dirty
                    ? 'Unsaved changes'
                    : 'Draft saved'}
              </span>
              <div className="publisher-actions">
                <Button
                  variant="outline"
                  disabled={
                    (!dirty && !reconstructingPrivateSave) ||
                    busy ||
                    refreshRequired ||
                    (pending !== null && !pendingSave)
                  }
                  onClick={() => void save()}
                >
                  {operation === 'saving'
                    ? 'Saving'
                    : pendingSave
                      ? 'Reconcile save'
                      : 'Save draft'}
                </Button>
                <Button
                  variant="primary"
                  disabled={
                    editor.server === null ||
                    dirty ||
                    !activeConnection ||
                    busy ||
                    pending !== null ||
                    refreshRequired
                  }
                  onClick={() => {
                    setPublishOpen(true);
                  }}
                >
                  Publish
                </Button>
              </div>
            </div>

            {editor.server === null ? null : (
              <section
                className="publisher-section publisher-revisions"
                aria-labelledby="revisions-heading"
              >
                <h3 id="revisions-heading">Immutable revisions</h3>
                {currentRevisions.length === 0 ? (
                  <p className="publisher-revisions__empty">No revisions published yet.</p>
                ) : (
                  <ol>
                    {currentRevisions.map((revision) => (
                      <li key={revision.id}>
                        <div className="publisher-revision__facts">
                          <strong>Revision {String(revision.content.revision_number)}</strong>
                          <span>{revision.availability}</span>
                          <code title={revision.digest}>{short(revision.digest, 12)}</code>
                          <time dateTime={revision.published_at}>
                            {new Date(revision.published_at).toLocaleString()}
                          </time>
                          <span>{revision.authority.account_login}</span>
                        </div>
                        {retiringId === revision.id ||
                        retirementCommand?.revisionId === revision.id ? (
                          <InlineConfirm
                            question={`Retire revision ${String(revision.content.revision_number)}? Existing Launches continue; new Launches stop.`}
                            confirmLabel={
                              retirementCommand?.revisionId === revision.id
                                ? 'Reconcile retirement'
                                : 'Retire revision'
                            }
                            busy={
                              operation === 'retiring' ||
                              (pending !== null &&
                                pending?.slot !== `trigger:retire:${revision.id}`)
                            }
                            cancelDisabled={retirementCommand?.revisionId === revision.id}
                            onCancel={() => {
                              setRetiringId(null);
                            }}
                            onConfirm={() => void retire(revision)}
                          />
                        ) : (
                          <div className="publisher-revision__actions">
                            <Button
                              size="sm"
                              aria-label={`Copy public URL for revision ${String(revision.content.revision_number)}`}
                              title="Copy public Trigger URL"
                              icon={<IconLinkOutline16 />}
                              onClick={() => void copy('Public Trigger URL', publicUrl(revision))}
                            />
                            <Button
                              size="sm"
                              aria-label={`Copy README Markdown for revision ${String(revision.content.revision_number)}`}
                              title="Copy README Markdown"
                              icon={<IconCopyOutline16 />}
                              onClick={() => void copy('README Markdown', readmeLink(revision))}
                            />
                            {revision.availability === 'published' ? (
                              <Button
                                size="sm"
                                variant="danger"
                                aria-label={`Retire revision ${String(revision.content.revision_number)}`}
                                title="Retire revision"
                                icon={<IconTrashOutline16 />}
                                disabled={pending !== null || refreshRequired}
                                onClick={() => {
                                  setRetiringId(revision.id);
                                }}
                              />
                            ) : null}
                          </div>
                        )}
                      </li>
                    ))}
                  </ol>
                )}
              </section>
            )}
          </div>
        </div>
      </div>

      <Modal
        open={confirmingRevoke !== null || pendingRevoke}
        title="Revoke GitHub connection?"
        description="CMA will deactivate this exact NyxID connection. Trigger drafts, published revisions, Launches, workspaces, forks, branches, commits, pull requests, and sessions are not deleted."
        dismissible={operation !== 'revoking' && !pendingRevoke}
        onClose={() => {
          if (!busy && !pendingRevoke) setConfirmingRevoke(null);
        }}
        footer={
          <>
            <Button disabled={busy || pendingRevoke} onClick={() => setConfirmingRevoke(null)}>
              Keep connection
            </Button>
            <Button
              variant="danger"
              disabled={busy || revokeServiceId === null || (pending !== null && !pendingRevoke)}
              onClick={() => void revokeConnection(revokeServiceId)}
            >
              {operation === 'revoking'
                ? pendingRevoke
                  ? 'Reconciling'
                  : 'Submitting'
                : pendingRevoke
                  ? 'Reconcile revocation'
                  : 'Revoke connection'}
            </Button>
          </>
        }
      >
        <p>
          {revokeConnectionAccount === null
            ? 'Affected Launches will require another eligible GitHub connection before more GitHub work can continue.'
            : `${revokeConnectionAccount} will no longer be eligible for new CMA GitHub work.`}{' '}
          Provider access may remain usable until revocation is acknowledged or the bounded
          credential lifetime expires.
        </p>
      </Modal>

      <Modal
        open={publishOpen || pendingPublish}
        title="Publish immutable revision"
        description="Publishing creates a new public README destination. This revision cannot be edited."
        onClose={() => {
          if (!busy && !pendingPublish) setPublishOpen(false);
        }}
        footer={
          <>
            <Button
              variant="outline"
              disabled={busy || pendingPublish}
              onClick={() => {
                setPublishOpen(false);
              }}
            >
              Keep editing
            </Button>
            <Button
              variant="primary"
              disabled={busy || refreshRequired || (pending !== null && !pendingPublish)}
              onClick={() => void publish()}
            >
              {pendingPublish ? 'Reconcile publication' : 'Publish immutable revision'}
            </Button>
          </>
        }
      >
        <dl className="publisher-publish-facts">
          <div>
            <dt>Revision</dt>
            <dd>
              {String(
                publicationCommand?.revisionNumber ??
                  publicationEditor.server?.next_revision_number ??
                  1,
              )}
            </dd>
          </div>
          <div>
            <dt>Repository</dt>
            <dd>
              {publicationEditor.repository === null
                ? 'Unverified'
                : canonicalRepositoryUrl(publicationEditor.repository)}
            </dd>
          </div>
          <div>
            <dt>Source mode</dt>
            <dd>{publicationEditor.content.repository.source_mode.replaceAll('_', ' ')}</dd>
          </div>
          <div>
            <dt>Summary</dt>
            <dd>{publicationEditor.content.task.public_summary}</dd>
          </div>
        </dl>
      </Modal>

      <Modal
        open={discardEditor !== null}
        title="Discard unsaved draft changes?"
        description="The selected draft has not been changed on the server."
        onClose={() => {
          setDiscardEditor(null);
        }}
        footer={
          <>
            <Button
              variant="outline"
              onClick={() => {
                setDiscardEditor(null);
              }}
            >
              Keep editing
            </Button>
            <Button
              variant="danger"
              onClick={() => {
                if (discardEditor !== null) {
                  setCheckedConnection(null);
                  setEditor(discardEditor);
                }
                setDiscardEditor(null);
                setConflict(null);
                setFailure(null);
                setStatus(null);
              }}
            >
              Discard changes
            </Button>
          </>
        }
      />
    </section>
  );
}
