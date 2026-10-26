import { useState } from 'react';
import { validStoredOutboundMessage, type OutboundMessage } from './outboundMedia';

export interface OutboundReceipt {
  delivery_id: string;
  route_id: string;
  status: 'prepared' | 'committed' | 'cancelled';
  delivery?: { outcome: string; reason?: string } | null;
}
export interface OutboundDraft {
  key: string;
  body: { source_id: string; route_id: string; message: OutboundMessage };
}
export interface OutboundIntent {
  draft: OutboundDraft;
  receipt?: OutboundReceipt;
  uncertain: boolean;
}
export function outboundStorageKey(owner: string, recordId: string) {
  return `cma:channel-outbound:${JSON.stringify([location.origin, owner, recordId])}`;
}
export function validOutboundReceipt(value: unknown): value is OutboundReceipt {
  if (!value || typeof value !== 'object') return false;
  const receipt = value as OutboundReceipt;
  return (
    typeof receipt.delivery_id === 'string' &&
    receipt.delivery_id.length > 0 &&
    typeof receipt.route_id === 'string' &&
    receipt.route_id.length > 0 &&
    ['prepared', 'committed', 'cancelled'].includes(receipt.status) &&
    (receipt.delivery == null ||
      (['confirmed', 'rejected', 'unknown'].includes(receipt.delivery.outcome) &&
        (receipt.delivery.reason === undefined || typeof receipt.delivery.reason === 'string')))
  );
}
function load(key: string): { intent?: OutboundIntent; error?: string } {
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
/** One per-tab slot per owner and stable channel record; contains no credentials.
 * Keep unsettled intent across logout, but never show it to a different owner.
 */
export function useOutboundJournal(owner: string, recordId: string) {
  const key = outboundStorageKey(owner, recordId);
  const [saved, setSaved] = useState(() => load(key));
  const retain = (intent?: OutboundIntent) => {
    try {
      if (intent)
        sessionStorage.setItem(
          key,
          JSON.stringify({
            version: intent.draft.body.message.native_attachments?.length ? 2 : 1,
            intent,
          }),
        );
      else sessionStorage.removeItem(key);
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
  return { ...saved, retain };
}
