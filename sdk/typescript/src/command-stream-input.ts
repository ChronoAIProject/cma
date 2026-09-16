import { ValidationError } from './errors.js';

/** Freeze invocation intent before the transport awaits request IDs or credentials. */
export function commandStreamInput<T>(body: T, key?: string) {
  if (
    key !== undefined &&
    (typeof key !== 'string' ||
      !key.length ||
      new TextEncoder().encode(key).length > 256 ||
      [...key].some(
        (char) =>
          char.charCodeAt(0) < 32 || (char.charCodeAt(0) >= 127 && char.charCodeAt(0) <= 159),
      ))
  )
    throw new ValidationError(
      'idempotencyKey must contain 1 to 256 bytes without control characters',
    );
  let captured: T;
  try {
    captured = JSON.parse(JSON.stringify(body)) as T;
  } catch (cause) {
    throw new ValidationError('The command input must be JSON serializable', { cause });
  }
  return { body: captured, headers: key === undefined ? undefined : { 'Idempotency-Key': key } };
}
