export interface OutboundReceipt {
  delivery_id: string;
  route_id: string;
  status: 'prepared' | 'committed' | 'cancelled';
  expires_at?: number;
  rendering?: { outcome: 'rendered' | 'degraded' | 'unsupported' };
  delivery?: { outcome: 'confirmed' | 'rejected' | 'unknown'; reason?: string } | null;
}

function record(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === 'object' && !Array.isArray(value);
}

export function validOutboundReceipt(value: unknown): value is OutboundReceipt {
  if (!record(value)) return false;
  const delivery = value.delivery;
  const rendering = value.rendering;
  return (
    typeof value.delivery_id === 'string' &&
    value.delivery_id.length > 0 &&
    typeof value.route_id === 'string' &&
    value.route_id.length > 0 &&
    typeof value.status === 'string' &&
    ['prepared', 'committed', 'cancelled'].includes(value.status) &&
    (value.expires_at === undefined ||
      (Number.isSafeInteger(value.expires_at) &&
        (value.expires_at as number) >= 0 &&
        (value.expires_at as number) <= 8640000000000)) &&
    (rendering === undefined ||
      (record(rendering) &&
        typeof rendering.outcome === 'string' &&
        ['rendered', 'degraded', 'unsupported'].includes(rendering.outcome))) &&
    (delivery == null ||
      (record(delivery) &&
        typeof delivery.outcome === 'string' &&
        ['confirmed', 'rejected', 'unknown'].includes(delivery.outcome) &&
        (delivery.reason === undefined || typeof delivery.reason === 'string')))
  );
}

const reasons: Readonly<Record<string, string>> = {
  provider_failure:
    'The provider reported an error. Delivery is unconfirmed; some or all of the message may have been sent.',
  response_lost:
    'The provider response was lost. Delivery is unconfirmed; some or all of the message may have been sent.',
  receipt_invalid:
    'The provider did not return a valid acknowledgement. Delivery is unconfirmed; some or all of the message may have been sent.',
  no_credential:
    'The destination has no usable credential. Review its settings before a new message.',
  outbound_authority_changed: 'The destination settings changed. Review them before a new message.',
};

export interface OutboundStatus {
  message: string;
  send: boolean;
  cancel: boolean;
  check: boolean;
  newMessage: boolean;
  separateMessage: boolean;
}

export function outboundStatus(
  receipt: OutboundReceipt,
  uncertain = false,
  preparationExpired = false,
  now = Date.now(),
): OutboundStatus {
  const status: OutboundStatus = {
    message: 'The message status could not be verified. Check the saved delivery status.',
    send: false,
    cancel: false,
    check: true,
    newMessage: false,
    separateMessage: false,
  };
  if (!validOutboundReceipt(receipt)) return status;
  if (receipt.status === 'cancelled' && receipt.delivery == null)
    return { ...status, message: 'Cancelled before sending.', check: false, newMessage: true };
  if (receipt.status === 'prepared' && receipt.delivery == null) {
    if (
      preparationExpired ||
      (receipt.expires_at !== undefined && receipt.expires_at * 1000 <= now)
    )
      return {
        ...status,
        message: preparationExpired
          ? 'This preparation can no longer be sent. Check or cancel the original preparation before starting a new message.'
          : 'This preparation may have expired. Check or cancel it before starting a new message.',
        cancel: true,
      };
    if (receipt.rendering?.outcome === 'unsupported')
      return {
        ...status,
        message:
          'This message could not be rendered. Cancel the preparation before reviewing a new message.',
        cancel: true,
      };
    return {
      ...status,
      message: uncertain
        ? 'Delivery is in progress or uncertain. Check its status before continuing.'
        : 'Review the destination, then send when ready.',
      send: !uncertain,
      cancel: !uncertain,
    };
  }
  if (receipt.status !== 'committed') return status;
  const delivery = receipt.delivery;
  if (delivery?.outcome === 'confirmed')
    return {
      ...status,
      message: 'The platform accepted the message.',
      check: false,
      newMessage: true,
    };
  if (delivery?.outcome === 'rejected') {
    const reason = delivery.reason;
    if (reason && !['no_credential', 'outbound_authority_changed'].includes(reason)) return status;
    return {
      ...status,
      message: `The message was rejected without confirmed delivery. ${reason ? reasons[reason] : 'Review the destination settings before trying a new message.'}`,
      check: false,
      newMessage: true,
    };
  }
  if (!delivery || delivery.reason === 'delivery_in_progress_or_uncertain' || !delivery.reason)
    return {
      ...status,
      message: 'Delivery is in progress or uncertain. Check its status before continuing.',
    };
  if (['provider_failure', 'response_lost', 'receipt_invalid'].includes(delivery.reason))
    return { ...status, message: reasons[delivery.reason]!, separateMessage: true };
  return status;
}

export function outboundSeparateMessageEnabled() {
  return import.meta.env.VITE_OUTBOUND_SEPARATE_MESSAGE_ENABLED === 'true';
}
