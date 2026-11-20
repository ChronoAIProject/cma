import type { UsageMetricQuantity } from '@cma/client';

export function humanDuration(value: UsageMetricQuantity): string {
  const seconds = typeof value === 'string' ? BigInt(value) : BigInt(Math.floor(value));
  if (seconds === 0n) return 'None';
  const days = seconds / 86400n;
  const hours = (seconds % 86400n) / 3600n;
  const minutes = (seconds % 3600n) / 60n;
  const parts = [
    days > 0n ? `${days.toLocaleString('en-SG')}d` : '',
    hours > 0n ? `${hours}h` : '',
    minutes > 0n ? `${minutes}m` : '',
  ].filter(Boolean);
  return parts.length ? parts.slice(0, 2).join(' ') : `${seconds}s`;
}
