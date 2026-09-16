// Only errors created by the observer may contribute a code to private evidence.
// Exception messages from HTTP, JSON, files or subprocesses are never copied.
export class ObservationError extends Error {}

export function check(condition, code) {
  if (!condition) throw new ObservationError(code);
}

export function errorCode(error) {
  return error instanceof ObservationError ? error.message : 'continuity_observation_failed';
}

export function requireActive(signal) {
  if (signal?.aborted) {
    throw signal.reason instanceof ObservationError ? signal.reason : new ObservationError('observation_cancelled');
  }
}
