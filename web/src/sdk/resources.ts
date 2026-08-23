/**
 * One session's current sandbox-resource status over REST.
 *
 * The live AG-UI lane says what the latest reading is. This point read answers
 * the one distinction that lane deliberately cannot: whether no reading has
 * arrived yet or this runtime image will never report one.
 */

import type { Resources } from '../chat/conversationModel';
import type { ResolvedConfig } from './config';
import { ApiError } from './errors';
import { request } from './http';

/** Why a granted session has no reading at this moment. */
export type ResourceStatusReason = 'no_reading_yet' | 'runtime_does_not_report';

/** The five-answer REST route narrowed to one visible session's successful body. */
export interface SessionResourceStatus {
  readonly reading?: Resources;
  readonly observedAt?: string;
  readonly reason?: ResourceStatusReason;
}

/** Read the latest resource status once; this function never polls. */
export async function readResources(
  config: ResolvedConfig,
  id: string,
  signal?: AbortSignal,
): Promise<SessionResourceStatus> {
  const raw = await request(config, `/api/v1/sessions/${encodeURIComponent(id)}/resources`, {
    signal,
  });
  return toResourceStatus(raw);
}

/** Validate the REST document and translate its snake-case reading to AG-UI's model. */
export function toResourceStatus(raw: unknown): SessionResourceStatus {
  const body = objectOf(raw, 'resource status');
  if (body.reading === null) {
    if (body.observed_at !== null) {
      throw new ApiError(0, 'resource status without a reading carried an observation time');
    }
    if (body.reason === 'no_reading_yet' || body.reason === 'runtime_does_not_report') {
      return { reason: body.reason };
    }
    throw new ApiError(0, 'resource status without a reading carried no recognised reason');
  }

  if (body.reason !== null) {
    throw new ApiError(0, 'resource status with a reading also carried an absence reason');
  }
  if (typeof body.observed_at !== 'string' || Number.isNaN(Date.parse(body.observed_at))) {
    throw new ApiError(0, 'resource status with a reading carried no observation time');
  }
  const reading = objectOf(body.reading, 'resource reading');
  return {
    reading: {
      cpuMillisUsed: figureOf(reading, 'cpu_millis_used'),
      cpuMillisLimit: figureOf(reading, 'cpu_millis_limit'),
      memoryBytesUsed: figureOf(reading, 'memory_bytes_used'),
      memoryBytesLimit: figureOf(reading, 'memory_bytes_limit'),
      memoryBytesPeak: figureOf(reading, 'memory_bytes_peak'),
      stallPermille: figureOf(reading, 'stall_permille'),
      ceilingHits: figureOf(reading, 'ceiling_hits'),
      allocationFailures: figureOf(reading, 'allocation_failures'),
      kills: figureOf(reading, 'kills'),
      receivedBytes: figureOf(reading, 'received_bytes'),
      sentBytes: figureOf(reading, 'sent_bytes'),
      receivedBytesPerSecond: figureOf(reading, 'received_bytes_per_second'),
      sentBytesPerSecond: figureOf(reading, 'sent_bytes_per_second'),
      intervalMs: figureOf(reading, 'interval_ms'),
    },
    observedAt: body.observed_at,
  };
}

function objectOf(raw: unknown, name: string): Record<string, unknown> {
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) {
    throw new ApiError(0, `${name} was not an object`);
  }
  return raw as Record<string, unknown>;
}

/** `null` is absence. Missing, negative, fractional, or unsafe numbers violate the contract. */
function figureOf(body: Record<string, unknown>, field: string): number | undefined {
  if (!(field in body)) {
    throw new ApiError(0, `resource reading omitted ${field}`);
  }
  const value = body[field];
  if (value === null) {
    return undefined;
  }
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0) {
    throw new ApiError(0, `resource reading carried an invalid ${field}`);
  }
  return value;
}
