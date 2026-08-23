import type { MemoryPressure, Resources } from './conversationModel';

/** A JSON integer the browser can carry without silently changing it. */
function figure(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 ? value : undefined;
}

/**
 * Fold one whole `/resources` replacement into the conversation model.
 *
 * Every key is written even when its value is absent. That preserves the
 * server's distinction between a real zero and `null`, and makes a replacement
 * discard every figure from its predecessor rather than accidentally merging
 * two readings.
 */
export function resourcesFrom(value: unknown): Resources | undefined {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    return undefined;
  }
  const reading = value as Record<string, unknown>;
  return {
    cpuMillisUsed: figure(reading.cpuMillisUsed),
    cpuMillisLimit: figure(reading.cpuMillisLimit),
    memoryBytesUsed: figure(reading.memoryBytesUsed),
    memoryBytesLimit: figure(reading.memoryBytesLimit),
    memoryBytesPeak: figure(reading.memoryBytesPeak),
    stallPermille: figure(reading.stallPermille),
    ceilingHits: figure(reading.ceilingHits),
    allocationFailures: figure(reading.allocationFailures),
    kills: figure(reading.kills),
    receivedBytes: figure(reading.receivedBytes),
    sentBytes: figure(reading.sentBytes),
    receivedBytesPerSecond: figure(reading.receivedBytesPerSecond),
    sentBytesPerSecond: figure(reading.sentBytesPerSecond),
    intervalMs: figure(reading.intervalMs),
  };
}

/** A complete open episode, or absence for null, missing, or malformed state. */
export function memoryPressureFrom(value: unknown): MemoryPressure | undefined {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    return undefined;
  }
  const pressure = value as Record<string, unknown>;
  const usedBytes = figure(pressure.usedBytes);
  const limitBytes = figure(pressure.limitBytes);
  const occupancyPermille = figure(pressure.occupancyPermille);
  const since =
    typeof pressure.since === 'string' &&
    pressure.since !== '' &&
    !Number.isNaN(Date.parse(pressure.since))
      ? pressure.since
      : undefined;
  return usedBytes === undefined ||
    limitBytes === undefined ||
    occupancyPermille === undefined ||
    since === undefined
    ? undefined
    : { usedBytes, limitBytes, occupancyPermille, since };
}
