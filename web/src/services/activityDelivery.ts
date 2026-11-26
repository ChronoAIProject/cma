import type { ActivityDelivery } from './useChannelActivity';

export function deliveryLabel(delivery: ActivityDelivery): string {
  if (delivery.step === 'platform_status' || delivery.step === 'platform_failure') {
    switch (delivery.outcome) {
      case 'sent':
        return 'Sent by WhatsApp';
      case 'delivered':
        return 'Delivered to recipient';
      case 'read':
        return 'Read by recipient';
      case 'played':
        return 'Voice message played';
      case 'failed':
        return 'WhatsApp could not deliver this message';
    }
  }
  if (delivery.outcome === 'confirmed') {
    return delivery.recipients?.some((recipient) => recipient.kind.startsWith('whatsapp'))
      ? 'Accepted by WhatsApp'
      : 'Delivery confirmed';
  }
  if (delivery.outcome === 'buffered') return 'Buffered locally';
  if (delivery.outcome === 'unavailable') return 'Delivery unavailable';
  return delivery.outcome.replaceAll('_', ' ');
}

export function deliveryTime(delivery: ActivityDelivery): string | null {
  return delivery.outcome === 'confirmed' || delivery.step.startsWith('platform_')
    ? (delivery.confirmed_at ?? null)
    : delivery.observed_at;
}
