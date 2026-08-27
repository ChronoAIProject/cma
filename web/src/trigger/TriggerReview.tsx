import { useEffect, useMemo, useRef, useState } from 'react';
import type {
  JsonValue,
  Launch,
  LaunchOverrides,
  LaunchQuotaSnapshot,
  TriggerConnection,
} from '@chronoai/cma-sdk';
import {
  ApiError,
  beginTriggerConnection,
  createOrResumeLaunch,
  listTriggerConnections,
  preflightTriggerConnection,
  readLaunch,
  reconnectTriggerConnection,
  revokeTriggerConnection,
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
  IconTrashOutline16,
  IconWarningOutline16,
} from '../ui/icons';
import type { AppRouter, TriggerAuthError } from '../routing/routes';
import { authorizeTriggerConnection, isActiveTriggerConnection } from './connectionFlow';
import { TriggerAuthNotice } from './TriggerAuthNotice';
import {
  commandBoolean,
  commandObject,
  exactString,
  nonEmptyString,
  nonNegativeInteger,
  oneOfStrings,
} from './commandValidation';
import {
  editableFields,
  jsonText,
  shortRevision,
  taskFields,
  timestamp,
  triggerKernelLabel,
  upstreamRepository,
} from './model';
import {
  commandOutcomeIsUnresolved,
  retainedCommandMetadata,
  retainedCommandRequest,
  useIdempotentCommand,
} from './useIdempotentCommand';
import './Trigger.css';

type TriggerReviewProps =
  | {
      readonly ownerId: string;
      readonly publicId: string;
      readonly recoveryLaunchId?: never;
      readonly authError?: TriggerAuthError;
      readonly navigate: AppRouter['navigate'];
    }
  | {
      readonly ownerId: string;
      readonly publicId?: never;
      readonly recoveryLaunchId: string;
      readonly authError?: never;
      readonly navigate: AppRouter['navigate'];
    };

type TaskValues = Readonly<Record<string, JsonValue>>;

interface ReviewSnapshot {
  readonly selectedService: string;
  readonly overrides: LaunchOverrides;
  readonly workflowAcknowledged: boolean;
}

