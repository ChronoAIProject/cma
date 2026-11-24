import type { RetainedReopenReceipt } from '@cma/client';

export interface RecoveryIntent {
  readonly revision: number;
  readonly key: string;
  readonly receipt?: RetainedReopenReceipt;
  readonly message?: string;
  readonly refused?: boolean;
}
function storageKey(owner: string, sandbox: string) {
  return `cma:retained-recovery:${JSON.stringify([location.origin, owner, sandbox])}`;
}
export function loadRecoveryIntent(
  owner: string,
  sandbox: string,
): { intent?: RecoveryIntent; error?: string } {
  try {
    const raw = sessionStorage.getItem(storageKey(owner, sandbox));
    if (raw === null) return {};
    const saved = JSON.parse(raw) as { version?: number; intent?: RecoveryIntent };
    const intent = saved.intent;
    if (
      saved.version !== 1 ||
      !intent ||
      !Number.isSafeInteger(intent.revision) ||
      intent.revision <= 0 ||
      typeof intent.key !== 'string' ||
      !/^[a-zA-Z0-9_:-]{1,128}$/.test(intent.key) ||
      (intent.refused !== undefined && typeof intent.refused !== 'boolean') ||
      (intent.receipt !== undefined &&
        (typeof intent.receipt.operation_id !== 'string' ||
          !Number.isSafeInteger(intent.receipt.revision) ||
          intent.receipt.revision !== intent.revision + 1 ||
          typeof intent.receipt.accepted_at !== 'string'))
    )
      throw new Error();
    // Persist only consent and receipt; explanatory text is produced locally.
    return {
      intent: {
        revision: intent.revision,
        key: intent.key,
        refused: intent.refused,
        receipt: intent.receipt,
      },
    };
  } catch {
    return {
      error:
        'The saved recovery request could not be read. Restore browser session storage before continuing.',
    };
  }
}
export function saveRecoveryIntent(owner: string, sandbox: string, intent: RecoveryIntent) {
  sessionStorage.setItem(
    storageKey(owner, sandbox),
    JSON.stringify({
      version: 1,
      intent: {
        revision: intent.revision,
        key: intent.key,
        refused: intent.refused,
        receipt: intent.receipt,
      },
    }),
  );
}

export function retireRecoveryReceipt(owner: string, sandbox: string, accepted: RecoveryIntent) {
  const saved = loadRecoveryIntent(owner, sandbox);
  if (saved.error) return saved;
  if (
    accepted.receipt &&
    saved.intent?.key === accepted.key &&
    saved.intent.revision === accepted.revision &&
    saved.intent.receipt?.operation_id === accepted.receipt.operation_id &&
    saved.intent.receipt.revision === accepted.receipt.revision &&
    saved.intent.receipt.accepted_at === accepted.receipt.accepted_at
  ) {
    try {
      sessionStorage.removeItem(storageKey(owner, sandbox));
      return {};
    } catch {
      return {
        ...saved,
        error:
          'The previous recovery receipt could not be removed. Restore browser session storage.',
      };
    }
  }
  return saved;
}
