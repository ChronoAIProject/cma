import { ValidationError } from '../errors.js';

export function ownerHeaders(owner: string | undefined): Record<string, string> {
  if (owner === undefined) return {};
  if (!/^[\x21-\x7e]{1,512}$/.test(owner))
    throw new ValidationError('owner must contain 1–512 visible ASCII bytes');
  return { 'X-CMAEG-Owner': owner };
}
