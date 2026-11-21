import { ProtocolError } from '../errors.js';
import { text } from '../schema.js';

export const reason = (value: unknown): string => {
  const parsed = text(value);
  if (
    parsed.trim().length === 0 ||
    new TextEncoder().encode(parsed).length > 512 ||
    /\p{Cc}/u.test(parsed)
  )
    throw new ProtocolError('reason must contain 1 to 512 bytes without control characters');
  return parsed;
};
