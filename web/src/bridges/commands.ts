import { useEffect, useRef, useState } from 'react';
import { ApiError, ValidationError } from '@chronoai/cma-sdk';
import type { CommandOptions } from '../api/client';

const failures: Readonly<Record<string, string>> = {
  bridge_incompatible_target:
    'The selected shared parent is incompatible. Review its owner, profile revisions, runtime support, connected services and reserved capacity.',
  bridge_invoke_consent_required:
    'Grant permission to use the retained Agent’s context before enabling this bridge.',
  bridge_callback_conflict:
    'The selected key’s callback conflicts with this bridge or another active route. Use a dedicated matching key and the exact callback shown here.',
  bridge_binding_mismatch:
    'The selected bot, key and routes do not have a verified matching owner and connection.',
  bridge_custody_required: 'Import the exact active Agent key selected in these saved settings.',
  bridge_renewable_authority_required:
    'Disable this bridge, then import its selected key with permission to use your NyxID connection for 30 days. Current account and organization admin access are required. Separate Sandboxes also need supported scoped key setup.',
  bridge_target_mismatch:
    'The retained Agent does not match the selected owner and resource ancestry.',
  bridge_profile_unavailable:
    'One or more selected profile revisions are unavailable or incompatible.',
  bridge_rebinding_policy_required:
    'Choose how already accepted work is handled before changing the retained Agent.',
  bridge_identity_immutable: 'Create a new bridge to change its owner, bot or platform.',
  bridge_management_required:
    'Current owner or organization admin access is required to manage this bridge.',
  invalid_bridge_configuration:
    'These settings cannot be used together. Review context sharing, resource placement and limits.',
  bridge_setup_unavailable: 'Channel setup is unavailable on this deployment.',
  bridge_forbidden: 'You no longer have permission to manage this bridge.',
  bridge_revision_conflict: 'The bridge changed. Refresh and review the current settings.',
  bridge_validation_unavailable:
    'Connection validation is unavailable. Check the selected bot, route and key.',
  bridge_validation_failed:
    'The selected connection could not be verified. Review its callback, owner and key.',
  bridge_credential_unavailable: 'The selected key is unavailable. Import an active matching key.',
  invalid_json: 'Some settings are invalid. Review the selected values.',
  bridge_invalid_configuration:
    'These settings cannot be used together. Review the context, placement and limits.',
};

// Provider failures must never render arbitrary response text near a write-only secret.
export function bridgeFailure(error: unknown): string {
  if (error instanceof ApiError) {
    const message = error.code === undefined ? undefined : failures[error.code];
    if (message !== undefined) return message;
    if (error.status === 401 || error.status === 403)
      return 'Current management access could not be confirmed.';
    if (error.status === 409)
      return 'The bridge changed or this action conflicts with its current state. Refresh and review it.';
    if (error.status === 422 || error.status === 400)
      return 'The selected settings were refused. Review the connection, context and limits.';
    if (error.status === 503) return 'This bridge capability is temporarily unavailable.';
  }
  if (error instanceof ValidationError) return 'Review the required fields and selected settings.';
  return 'The result could not be confirmed. Retry the same action to recover its result.';
}

type Pending = {
  readonly label: string;
  readonly key: string;
  readonly send: (options: CommandOptions) => Promise<void>;
};
export type BridgeCommand = ReturnType<typeof useBridgeCommand>;

/** Retains the exact nonsecret command across an ambiguous result. Never use for key import. */
export function useBridgeCommand(allowed: boolean) {
  const [busy, setBusy] = useState(false);
  const [pending, setPending] = useState<Pending>();
  const [error, setError] = useState<string>();
  const active = useRef<AbortController | undefined>(undefined);
  const retained = useRef<Pending | undefined>(undefined);
  useEffect(() => () => active.current?.abort(), []);
  useEffect(() => {
    if (!allowed) active.current?.abort();
  }, [allowed]);
  const execute = async (command: Pending) => {
    if (!allowed || active.current !== undefined) return;
    const request = new AbortController();
    active.current = request;
    retained.current = command;
    setPending(command);
    setBusy(true);
    setError(undefined);
    const timer = setTimeout(() => request.abort(), 15_000);
    try {
      await command.send({ signal: request.signal, idempotencyKey: command.key });
      request.signal.throwIfAborted();
      retained.current = undefined;
      setPending(undefined);
    } catch (failure) {
      setError(bridgeFailure(failure));
      if (
        failure instanceof ValidationError ||
        (failure instanceof ApiError && !failure.isRetryable)
      ) {
        retained.current = undefined;
        setPending(undefined);
      }
    } finally {
      clearTimeout(timer);
      if (active.current === request) {
        active.current = undefined;
        setBusy(false);
      }
    }
  };
  return {
    busy,
    locked: pending !== undefined,
    error,
    retryLabel: pending?.label,
    retry: () => {
      if (retained.current !== undefined) void execute(retained.current);
    },
    run: (label: string, send: Pending['send']) => {
      if (retained.current === undefined) void execute({ label, key: crypto.randomUUID(), send });
    },
  };
}