interface ReviewCommandMetadata {
  readonly launchId: string;
  readonly expectedRevision: number;
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

function formatMicros(micros: number, currency: string): string {
  return new Intl.NumberFormat(undefined, {
    style: 'currency',
    currency,
    minimumFractionDigits: 2,
    maximumFractionDigits: 6,
  }).format(micros / 1_000_000);
}

function deploymentCapacity(quota: LaunchQuotaSnapshot): string {
  const cpu = quota.free_cpu_millis;
  const memory = quota.free_memory_mib;
  if (cpu === undefined && memory === undefined) return 'No deployment ceiling declared';
  if (cpu === undefined) return `No CPU ceiling declared, ${String(memory)} MiB free at review`;
  if (memory === undefined) return `${String(cpu)}m CPU free, no memory ceiling declared`;
  return `${String(cpu)}m CPU, ${String(memory)} MiB free at review`;
}

export function TriggerReview(props: TriggerReviewProps) {
  const key =
    props.recoveryLaunchId === undefined
      ? `trigger:${props.publicId}`
      : `recovery:${props.recoveryLaunchId}`;
  return <TriggerReviewRoute key={key} {...props} />;
}

function TriggerReviewRoute({
  ownerId,
  publicId,
  recoveryLaunchId,
  authError,
  navigate,
}: TriggerReviewProps) {
  const [launch, setLaunch] = useState<Launch | null>(null);
  const [connections, setConnections] = useState<readonly TriggerConnection[]>([]);
  const [selectedService, setSelectedService] = useState('');
  const [preflight, setPreflight] = useState<TriggerConnection | null>(null);
  const [overrides, setOverrides] = useState<LaunchOverrides | null>(null);
  const [workflowAcknowledged, setWorkflowAcknowledged] = useState(false);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState<
    'saving' | 'starting' | 'connecting' | 'checking' | 'revoking' | null
  >(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [confirmingRevoke, setConfirmingRevoke] = useState<string | null>(null);
  const [reviewNeedsRefresh, setReviewNeedsRefresh] = useState(false);
  const [readAttempt, setReadAttempt] = useState(0);
  const heading = useRef<HTMLHeadingElement>(null);
  const authorization = useRef<AbortController | null>(null);
  const commandResources = useMemo(
    () => [
      ...(publicId === undefined ? [] : [`trigger:${publicId}`]),
      ...(launch === null ? [] : [`launch:${launch.id}`]),
    ],
    [launch, publicId],
  );
  const { execute: executeCommand, pending } = useIdempotentCommand(ownerId, commandResources);

  useEffect(() => {
    heading.current?.focus();
  }, [loading, publicId, recoveryLaunchId]);

  useEffect(() => {
    let live = true;
    const launchRequest =
      recoveryLaunchId === undefined
        ? executeCommand(
            {
              slot: `trigger:resume:${publicId}`,
              resource: `trigger:${publicId}`,
              action: 'create-or-resume',
              request: { publicId },
              persistRequest: true,
            },
            'Launch creation',
            (idempotencyKey, request) => createOrResumeLaunch(request.publicId, { idempotencyKey }),
          )
        : readLaunch(recoveryLaunchId);
    void Promise.all([launchRequest, listTriggerConnections()])
      .then(([current, available]) => {
        if (!live) return;
        if (recoveryLaunchId !== undefined) {
          if (
            current.id !== recoveryLaunchId ||
            current.state !== 'auth_required' ||
            current.started_at === null
          ) {
            navigate({ kind: 'launch', launchId: current.id }, { replace: true });
            return;
          }
        } else if (current.state !== 'draft' && current.state !== 'auth_required') {
          navigate({ kind: 'launch', launchId: current.id }, { replace: true });
          return;
        }
        setLaunch(current);
        setReviewNeedsRefresh(false);
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
  }, [executeCommand, navigate, publicId, readAttempt, recoveryLaunchId]);

  useEffect(() => {
    if (selectedService === '') {
      return;
    }
    const controller = new AbortController();
    void preflightTriggerConnection(selectedService, controller.signal)
      .then((connection) => {
        if (controller.signal.aborted) return;
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
    const authenticationRecovery = current.state === 'auth_required' && current.started_at !== null;
    if (current.connection?.service_id !== snapshot.selectedService || authenticationRecovery) {
      const slot = `launch:connection:${current.id}`;
      const identity = {
        slot,
        resource: `launch:${current.id}`,
        action: 'select-connection',
      } as const;
      const request = retainedCommandRequest(
        pending,
        identity,
        {
          launchId: current.id,
          expectedRevision: current.revision_number,
          serviceId: snapshot.selectedService,
        },
        (value) =>
          commandObject(value, {
            launchId: exactString(current.id),
            expectedRevision: nonNegativeInteger,
            serviceId: oneOfStrings(connections.map((connection) => connection.service_id)),
          }),
      );
      current = await executeCommand(
        { ...identity, request, persistRequest: true },
        'GitHub connection selection',
        (idempotencyKey, frozen) =>
          selectLaunchConnection(frozen.launchId, frozen.expectedRevision, frozen.serviceId, {
            idempotencyKey,
          }),
      );
      setLaunch(current);
    }
    const server = initialOverrides(current);
    if (!equalOverrides(snapshot.overrides, server) || reviewNeedsRefresh) {
      const slot = `launch:overrides:${current.id}`;
      const identity = {
        slot,
        resource: `launch:${current.id}`,
        action: 'update-review',
      } as const;
      const retained = retainedCommandMetadata<ReviewCommandMetadata>(pending, identity, (value) =>
        commandObject<ReviewCommandMetadata>(value, {
          launchId: exactString(current.id),
          expectedRevision: nonNegativeInteger,
        }),
      );
      const request = {
        launchId: retained?.launchId ?? current.id,
        expectedRevision: retained?.expectedRevision ?? current.revision_number,
        overrides: snapshot.overrides,
      };
      current = await executeCommand(
        {
          ...identity,
          request,
          retained: {
            launchId: request.launchId,
            expectedRevision: request.expectedRevision,
          },
        },
        'review update',
        (idempotencyKey, frozen) =>
          updateLaunchOverrides(frozen.launchId, frozen.expectedRevision, frozen.overrides, {
            idempotencyKey,
          }),
      );
      setLaunch(current);
      setReviewNeedsRefresh(false);
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
      if (commandOutcomeIsUnresolved(cause)) {
        setFailure(
          'The review outcome is unknown. CMA retained the exact connection, configuration, and command for reconciliation.',
        );
      } else if (
        cause instanceof ApiError &&
        (cause.code === 'trigger_state_conflict' || cause.code === 'revision_conflict')
      ) {
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

  async function resumeAfterSignIn(): Promise<void> {
    const snapshot = reviewSnapshot();
    if (snapshot === null || launch === null) return;
    setBusy('saving');
    setFailure(null);
    try {
      const resumed = await persistReview(snapshot);
      if (resumed !== null) {
        navigate({ kind: 'launch', launchId: resumed.id });
      }
    } catch (cause) {
      setFailure(
        commandOutcomeIsUnresolved(cause)
          ? 'The recovery outcome is unknown. CMA retained the exact connection selection for reconciliation.'
          : message(cause),
      );
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
      const identity = {
        slot: startSlot,
        resource: `launch:${reviewed.id}`,
        action: 'start',
      } as const;
      const request = retainedCommandRequest(
        pending,
        identity,
        {
          launchId: reviewed.id,
          expectedRevision: reviewed.revision_number,
          snapshotDigest: reviewed.effective.digest,
          acknowledgeWorkflowRisk: snapshot.workflowAcknowledged,
        },
        (value) =>
          commandObject(value, {
            launchId: exactString(reviewed.id),
            expectedRevision: nonNegativeInteger,
            snapshotDigest: nonEmptyString,
            acknowledgeWorkflowRisk: commandBoolean,
          }),
      );
      const started = await executeCommand(
        { ...identity, request, persistRequest: true },
        'Launch start',
        (idempotencyKey, frozen) =>
          startLaunch(
            frozen.launchId,
            frozen.expectedRevision,
            frozen.snapshotDigest,
            frozen.acknowledgeWorkflowRisk,
            { idempotencyKey },
          ),
      );
      setLaunch(started);
      setConfirming(false);
      navigate({ kind: 'launch', launchId: started.id });
    } catch (cause) {
      if (commandOutcomeIsUnresolved(cause)) {
        setFailure(
          'The Start outcome is unknown. CMA retained the exact reviewed configuration and command for reconciliation.',
        );
      } else if (
        cause instanceof ApiError &&
        (cause.code === 'trigger_price_changed' ||
          cause.code === 'trigger_runtime_changed' ||
          cause.code === 'trigger_quota_exhausted')
      ) {
        setConfirming(false);
        setReviewNeedsRefresh(true);
        setFailure(
          'Admission changed before Start. Save the review again to load current pricing, quota, and runtime evidence, then confirm Start again.',
        );
      } else if (
        cause instanceof ApiError &&
        (cause.code === 'trigger_draft_expired' || cause.code === 'trigger_quote_expired')
      ) {
        setConfirming(false);
        navigate({ kind: 'launch', launchId: launch.id });
      } else {
        setConfirming(false);
        setFailure(message(cause));
      }
    } finally {
      setBusy(null);
    }
  }

  async function beginConnection() {
    if (launch === null) throw new Error('The Launch is not ready for a GitHub connection.');
    const identity = {
      slot: 'connection:begin',
      resource: `launch:${launch.id}`,
      action: 'begin-connection',
    } as const;
    const request = retainedCommandRequest(
      pending,
      identity,
      { intentId: crypto.randomUUID() },
      (value) => commandObject(value, { intentId: nonEmptyString }),
    );
    return executeCommand(
      { ...identity, request, persistRequest: true },
      'GitHub connection',
      (idempotencyKey, frozen) => beginTriggerConnection(frozen.intentId, { idempotencyKey }),
    );
  }

  async function reconnectConnection(serviceId: string) {
    if (launch === null) throw new Error('The Launch is not ready for GitHub reconnection.');
    const identity = {
      slot: `connection:reconnect:${serviceId}`,
      resource: `launch:${launch.id}`,
      action: 'reconnect-connection',
    } as const;
    const request = retainedCommandRequest(pending, identity, { serviceId }, (value) =>
      commandObject(value, { serviceId: exactString(serviceId) }),
    );
    return executeCommand(
      { ...identity, request, persistRequest: true },
      'GitHub reconnection',
      (idempotencyKey, frozen) => reconnectTriggerConnection(frozen.serviceId, { idempotencyKey }),
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
          commandOutcomeIsUnresolved(cause)
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

  async function revokeConnection(serviceId: string | null): Promise<void> {
    if (launch === null || serviceId === null) return;
    setBusy('revoking');
    setFailure(null);
    const identity = {
      slot: `connection:revoke:${serviceId}`,
      resource: `launch:${launch.id}`,
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
      if (selectedService === serviceId) setPreflight(null);
      setConfirmingRevoke(null);
      try {
        setLaunch(await readLaunch(launch.id));
      } catch (cause) {
        setFailure(`The connection was revoked, but the Launch refresh failed. ${message(cause)}`);
      }
    } catch (cause) {
      if (!commandOutcomeIsUnresolved(cause)) setConfirmingRevoke(null);
      setFailure(
        commandOutcomeIsUnresolved(cause)
          ? 'The revocation outcome is unknown. CMA retained the exact connection and command key for reconciliation.'
          : message(cause),
      );
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
  const authenticationRecovery = launch.state === 'auth_required' && launch.started_at !== null;
  const connectionVerified =
    preflight?.service_id === selectedService && isActiveTriggerConnection(preflight);
  const admissionReady =
    launch.pricing !== null &&
    launch.runtime_admission !== null &&
    launch.review_quota !== null &&
    !reviewNeedsRefresh;
  const reviewDirty =
    launch.connection?.service_id !== selectedService ||
    !equalOverrides(overrides, initialOverrides(launch));
  const canStart =
    !authenticationRecovery &&
    connectionVerified &&
    admissionReady &&
    !reviewDirty &&
    (launch.workflow_paths.length === 0 || workflowAcknowledged);
  const canResume = authenticationRecovery && connectionVerified;
  const needsReconnect =
    selectedConnection !== null &&
    (selectedConnection.state !== 'active' || selectedConnection.account_id === null);
  const pendingRevoke = pending?.action === 'revoke-connection';
  const pendingRevokeRequest = pendingRevoke
    ? commandObject(pending.request, { serviceId: nonEmptyString })
    : null;
  const revokeServiceId = pendingRevokeRequest?.serviceId ?? confirmingRevoke;
  const revokeConnectionAccount =
    connections.find((connection) => connection.service_id === revokeServiceId)?.account_login ??
    null;
  const reconstructingReview =
    pending?.action === 'update-review' && pending.requiresRequestReconstruction;
  const editsLocked = busy !== null || (pending !== null && !reconstructingReview);
  const reviewLocked = editsLocked || authenticationRecovery;
  const pendingConnection = !pendingRevoke && (pending?.slot.startsWith('connection:') ?? false);
  const pendingReview =
    pending?.slot === `launch:connection:${launch.id}` ||
    pending?.slot === `launch:overrides:${launch.id}`;
  const pendingStart = pending?.slot === `launch:start:${launch.id}`;
  const startDialogOpen = confirming || pendingStart;
  const pendingStartSequence = pendingStart || (confirming && pendingReview);

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

      {authError === undefined ? null : (
        <TriggerAuthNotice error={authError} placement="workflow" />
      )}

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
              const disabled = reviewLocked || !field.editable || !editable.has('task_input');
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
                disabled={reviewLocked || !editable.has('branch_suffix')}
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
                disabled={reviewLocked || !editable.has('timeout')}
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
                disabled={reviewLocked || !editable.has('resource_size')}
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
                disabled={reviewLocked || !editable.has('resource_size')}
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
                    disabled={reviewLocked}
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
            {selectedConnection?.state === 'active' || pendingRevoke ? (
              <Button
                variant="danger"
                icon={<IconTrashOutline16 />}
                disabled={
                  busy !== null ||
                  (revokeServiceId === null && selectedService === '') ||
                  (pending !== null && !pendingRevoke)
                }
                onClick={() => setConfirmingRevoke(revokeServiceId ?? selectedService)}
              >
                {pendingRevoke ? 'Reconcile revocation' : 'Revoke connection'}
              </Button>
            ) : null}
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
                  disabled={reviewLocked}
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
              <dt>Price</dt>
              <dd>
                {launch.pricing === null
                  ? 'Admission unavailable'
                  : launch.pricing.billing_mode === 'zero_cost'
                    ? 'Zero cost'
                    : `${formatMicros(
                        launch.pricing.estimated_micros,
                        launch.pricing.currency,
                      )} maximum · ${formatMicros(
                        launch.pricing.micros_per_hour,
                        launch.pricing.currency,
                      )} per hour`}
              </dd>
            </div>
            <div>
              <dt>Review expires</dt>
              <dd>{timestamp(launch.draft_expires_at)}</dd>
            </div>
            <div>
              <dt>Kernel</dt>
              <dd>{triggerKernelLabel(launch) ?? 'Admission unavailable'}</dd>
            </div>
            <div>
              <dt>Runtime</dt>
              <dd>{launch.runtime_admission?.runtime_version ?? 'Admission unavailable'}</dd>
            </div>
            <div>
              <dt>Capabilities</dt>
              <dd>
                {launch.runtime_admission?.supported_capabilities
                  .map((capability) => capability.replaceAll('_', ' '))
                  .join(', ') || 'Admission unavailable'}
              </dd>
            </div>
            <div>
              <dt>Session quota</dt>
              <dd>
                {launch.review_quota === null
                  ? 'Admission unavailable'
                  : launch.review_quota.live_session_limit === undefined
                    ? `${String(launch.review_quota.live_sessions)} active · no owner limit declared`
                    : `${String(launch.review_quota.live_sessions)} of ${String(
                        launch.review_quota.live_session_limit,
                      )} active`}
              </dd>
            </div>
            <div>
              <dt>Deployment capacity</dt>
              <dd>
                {launch.review_quota === null
                  ? 'Admission unavailable'
                  : deploymentCapacity(launch.review_quota)}
              </dd>
            </div>
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
        {authenticationRecovery ? (
          <>
            <span className="control-reason">
              Sign in again and revalidate this connection to resume the existing Launch.
            </span>
            <Button
              variant="primary"
              disabled={!canResume || busy !== null || (pending !== null && !pendingReview)}
              onClick={() => void resumeAfterSignIn()}
            >
              {busy === 'saving'
                ? pendingReview
                  ? 'Reconciling'
                  : 'Resuming'
                : pendingReview
                  ? 'Reconcile recovery'
                  : 'Resume Launch'}
            </Button>
          </>
        ) : (
          <>
            <span className="control-reason">
              {canStart
                ? 'Nothing starts until you confirm.'
                : reviewNeedsRefresh
                  ? 'Save the review to accept current admission evidence before Start.'
                  : reviewDirty
                    ? 'Save the review to recalculate pricing, quota, and runtime evidence before Start.'
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
          </>
        )}
      </footer>

      <Modal
        open={confirmingRevoke !== null || pendingRevoke}
        title="Revoke GitHub connection?"
        description="CMA will deactivate this exact NyxID connection. Your Launch, workspace, fork, branch, commits, pull request, and session history are not deleted."
        dismissible={busy !== 'revoking' && !pendingRevoke}
        onClose={() => {
          if (busy === null && !pendingRevoke) setConfirmingRevoke(null);
        }}
        footer={
          <>
            <Button
              disabled={busy !== null || pendingRevoke}
              onClick={() => setConfirmingRevoke(null)}
            >
              Keep connection
            </Button>
            <Button
              variant="danger"
              disabled={
                busy !== null || revokeServiceId === null || (pending !== null && !pendingRevoke)
              }
              onClick={() => void revokeConnection(revokeServiceId)}
            >
              {busy === 'revoking'
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
        <p className="trigger-confirmation">
          {revokeConnectionAccount === null
            ? 'Affected Launches will require another eligible GitHub connection before more GitHub work can continue.'
            : `${revokeConnectionAccount} will no longer be eligible for new CMA GitHub work.`}{' '}
          Provider access may remain usable until revocation is acknowledged or the bounded
          credential lifetime expires.
        </p>
      </Modal>

      <Modal
        open={startDialogOpen}
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
