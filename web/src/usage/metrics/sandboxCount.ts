import type { UsageMetricQuantity } from '@cma/client';
import { displayQuantity } from './model';

export function sandboxCount(value: UsageMetricQuantity | null | undefined): string {
  const singular = typeof value === 'string' ? BigInt(value) === 1n : value === 1;
  return `${displayQuantity(value)} ${singular ? 'Sandbox' : 'Sandboxes'}`;
}
