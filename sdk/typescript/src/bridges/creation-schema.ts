import { ProtocolError } from '../errors.js';
import { resourceOwner } from '../resource-access.js';
import { object, text, type Output } from '../schema.js';

const shape = object({ bridge_id: text, owner: resourceOwner, callback_url: text });
export type BridgeCreationTarget = Output<typeof shape>;

/** A deployment-owned callback for the exact future Bridge, without reservation. */
export function creationTarget(value: unknown): BridgeCreationTarget {
  const result = shape(value);
  let url: URL;
  try {
    url = new URL(result.callback_url);
  } catch {
    throw new ProtocolError('bridge creation callback must be an absolute HTTPS URL');
  }
  if (
    !/^brg_[a-f0-9]{64}$/.test(result.bridge_id) ||
    result.callback_url !== result.callback_url.trim() ||
    url.protocol !== 'https:' ||
    url.username !== '' ||
    url.password !== '' ||
    url.search !== '' ||
    url.hash !== '' ||
    !url.pathname.endsWith(`/${result.bridge_id}`)
  )
    throw new ProtocolError('bridge creation callback does not match its identity');
  return result;
}
