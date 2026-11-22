import { useState } from 'react';
import { validStoredOutboundMessage, type OutboundMessage } from './outboundMedia';
import {
  outboundSeparateMessageEnabled,
  outboundStatus,
  validOutboundReceipt,
  type OutboundReceipt,
} from './outboundStatus';
export { validOutboundReceipt, type OutboundReceipt } from './outboundStatus';

export interface OutboundDraft {
  key: string;
  body: { source_id: string; route_id: string; message: OutboundMessage };
}
export interface OutboundIntent {
  draft: OutboundDraft;
  receipt?: OutboundReceipt;
  uncertain: boolean;
  preparationExpired?: boolean;
}
export function outboundStorageKey(owner: string, recordId: string) {
  return `cma:channel-outbound:${JSON.stringify([location.origin, owner, recordId])}`;
}
export function outboundRecoveryKey(owner: string, recordId: string) {
  return `${outboundStorageKey(owner, recordId)}:previous`;
}
export function loadOutboundIntent(key: string): { intent?: OutboundIntent; error?: string } {
  try {
    const raw = sessionStorage.getItem(key);
    if (raw === null) return {};
    const saved = JSON.parse(raw) as { version: number; intent: OutboundIntent };
    const intent = saved.intent;
    const draft = intent?.draft;
    if (
      ![1, 2].includes(saved.version) ||
      !draft ||
      typeof draft.key !== 'string' ||
      !/^[\x21-\x7e]{1,128}$/.test(draft.key) ||
      typeof draft.body?.source_id !== 'string' ||
      !draft.body.source_id ||
      typeof draft.body.route_id !== 'string' ||
      !draft.body.route_id ||
      !validStoredOutboundMessage(draft.body.message, saved.version) ||
      typeof intent.uncertain !== 'boolean' ||
      (intent.preparationExpired !== undefined && typeof intent.preparationExpired !== 'boolean') ||
      (intent.receipt !== undefined &&
        (!validOutboundReceipt(intent.receipt) || intent.receipt.route_id !== draft.body.route_id))
    )
      throw new Error();
    return { intent };
  } catch {
    return {
      error:
        'The saved message could not be read. Restore browser session storage before continuing.',
    };
  }
}
export function storeOutboundIntent(key: string, intent?: OutboundIntent) {
  if (intent)
    sessionStorage.setItem(
      key,
      JSON.stringify({
        version: intent.draft.body.message.native_attachments?.length ? 2 : 1,
        intent,
      }),
    );
  else sessionStorage.removeItem(key);
}
/** One per-tab slot per owner and stable channel record; contains no credentials.
 * Keep unsettled intent across logout, but never show it to a different owner.
 */
export function useOutboundJournal(owner: string, recordId: string) {
  const key = outboundStorageKey(owner, recordId);
  const recoveryKey = outboundRecoveryKey(owner, recordId);
  const [saved, setSaved] = useState(() => loadOutboundIntent(key));
  const [previous, setPrevious] = useState(() => loadOutboundIntent(recoveryKey));
  const [recoveryError, setRecoveryError] = useState('');
  const retain = (intent?: OutboundIntent) => {
    try {
      storeOutboundIntent(key, intent);
      setSaved({ intent });
      return true;
    } catch {
      setSaved((previous) => ({
        ...previous,
        error: 'The message could not be saved. Restore browser session storage before continuing.',
      }));
      return false;
    }
  };
  const archive = () => {
    const intent = saved.intent;
    if (
      !outboundSeparateMessageEnabled() ||
      !intent?.receipt ||
      !outboundStatus(intent.receipt).separateMessage
    )
      return false;
    try {
      // Re-read the bounded slot before writing. A failed active-slot removal
      // can leave the same intent in both slots; retrying that move is safe.
      const stored = loadOutboundIntent(recoveryKey);
      if (
        stored.error ||
        (stored.intent && JSON.stringify(stored.intent) !== JSON.stringify(intent))
      ) {
        setRecoveryError(
          'A previous delivery is still retained. Check it before starting another separate message.',
        );
        return false;
      }
      storeOutboundIntent(recoveryKey, intent);
      setPrevious({ intent });
      sessionStorage.removeItem(key);
      setSaved({});
      setRecoveryError('');
      return true;
    } catch {
      setRecoveryError(
        'The previous delivery could not be saved. Your current message is retained; you can still check its status.',
      );
      return false;
    }
  };
  const retainPrevious = (intent?: OutboundIntent) => {
    if (
      !intent &&
      (!previous.intent?.receipt || !outboundStatus(previous.intent.receipt).newMessage)
    )
      return false;
    try {
      storeOutboundIntent(recoveryKey, intent);
      setPrevious({ intent });
      setRecoveryError('');
      return true;
    } catch {
      setRecoveryError(
        'The previous delivery status could not be saved. Its original receipt is retained.',
      );
      return false;
    }
  };
  return {
    ...saved,
    retain,
    previous: previous.intent,
    recoveryError: previous.error ?? recoveryError,
    archive,
    retainPrevious,
  };
}
