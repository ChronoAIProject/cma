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
  TransportError,
  beginTriggerConnection,
  createTrigger,
  listTriggerConnections,
  listTriggerRevisions,
  listTriggers,
  preflightTriggerConnection,
  publishTrigger,
  readTrigger,
  reconnectTriggerConnection,
  resolveTriggerRepository,
  retireTriggerRevision,
  updateTrigger,
} from '../api/client';
import { newIntentKey } from '../trigger/idempotency';
import { authorizeTriggerConnection, isActiveTriggerConnection } from '../trigger/connectionFlow';
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
  | 'resolving'
  | 'saving'
  | 'publishing'
  | 'retiring'
  | 'reconciling';

interface Conflict {
  readonly current: TriggerDraft;
}

interface PublisherSurfaceProps {
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

export function PublisherSurface({ displayIdentity }: PublisherSurfaceProps) {
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
  const [retiringId, setRetiringId] = useState<string | null>(null);
  const [discardEditor, setDiscardEditor] = useState<PublisherEditor | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  const [readAttempt, setReadAttempt] = useState(0);
  const [loaded, setLoaded] = useState(false);
  const commandKeys = useRef(new Map<string, string>());
  const authorization = useRef<AbortController | null>(null);
  const heading = useRef<HTMLHeadingElement>(null);

  const validation = useMemo(() => validatePublisherEditor(editor), [editor]);
  const dirty = editorIsDirty(editor);
  const busy = operation !== 'loading' && operation !== null;
  const selectedConnection =
    connections.find((connection) => connection.service_id === editor.selectedServiceId) ?? null;
  const activeConnection =
    checkedConnection?.service_id === editor.selectedServiceId &&
    isActiveTriggerConnection(checkedConnection);

  const currentRevisions = useMemo(() => {
    if (editor.server === null) return [];
    return revisions
      .filter((revision) => revision.content.trigger_id === editor.server?.id)
      .sort((left, right) => right.content.revision_number - left.content.revision_number);
  }, [editor.server, revisions]);

  useEffect(() => {
    heading.current?.focus();
  }, []);

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
    const serviceId = editor.selectedServiceId;
    if (serviceId === '') return;
    const controller = new AbortController();
    void preflightTriggerConnection(serviceId, controller.signal)
      .then((connection) => {
        setCheckedConnection(connection);
        setConnections((items) => [
          connection,
          ...items.filter((item) => item.service_id !== connection.service_id),
        ]);
      })
      .catch((cause: unknown) => {
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
    },
    [],
  );

  const changeContent = useCallback(
    (change: (content: TriggerContentDraft) => TriggerContentDraft) => {
      setEditor((current) => ({ ...current, content: change(current.content) }));
      setConflict(null);
      setFailure(null);
      setStatus(null);
    },
    [],
  );

  async function command<T>(slot: string, send: (key: string) => Promise<T>): Promise<T> {
    const retained = commandKeys.current.get(slot) ?? newIntentKey();
    commandKeys.current.set(slot, retained);
    let finite = false;
    try {
      try {
        const result = await send(retained);
        finite = true;
        return result;
      } catch (cause) {
        if (!(cause instanceof TransportError)) {
          finite = true;
          throw cause;
        }
        setOperation('reconciling');
        try {
          const result = await send(retained);
          finite = true;
          return result;
        } catch (reconciled) {
          if (!(reconciled instanceof TransportError)) finite = true;
          throw reconciled;
        }
      }
    } finally {
      if (finite) commandKeys.current.delete(slot);
    }
  }

  function chooseEditor(next: PublisherEditor): void {
    if (dirty) {
      setDiscardEditor(next);
      return;
    }
    setCheckedConnection(null);
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
    authorization.current?.abort();
    const controller = new AbortController();
    authorization.current = controller;
    setOperation('connecting');
    setFailure(null);
    setStatus('Waiting for GitHub authorization');
    try {
      const intentId = crypto.randomUUID();
      const connection = await authorizeTriggerConnection({
        signal: controller.signal,
        begin: () =>
          mode === 'begin'
            ? command(`connection:begin:${intentId}`, (idempotencyKey) =>
                beginTriggerConnection(intentId, { idempotencyKey }),
              )
            : command(`connection:reconnect:${editor.selectedServiceId}`, (idempotencyKey) =>
                reconnectTriggerConnection(editor.selectedServiceId, { idempotencyKey }),
              ),
        preflight: preflightTriggerConnection,
        onService: (serviceId) => {
          setCheckedConnection(null);
          setEditor((current) => ({
            ...current,
            selectedServiceId: serviceId,
            repository: null,
          }));
        },
      });
      setCheckedConnection(connection);
      await refreshConnections();
      setStatus(`Connected as ${connection.account_login ?? 'GitHub user'}`);
    } catch (cause) {
      if (!controller.signal.aborted) setFailure(message(cause));
    } finally {
      if (authorization.current === controller) authorization.current = null;
      setOperation((current) =>
        current === 'connecting' || current === 'reconciling' ? null : current,
      );
    }
  }

  async function checkConnection(): Promise<void> {
    if (editor.selectedServiceId === '') return;
    setOperation('checking');
    setFailure(null);
    try {
      const connection = await preflightTriggerConnection(editor.selectedServiceId);
      setCheckedConnection(connection);
      await refreshConnections();
      setStatus(`Connection verified for ${connection.account_login ?? 'GitHub user'}`);
    } catch (cause) {
      setCheckedConnection(null);
      setFailure(message(cause));
    } finally {
      setOperation((current) => (current === 'checking' ? null : current));
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
    setOperation('resolving');
    setFailure(null);
    try {
      const selection = await resolveTriggerRepository(
        editor.selectedServiceId,
        locator.owner,
        locator.name,
      );
      setEditor((current) => applyRepository(current, selection));
      setStatus(
        `Verified ${selection.repository.canonical_owner}/${selection.repository.canonical_name}`,
      );
    } catch (cause) {
      setFailure(message(cause));
    } finally {
      setOperation((current) => (current === 'resolving' ? null : current));
    }
  }

  function acceptDraft(saved: TriggerDraft, notice: string): void {
    setDrafts((items) => replaceDraft(items, saved));
    setEditor(editorFromDraft(saved));
    setConflict(null);
    setFailure(null);
    setStatus(notice);
  }

  async function save(expectedVersion?: number): Promise<TriggerDraft | null> {
    const problem = firstValidationMessage(validation);
    if (problem !== undefined) {
      setFailure(problem);
      return null;
    }
    if (!activeConnection) {
      setFailure('Check the selected GitHub connection before saving.');
      return null;
    }
    setOperation('saving');
    setFailure(null);
    const server = editor.server;
    const slot = server === null ? 'trigger:create' : `trigger:update:${server.id}`;
    try {
      const saved = await command(slot, (idempotencyKey) =>
        server === null
          ? createTrigger(editor.selectedServiceId, editor.content, { idempotencyKey })
          : updateTrigger(
              server.id,
              expectedVersion ?? server.version,
              editor.selectedServiceId,
              editor.content,
              { idempotencyKey },
            ),
      );
      acceptDraft(saved, server === null ? 'Trigger draft created' : 'Draft saved');
      return saved;
    } catch (cause) {
      if (server !== null && cause instanceof ApiError && cause.status === 409) {
        try {
          const current = await readTrigger(server.id);
          setConflict({ current });
          setFailure('This draft changed elsewhere. Your edits are retained.');
        } catch (readCause) {
          setFailure(`${message(cause)} ${message(readCause)}`);
        }
      } else {
        setFailure(
          cause instanceof TransportError
            ? 'The save outcome is still unknown. Your edits and command key are retained; retry to reconcile it.'
            : message(cause),
        );
      }
      return null;
    } finally {
      setOperation((current) =>
        current === 'saving' || current === 'reconciling' ? null : current,
      );
    }
  }

  async function publish(): Promise<void> {
    const server = editor.server;
    if (server === null || dirty || !activeConnection) return;
    setOperation('publishing');
    setFailure(null);
    try {
      const revision = await command(
        `trigger:publish:${server.id}:${String(server.next_revision_number)}`,
        (idempotencyKey) =>
          publishTrigger(server.id, server.version, editor.selectedServiceId, { idempotencyKey }),
      );
      const refreshed = await readTrigger(server.id);
      setRevisions((items) => [revision, ...items.filter((item) => item.id !== revision.id)]);
      acceptDraft(refreshed, `Revision ${String(revision.content.revision_number)} published`);
      setPublishOpen(false);
    } catch (cause) {
      setFailure(
        cause instanceof TransportError
          ? 'The publish outcome is still unknown. The command key is retained for reconciliation.'
          : message(cause),
      );
    } finally {
      setOperation((current) =>
        current === 'publishing' || current === 'reconciling' ? null : current,
      );
    }
  }

  async function retire(revision: TriggerRevision): Promise<void> {
    setOperation('retiring');
    setFailure(null);
    try {
      const retired = await command(`trigger:retire:${revision.id}`, (idempotencyKey) =>
        retireTriggerRevision(revision.id, { idempotencyKey }),
      );
      setRevisions((items) => items.map((item) => (item.id === retired.id ? retired : item)));
      setRetiringId(null);
      setStatus(`Revision ${String(retired.content.revision_number)} retired`);
    } catch (cause) {
      setFailure(
        cause instanceof TransportError
          ? 'The retirement outcome is still unknown. Retry to reconcile the retained command.'
          : message(cause),
      );
    } finally {
      setOperation((current) =>
        current === 'retiring' || current === 'reconciling' ? null : current,
      );
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
          onClick={() => {
            chooseEditor(newPublisherEditor(displayIdentity));
          }}
        >
          New Trigger
        </Button>
      </div>

      {failure === null ? null : (
        <div className="notice notice--fail publisher__notice" role="alert">
          <IconWarningOutline16 />
          <div>
            <p className="notice__title">The Trigger was not changed.</p>
            <p className="notice__detail">{failure}</p>
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
                    onChange={(event) => {
                      setCheckedConnection(null);
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
                      disabled={operation === 'connecting'}
                      onClick={() => void authorize('begin')}
                    >
                      Connect GitHub
                    </Button>
                  ) : selectedConnection?.state !== 'active' ? (
                    <Button
                      icon={<IconRefreshOutline16 />}
                      disabled={operation === 'connecting'}
                      onClick={() => void authorize('reconnect')}
                    >
                      Reconnect
                    </Button>
                  ) : (
                    <Button
                      icon={<IconCheckOutline16 />}
                      disabled={operation === 'checking'}
                      onClick={() => void checkConnection()}
                    >
                      Check connection
                    </Button>
                  )}
                  <Button
                    size="sm"
                    icon={<IconPlusOutline16 />}
                    disabled={operation === 'connecting'}
                    onClick={() => void authorize('begin')}
                  >
                    Connect another
                  </Button>
                </div>
              </div>
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
                  <div className="publisher-segmented" role="group" aria-label="Permission ceiling">
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
              <label className="publisher-field">
                <span>Optional check identifiers, one per line</span>
                <textarea
                  className="field__input publisher-textarea publisher-textarea--short"
                  value={editor.content.override_policy.optional_checks.join('\n')}
                  onChange={(event) => {
                    changeContent((content) => ({
                      ...content,
                      override_policy: {
                        ...content.override_policy,
                        optional_checks: event.target.value.split('\n').filter(Boolean),
                      },
                    }));
                  }}
                />
              </label>
            </section>

            <div className="publisher-savebar">
              <span>
                {operation === 'reconciling'
                  ? 'Reconciling'
                  : dirty
                    ? 'Unsaved changes'
                    : 'Draft saved'}
              </span>
              <div className="publisher-actions">
                <Button variant="outline" disabled={!dirty || busy} onClick={() => void save()}>
                  {operation === 'saving' ? 'Saving' : 'Save draft'}
                </Button>
                <Button
                  variant="primary"
                  disabled={editor.server === null || dirty || !activeConnection || busy}
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
                        {retiringId === revision.id ? (
                          <InlineConfirm
                            question={`Retire revision ${String(revision.content.revision_number)}? Existing Launches continue; new Launches stop.`}
                            confirmLabel="Retire revision"
                            busy={operation === 'retiring' || operation === 'reconciling'}
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
        open={publishOpen}
        title="Publish immutable revision"
        description="Publishing creates a new public README destination. This revision cannot be edited."
        onClose={() => {
          if (!busy) setPublishOpen(false);
        }}
        footer={
          <>
            <Button
              variant="outline"
              disabled={busy}
              onClick={() => {
                setPublishOpen(false);
              }}
            >
              Keep editing
            </Button>
            <Button variant="primary" disabled={busy} onClick={() => void publish()}>
              Publish immutable revision
            </Button>
          </>
        }
      >
        <dl className="publisher-publish-facts">
          <div>
            <dt>Revision</dt>
            <dd>{String(editor.server?.next_revision_number ?? 1)}</dd>
          </div>
          <div>
            <dt>Repository</dt>
            <dd>
              {editor.repository === null
                ? 'Unverified'
                : canonicalRepositoryUrl(editor.repository)}
            </dd>
          </div>
          <div>
            <dt>Source mode</dt>
            <dd>{editor.content.repository.source_mode.replaceAll('_', ' ')}</dd>
          </div>
          <div>
            <dt>Summary</dt>
            <dd>{editor.content.task.public_summary}</dd>
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
