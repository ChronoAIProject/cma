import {
  connectionUrl,
  type ActiveConnection,
  type ConnectionReport,
  type StartedConnection,
} from './client';

interface PendingIntent extends ActiveConnection {
  readonly force: boolean;
  readonly started?: StartedConnection;
}

interface RecoveryRecord {
  readonly owner: string;
  readonly needsResume: boolean;
  readonly pending?: PendingIntent;
}

function validRecord(value: unknown): value is RecoveryRecord {
  if (typeof value !== 'object' || value === null) return false;
  const record = value as RecoveryRecord;
  const pending = record.pending;
  return (
    typeof record.owner === 'string' &&
    record.owner !== '' &&
    typeof record.needsResume === 'boolean' &&
    (pending === undefined ||
      (pending !== null &&
        typeof pending === 'object' &&
        typeof pending.intent_id === 'string' &&
        pending.intent_id !== '' &&
        typeof pending.service_id === 'string' &&
        pending.service_id !== '' &&
        typeof pending.force === 'boolean' &&
        (pending.started === undefined ||
          (pending.started !== null &&
            typeof pending.started === 'object' &&
            (pending.started.user_code === undefined ||
              typeof pending.started.user_code === 'string') &&
            pending.started.owner === record.owner &&
            pending.started.intent_id === pending.intent_id &&
            pending.started.service_id === pending.service_id &&
            connectionUrl(pending.started.hosted_url) !== undefined &&
            Number.isFinite(Date.parse(pending.started.expires_at))))))
  );
}

export class ConnectionIntentRecovery {
  private record: RecoveryRecord | undefined;
  private readonly storageKey: string;

  constructor(
    key: string,
    private readonly enabled: boolean,
  ) {
    this.storageKey = `cma.connection-recovery.v1.${key}`;
  }

  get pending() {
    return this.record?.pending;
  }
  get needsResume() {
    return this.record?.needsResume === true;
  }

  bind(report: ConnectionReport, legacy?: StartedConnection) {
    if (!this.enabled || this.record !== undefined) return;
    const raw = sessionStorage.getItem(this.storageKey);
    let stored: unknown =
      legacy === undefined
        ? undefined
        : {
            owner: legacy.owner,
            needsResume: true,
            pending: {
              service_id: legacy.service_id,
              intent_id: legacy.intent_id,
              force: false,
              started: legacy,
            },
          };
    try {
      if (raw !== null) stored = JSON.parse(raw);
    } catch {
      /* Refuse malformed recovery. */
    }
    this.record =
      validRecord(stored) &&
      stored.owner === report.owner &&
      (stored.pending === undefined ||
        report.services.some((row) => row.service === stored.pending?.service_id))
        ? stored
        : { owner: report.owner, needsResume: raw !== null || legacy !== undefined };
    this.save(this.record);
  }

  prepare(service: string, force: boolean): ActiveConnection & { readonly force: boolean } {
    const pending = this.pending ?? { service_id: service, intent_id: crypto.randomUUID(), force };
    if (this.record !== undefined) {
      // A reload before acknowledgement must require another explicit action.
      this.save({ ...this.record, needsResume: true, pending });
      this.record = { ...this.record, needsResume: false, pending };
    }
    return { service_id: pending.service_id, intent_id: pending.intent_id, force: pending.force };
  }

  acknowledge(started: StartedConnection) {
    if (this.record === undefined || this.pending === undefined) return;
    const record = { ...this.record, pending: { ...this.pending, started } };
    if (!validRecord(record)) throw new Error('The sign-in link is unavailable. Please try again.');
    this.update(record);
  }

  interrupt(clearPending: boolean, needsResume = true) {
    if (this.record === undefined) return;
    this.update({
      ...this.record,
      needsResume,
      pending: clearPending ? undefined : this.pending,
    });
  }

  settle() {
    if (this.record !== undefined) this.update({ ...this.record, pending: undefined });
  }

  confirm() {
    if (this.record !== undefined) {
      const confirmed = { ...this.record, needsResume: false };
      this.save(confirmed);
      this.record = confirmed;
    }
  }

  private update(record: RecoveryRecord) {
    this.record = record;
    this.save(record);
  }

  private save(record: RecoveryRecord) {
    sessionStorage.setItem(this.storageKey, JSON.stringify(record));
  }
}

export function savedConnection(
  key: string,
  includeExpired: boolean,
): StartedConnection | undefined {
  try {
    const value = JSON.parse(
      sessionStorage.getItem(`cma.connection.${key}`) ?? 'null',
    ) as StartedConnection | null;
    if (
      value !== null &&
      typeof value.owner === 'string' &&
      typeof value.intent_id === 'string' &&
      typeof value.service_id === 'string' &&
      connectionUrl(value.hosted_url) !== undefined &&
      (includeExpired
        ? Number.isFinite(Date.parse(value.expires_at))
        : Date.parse(value.expires_at) > Date.now())
    )
      return value;
  } catch {
    /* Discard malformed legacy handoffs. */
  }
  return undefined;
}
