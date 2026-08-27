import { useEffect, useMemo, useRef, useState } from 'react';
import type { JsonValue, Launch, LaunchOverrides, TriggerConnection } from '@chronoai/cma-sdk';
import {
  ApiError,
  TransportError,
  beginTriggerConnection,
  createOrResumeLaunch,
  listTriggerConnections,
  preflightTriggerConnection,
  reconnectTriggerConnection,
  selectLaunchConnection,
  startLaunch,
  updateLaunchOverrides,
} from '../api/client';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { Modal } from '../ui/Modal';
import {
  IconBranchOutline16,
  IconCheckOutline16,
  IconLinkOutline16,
  IconWarningOutline16,
} from '../ui/icons';
import type { AppRouter } from '../routing/routes';
import { authorizeTriggerConnection, isActiveTriggerConnection } from './connectionFlow';
import {
  editableFields,
  jsonText,
  optionalCheckChoices,
  shortRevision,
  taskFields,
  upstreamRepository,
} from './model';
import { useIdempotentCommand } from './useIdempotentCommand';
import './Trigger.css';

interface TriggerReviewProps {
  readonly publicId: string;
  readonly navigate: AppRouter['navigate'];
}

type TaskValues = Readonly<Record<string, JsonValue>>;

interface ReviewSnapshot {
  readonly selectedService: string;
  readonly overrides: LaunchOverrides;
  readonly workflowAcknowledged: boolean;
}

