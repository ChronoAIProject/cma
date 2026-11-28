import {createHash} from 'node:crypto';

const oidNamespace = Buffer.from('6ba7b8129dad11d180b400c04fd430c8', 'hex');

// Document Service generates UUIDv5 identities from ordered JSON string tuples.
export function documentIdentity(prefix, parts) {
  const bytes = createHash('sha1').update(oidNamespace).update(JSON.stringify(parts)).digest();
  bytes[6] = (bytes[6] & 0x0f) | 0x50;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  return prefix + bytes.subarray(0, 16).toString('hex');
}