function message(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

function initialTaskValues(launch: Launch): TaskValues {
  const values: Record<string, JsonValue> = {};
  for (const field of taskFields(launch)) {
    const override = launch.overrides.task_inputs[field.key];
    const value = override ?? field.defaultValue;
    if (value !== undefined) values[field.key] = value;
  }
  return values;
}

function initialOverrides(launch: Launch): LaunchOverrides {
  return {
    task_inputs: initialTaskValues(launch),
    branch_suffix: launch.overrides.branch_suffix ?? launch.effective.branch_suffix,
    resource_size: launch.overrides.resource_size ?? launch.effective.resource_size,
    timeout_seconds: launch.overrides.timeout_seconds ?? launch.effective.timeout_seconds,
    optional_checks: launch.overrides.optional_checks,
  };
}

function equalOverrides(left: LaunchOverrides, right: LaunchOverrides): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

function connectionReady(connection: TriggerConnection | null): boolean {
  return connection !== null && connection.state === 'active' && connection.account_id !== null;
}

export function TriggerReview({ publicId, navigate }: TriggerReviewProps) {
  const [launch, setLaunch] = useState<Launch | null>(null);
  const [connections, setConnections] = useState<readonly TriggerConnection[]>([]);
  const [selectedService, setSelectedService] = useState('');
  const [preflight, setPreflight] = useState<TriggerConnection | null>(null);
  const [overrides, setOverrides] = useState<LaunchOverrides | null>(null);
  const [workflowAcknowledged, setWorkflowAcknowledged] = useState(false);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState<'saving' | 'starting' | 'connecting' | 'checking' | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [readAttempt, setReadAttempt] = useState(0);
  const heading = useRef<HTMLHeadingElement>(null);
  const authorization = useRef<AbortController | null>(null);
  const { execute: executeCommand, pending } = useIdempotentCommand();

  useEffect(() => {
    heading.current?.focus();
  }, [publicId]);

  useEffect(() => {
    let live = true;
    void Promise.all([
      executeCommand(`trigger:resume:${publicId}`, 'Launch creation', (idempotencyKey) =>
        createOrResumeLaunch(publicId, { idempotencyKey }),
      ),
      listTriggerConnections(),
    ])
      .then(([current, available]) => {
        if (!live) return;
        if (current.state !== 'draft' && current.state !== 'auth_required') {
          navigate({ kind: 'launch', launchId: current.id }, { replace: true });
          return;
        }
        setLaunch(current);
        setOverrides(initialOverrides(current));
        setWorkflowAcknowledged(current.workflow_risk_acknowledged);
        setConnections(available);
        const selected =
          current.connection?.service_id ?? available.find(connectionReady)?.service_id ?? '';
        setSelectedService(selected);
      })
      .catch((cause: unknown) => {
        if (live) setFailure(message(cause));
      })
      .finally(() => {
        if (live) setLoading(false);
      });
    return () => {
      live = false;
    };
  }, [executeCommand, navigate, publicId, readAttempt]);

  useEffect(() => {
    if (selectedService === '') {
      return;
    }
    const controller = new AbortController();
    void preflightTriggerConnection(selectedService, controller.signal)
      .then((connection) => {
        setPreflight(connection);
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
  }, [selectedService]);

  useEffect(
    () => () => {
      authorization.current?.abort();
    },
    [],
  );

  const fields = useMemo(() => (launch === null ? [] : taskFields(launch)), [launch]);
  const editable = useMemo(
    () => (launch === null ? new Set<string>() : editableFields(launch)),
    [launch],
  );
  const checkChoices = useMemo(
    () => (launch === null ? [] : optionalCheckChoices(launch)),
    [launch],
  );

  function reviewSnapshot(): ReviewSnapshot | null {
    if (overrides === null || selectedService === '') return null;
    return {
      selectedService,
      overrides: {
        ...overrides,
        task_inputs: Object.fromEntries(
          Object.entries(overrides.task_inputs).filter(([, value]) => value !== undefined),
        ),
      },
      workflowAcknowledged,
    };
  }

  async function persistReview(snapshot: ReviewSnapshot): Promise<Launch | null> {
    if (launch === null) return null;
    let current = launch;
    if (current.connection?.service_id !== snapshot.selectedService) {
      const slot = `launch:connection:${current.id}`;
      current = await executeCommand(slot, 'GitHub connection selection', (idempotencyKey) =>
        selectLaunchConnection(current.id, current.revision_number, snapshot.selectedService, {
          idempotencyKey,
        }),
      );
      setLaunch(current);
    }
    const server = initialOverrides(current);
    if (!equalOverrides(snapshot.overrides, server)) {
      const slot = `launch:overrides:${current.id}`;
      current = await executeCommand(slot, 'review update', (idempotencyKey) =>
        updateLaunchOverrides(current.id, current.revision_number, snapshot.overrides, {
          idempotencyKey,
        }),
      );
      setLaunch(current);
    }
    setLaunch(current);
    setOverrides(initialOverrides(current));
    return current;
  }

  async function save(): Promise<void> {
    const snapshot = reviewSnapshot();
    if (snapshot === null) return;
    setBusy('saving');
    setFailure(null);
    try {
      await persistReview(snapshot);
    } catch (cause) {
      if (cause instanceof TransportError) {
        setFailure(
          'The review outcome is unknown. CMA retained the exact connection, configuration, and command for reconciliation.',
        );
      } else if (cause instanceof ApiError && cause.status === 409) {
        setFailure(
          'This review changed in another tab. Your entries are retained; reload the current Launch before saving again.',
        );
      } else {
        setFailure(message(cause));
      }
    } finally {
      setBusy(null);
    }
  }

  async function start(): Promise<void> {
    const snapshot = reviewSnapshot();
    if (snapshot === null || launch === null) return;
    setBusy('starting');
    setFailure(null);
    try {
      const startSlot = `launch:start:${launch.id}`;
      const reviewed = pending?.slot === startSlot ? launch : await persistReview(snapshot);
      if (reviewed === null) return;
      const started = await executeCommand(startSlot, 'Launch start', (idempotencyKey) =>
        startLaunch(
          reviewed.id,
          reviewed.revision_number,
          reviewed.effective.digest,
          snapshot.workflowAcknowledged,
          { idempotencyKey },
        ),
      );
      setLaunch(started);
      setConfirming(false);
      navigate({ kind: 'launch', launchId: started.id });
    } catch (cause) {
      if (cause instanceof TransportError) {
        setFailure(
          'The Start outcome is unknown. CMA retained the exact reviewed configuration and command for reconciliation.',
        );
      } else {
        setConfirming(false);
        setFailure(message(cause));
      }
    } finally {
      setBusy(null);
    }
  }

  async function beginConnection() {
    const intentId = crypto.randomUUID();
    return executeCommand('connection:begin', 'GitHub connection', (idempotencyKey) =>
      beginTriggerConnection(intentId, { idempotencyKey }),
    );
  }

  async function reconnectConnection(serviceId: string) {
    return executeCommand(
      `connection:reconnect:${serviceId}`,
      'GitHub reconnection',
      (idempotencyKey) => reconnectTriggerConnection(serviceId, { idempotencyKey }),
    );
  }

  async function connect(mode: 'begin' | 'reconnect'): Promise<void> {
    if (mode === 'reconnect' && selectedService === '') return;
    authorization.current?.abort();
    const controller = new AbortController();
    authorization.current = controller;
    setBusy('connecting');
    setFailure(null);
    try {
      const connection = await authorizeTriggerConnection({
        signal: controller.signal,
        begin: () => (mode === 'begin' ? beginConnection() : reconnectConnection(selectedService)),
        preflight: preflightTriggerConnection,
        onService: (serviceId) => {
          setSelectedService(serviceId);
          setPreflight(null);
        },
      });
      setSelectedService(connection.service_id);
      setPreflight(connection);
      setConnections((items) => [
        connection,
        ...items.filter((item) => item.service_id !== connection.service_id),
      ]);
    } catch (cause) {
      if (!controller.signal.aborted) {
        setFailure(
          cause instanceof TransportError
            ? 'The connection request outcome is unknown. CMA retained the exact command for reconciliation.'
            : message(cause),
        );
      }
    } finally {
      if (authorization.current === controller) authorization.current = null;
      setBusy(null);
    }
  }

  async function checkConnection(): Promise<void> {
    if (selectedService === '') return;
    setBusy('checking');
    setFailure(null);
    try {
      const connection = await preflightTriggerConnection(selectedService);
      setPreflight(connection);
      setConnections((items) => [
        connection,
        ...items.filter((item) => item.service_id !== connection.service_id),
      ]);
    } catch (cause) {
      setPreflight(null);
      setFailure(message(cause));
    } finally {
      setBusy(null);
    }
  }

  if (loading) {
    return (
      <main className="trigger-workflow" aria-busy="true">
        <header className="trigger-workflow__head">
          <h1 ref={heading} tabIndex={-1}>
            Review Trigger
          </h1>
        </header>
        <div className="trigger-workflow__loading">
          <div className="skeleton" />
          <div className="skeleton" />
          <div className="skeleton" />
        </div>
      </main>
    );
  }

  if (failure !== null && launch === null) {
    return (
      <main className="trigger-workflow">
        <section className="trigger-workflow__fatal" role="alert">
          <IconWarningOutline16 />
          <div>
            <h1 ref={heading} tabIndex={-1}>
              Trigger review unavailable
            </h1>
            <p>{failure}</p>
            <Button
              variant="outline"
              onClick={() => {
                setLoading(true);
                setFailure(null);
                setReadAttempt((attempt) => attempt + 1);
              }}
            >
              {pending?.slot === `trigger:resume:${publicId}` ? 'Reconcile Launch' : 'Retry'}
            </Button>
          </div>
        </section>
      </main>
    );
  }

  if (launch === null || overrides === null) return null;
  const repository = upstreamRepository(launch);
  const selectedConnection =
    connections.find((item) => item.service_id === selectedService) ?? null;
  const canStart =
    preflight?.service_id === selectedService &&
    isActiveTriggerConnection(preflight) &&
    (launch.workflow_paths.length === 0 || workflowAcknowledged);
  const needsReconnect =
    selectedConnection !== null &&
    (selectedConnection.state !== 'active' || selectedConnection.account_id === null);
  const editsLocked = busy !== null || pending !== null;
  const pendingConnection = pending?.slot.startsWith('connection:') ?? false;
  const pendingReview =
    pending?.slot === `launch:connection:${launch.id}` ||
    pending?.slot === `launch:overrides:${launch.id}`;
  const pendingStart = pending?.slot === `launch:start:${launch.id}`;
  const pendingStartSequence = confirming && (pendingReview || pendingStart);

  return (
    <main className="trigger-workflow">
      <header className="trigger-workflow__head">
        <div>
          <p className="eyebrow">Trigger review</p>
          <h1 ref={heading} tabIndex={-1}>
            {launch.revision_content.task.public_summary}
          </h1>
          <p className="trigger-workflow__repo">
            <IconBranchOutline16 />
            {repository.canonical_owner}/{repository.canonical_name}
          </p>
        </div>
        <code title={launch.revision_digest}>Revision {shortRevision(launch.revision_digest)}</code>
      </header>

      {failure === null ? null : (
        <div className="notice notice--fail trigger-workflow__notice" role="alert">
          <IconWarningOutline16 />
          <div>
            <p className="notice__title">The review could not be applied.</p>
            <p className="notice__detail">{failure}</p>
          </div>
        </div>
      )}
      {pending === null ? null : (
        <div className="notice notice--quiet trigger-workflow__notice" role="status">
          <IconCheckOutline16 />
          <div>
            <p className="notice__title">Reconciling {pending.label}</p>
            <p className="notice__detail">
              Only the retained same-key command can run until CMA returns a finite result.
            </p>
          </div>
        </div>
      )}

      <div className="trigger-workflow__scroll">
        <section className="trigger-band" aria-labelledby="trigger-task-heading">
          <h2 id="trigger-task-heading">Task</h2>
          <div className="trigger-formgrid">
            {fields.map((field) => {
              const value = overrides.task_inputs[field.key] ?? field.defaultValue;
              const disabled = editsLocked || !field.editable || !editable.has('task_input');
              return (
                <label className="field" key={field.key}>
                  <span className="field__label">{field.key.replaceAll('_', ' ')}</span>
                  {field.kind.type === 'boolean' ? (
                    <span className="trigger-checkrow">
                      <input
                        type="checkbox"
                        checked={value === true}
                        disabled={disabled}
                        onChange={(event) => {
                          setOverrides({
                            ...overrides,
                            task_inputs: {
                              ...overrides.task_inputs,
                              [field.key]: event.target.checked,
                            },
                          });
                        }}
                      />
                      <span>{value === true ? 'Enabled' : 'Disabled'}</span>
                    </span>
                  ) : field.kind.type === 'choice' ? (
                    <select
                      className="trigger-select"
                      value={typeof value === 'string' ? value : ''}
                      disabled={disabled}
                      required={field.required}
                      onChange={(event) => {
                        setOverrides({
                          ...overrides,
                          task_inputs: {
                            ...overrides.task_inputs,
                            [field.key]: event.target.value,
                          },
                        });
                      }}
                    >
                      {field.kind.values.map((choice) => (
                        <option key={choice}>{choice}</option>
                      ))}
                    </select>
                  ) : (
                    <Input
                      placement="field__input"
                      type={field.kind.type === 'integer' ? 'number' : 'text'}
                      value={typeof value === 'string' || typeof value === 'number' ? value : ''}
                      disabled={disabled}
                      required={field.required}
                      min={field.kind.minimum}
                      max={field.kind.maximum}
                      maxLength={field.kind.type === 'text' ? field.kind.maximum : undefined}
                      onChange={(event) => {
                        const next: JsonValue =
                          field.kind.type === 'integer'
                            ? Number(event.target.value)
                            : event.target.value;
                        setOverrides({
                          ...overrides,
                          task_inputs: { ...overrides.task_inputs, [field.key]: next },
                        });
                      }}
                    />
                  )}
                  <span className="field__hint">
                    {disabled ? 'Locked by the publisher' : 'Contributor editable'}
                  </span>
                </label>
              );
            })}
          </div>
        </section>

        <section className="trigger-band" aria-labelledby="trigger-runtime-heading">
          <h2 id="trigger-runtime-heading">Session configuration</h2>
          <div className="trigger-formgrid">
            <label className="field">
              <span className="field__label">Branch suffix</span>
              <Input
                placement="field__input"
                value={overrides.branch_suffix ?? ''}
                disabled={editsLocked || !editable.has('branch_suffix')}
                onChange={(event) =>
                  setOverrides({ ...overrides, branch_suffix: event.target.value })
                }
              />
              <span className="field__hint">The server owns the cma/ branch prefix.</span>
            </label>
            <label className="field">
              <span className="field__label">Timeout seconds</span>
              <Input
                placement="field__input"
                type="number"
                min={1}
                value={overrides.timeout_seconds ?? launch.effective.timeout_seconds}
                disabled={editsLocked || !editable.has('timeout')}
                onChange={(event) =>
                  setOverrides({ ...overrides, timeout_seconds: Number(event.target.value) })
                }
              />
            </label>
            <label className="field">
              <span className="field__label">CPU millis</span>
              <Input
                placement="field__input"
                type="number"
                min={1}
                value={
                  overrides.resource_size?.cpu_millis ?? launch.effective.resource_size.cpu_millis
                }
                disabled={editsLocked || !editable.has('resource_size')}
                onChange={(event) =>
                  setOverrides({
                    ...overrides,
                    resource_size: {
                      cpu_millis: Number(event.target.value),
                      memory_mib:
                        overrides.resource_size?.memory_mib ??
                        launch.effective.resource_size.memory_mib,
                    },
                  })
                }
              />
            </label>
            <label className="field">
              <span className="field__label">Memory MiB</span>
              <Input
                placement="field__input"
                type="number"
                min={1}
                value={
                  overrides.resource_size?.memory_mib ?? launch.effective.resource_size.memory_mib
                }
                disabled={editsLocked || !editable.has('resource_size')}
                onChange={(event) =>
                  setOverrides({
                    ...overrides,
                    resource_size: {
                      cpu_millis:
                        overrides.resource_size?.cpu_millis ??
                        launch.effective.resource_size.cpu_millis,
                      memory_mib: Number(event.target.value),
                    },
                  })
                }
              />
            </label>
          </div>
          {checkChoices.length === 0 ? null : (
            <fieldset className="trigger-checks">
              <legend>Optional checks</legend>
              {checkChoices.map((check) => (
                <label key={check}>
                  <input
                    type="checkbox"
                    checked={overrides.optional_checks.includes(check)}
                    disabled={editsLocked}
                    onChange={(event) =>
                      setOverrides({
                        ...overrides,
                        optional_checks: event.target.checked
                          ? [...overrides.optional_checks, check]
                          : overrides.optional_checks.filter((item) => item !== check),
                      })
                    }
                  />
                  <span>{check}</span>
                </label>
              ))}
            </fieldset>
          )}
        </section>

        <section className="trigger-band" aria-labelledby="trigger-connection-heading">
          <h2 id="trigger-connection-heading">GitHub connection</h2>
          {connections.length === 0 ? (
            <div className="trigger-connectionempty">
              <p>No GitHub account is connected through NyxID.</p>
              <Button
                icon={<IconLinkOutline16 />}
                disabled={busy !== null || (pending !== null && !pendingConnection)}
                onClick={() => void connect('begin')}
              >
                {busy === 'connecting'
                  ? 'Connecting'
                  : pendingConnection
                    ? 'Reconcile connection'
                    : 'Connect GitHub'}
              </Button>
            </div>
          ) : (
            <div className="trigger-connections" role="radiogroup" aria-label="GitHub account">
              {connections.map((connection) => (
                <label key={connection.service_id} className="trigger-connection">
                  <input
                    type="radio"
                    name="trigger-connection"
                    value={connection.service_id}
                    checked={selectedService === connection.service_id}
                    disabled={editsLocked}
                    onChange={() => {
                      setSelectedService(connection.service_id);
                      setPreflight(null);
                    }}
                  />
                  <span>
                    <strong>{connection.account_login ?? 'GitHub account unavailable'}</strong>
                    <small>
                      {connection.state} ·{' '}
                      {connection.granted_scopes.join(', ') || 'no scopes reported'}
                    </small>
                  </span>
                  {preflight?.service_id === connection.service_id && connectionReady(preflight) ? (
                    <span className="trigger-connection__verified">
                      <IconCheckOutline16 /> Verified
                    </span>
                  ) : null}
                </label>
              ))}
            </div>
          )}
          <div className="trigger-connectionactions">
            {needsReconnect ? (
              <Button
                icon={<IconLinkOutline16 />}
                disabled={busy !== null || (pending !== null && !pendingConnection)}
                onClick={() => void connect('reconnect')}
              >
                {busy === 'connecting'
                  ? 'Connecting'
                  : pendingConnection
                    ? 'Reconcile connection'
                    : 'Reconnect'}
              </Button>
            ) : null}
            {selectedService === '' ? null : (
              <Button
                variant="outline"
                icon={<IconCheckOutline16 />}
                disabled={busy !== null || pending !== null}
                onClick={() => void checkConnection()}
              >
                {busy === 'checking' ? 'Checking' : 'Check access'}
              </Button>
            )}
            {connections.length === 0 ? null : (
              <Button
                variant="outline"
                icon={<IconLinkOutline16 />}
                disabled={busy !== null || (pending !== null && !pendingConnection)}
                onClick={() => void connect('begin')}
              >
                {pendingConnection ? 'Reconcile connection' : 'Connect another'}
              </Button>
            )}
          </div>
          {selectedConnection === null ? null : (
            <p className="trigger-connectionnote">
              CMA stores this NyxID service identity and scope snapshot, never a GitHub token.
            </p>
          )}
        </section>

        <section className="trigger-band" aria-labelledby="trigger-locked-heading">
          <h2 id="trigger-locked-heading">Locked safety fields</h2>
          <dl className="trigger-facts">
            <div>
              <dt>Upstream</dt>
              <dd>
                {repository.canonical_owner}/{repository.canonical_name}
              </dd>
            </div>
            <div>
              <dt>Source</dt>
              <dd>
                {jsonText(launch.revision_content.upstream.source_mode)?.replaceAll('_', ' ') ??
                  'Pinned'}
              </dd>
            </div>
            <div>
              <dt>Permission</dt>
              <dd>{launch.effective.permission}</dd>
            </div>
            <div>
              <dt>Sandbox profile</dt>
              <dd>{jsonText(launch.revision_content.session.sandbox_profile) ?? 'Not named'}</dd>
            </div>
            <div>
              <dt>Credential route</dt>
              <dd>
                {jsonText(launch.revision_content.repository_access.credential_route)?.replaceAll(
                  '_',
                  ' ',
                ) ?? 'Contributor connection'}
              </dd>
            </div>
            <div>
              <dt>Billing</dt>
              <dd>
                {jsonText(launch.revision_content.commercial_policy.billing_mode)?.replaceAll(
                  '_',
                  ' ',
                ) ?? 'Not priced'}
              </dd>
            </div>
          </dl>
        </section>

        {launch.workflow_paths.length === 0 ? null : (
          <section className="trigger-workflowwarning" aria-labelledby="trigger-workflow-warning">
            <IconWarningOutline16 />
            <div>
              <h2 id="trigger-workflow-warning">Workflow files at the pinned baseline</h2>
              <ul>
                {launch.workflow_paths.map((path) => (
                  <li key={path}>
                    <code>{path}</code>
                  </li>
                ))}
              </ul>
              <label className="trigger-checkrow">
                <input
                  type="checkbox"
                  checked={workflowAcknowledged}
                  disabled={editsLocked}
                  onChange={(event) => setWorkflowAcknowledged(event.target.checked)}
                />
                <span>
                  I understand GitHub may require maintainer approval before fork workflows run.
                </span>
              </label>
            </div>
          </section>
        )}

        <section className="trigger-effective" aria-labelledby="trigger-effective-heading">
          <h2 id="trigger-effective-heading">Effective configuration</h2>
          <pre>{launch.effective.rendered_prompt}</pre>
          <dl className="trigger-facts">
            <div>
              <dt>Branch</dt>
              <dd>cma/{overrides.branch_suffix ?? launch.effective.branch_suffix}</dd>
            </div>
            <div>
              <dt>Resources</dt>
              <dd>
                {String(
                  overrides.resource_size?.cpu_millis ?? launch.effective.resource_size.cpu_millis,
                )}
                m CPU,{' '}
                {String(
                  overrides.resource_size?.memory_mib ?? launch.effective.resource_size.memory_mib,
                )}{' '}
                MiB
              </dd>
            </div>
            <div>
              <dt>Timeout</dt>
              <dd>
                {String(overrides.timeout_seconds ?? launch.effective.timeout_seconds)} seconds
              </dd>
            </div>
          </dl>
        </section>
      </div>

      <footer className="trigger-workflow__actions">
        <span className="control-reason">
          {canStart
            ? 'Nothing starts until you confirm.'
            : 'Select and verify GitHub access before Start.'}
        </span>
        <Button
          variant="outline"
          disabled={
            busy !== null ||
            selectedService === '' ||
            (pending !== null && !pendingReview) ||
            confirming
          }
          onClick={() => void save()}
        >
          {busy === 'saving' ? 'Saving' : pendingReview ? 'Reconcile review' : 'Save review'}
        </Button>
        <Button
          variant="primary"
          disabled={!canStart || busy !== null || pending !== null}
          onClick={() => setConfirming(true)}
        >
          Start Launch
        </Button>
      </footer>

      <Modal
        open={confirming}
        title="Start this Launch?"
        description="CMA will create or reuse your fork, create a Launch branch, allocate a sandbox, start a session, and send the reviewed task once."
        onClose={() => {
          if (!pendingStartSequence && busy === null) setConfirming(false);
        }}
        footer={
          <>
            <Button
              disabled={busy !== null || pendingStartSequence}
              onClick={() => setConfirming(false)}
            >
              Cancel
            </Button>
            <Button
              variant="primary"
              disabled={busy !== null || (pending !== null && !pendingStartSequence)}
              onClick={() => void start()}
            >
              {busy === 'starting'
                ? pendingStartSequence
                  ? 'Reconciling'
                  : 'Submitting'
                : pendingStartSequence
                  ? 'Reconcile Start'
                  : 'Start Launch'}
            </Button>
          </>
        }
      >
        <p className="trigger-confirmation">
          The upstream target and GitHub account are locked to this reviewed snapshot. Publication
          and a pull request will still require separate confirmations.
        </p>
      </Modal>
    </main>
  );
}
