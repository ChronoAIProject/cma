import {createHash, createPublicKey, verify} from 'node:crypto';
import {Blocked} from '../../executable/runner.mjs';

export const need = (condition, code) => { if (!condition) throw new Blocked(code); };
export const digest = value => createHash('sha256').update(value).digest('hex');
export const SHA256 = /^[a-f0-9]{64}$/;
export const canonical = value => {
  if (Array.isArray(value)) return '[' + value.map(canonical).join(',') + ']';
  if (value && typeof value === 'object') return '{' + Object.keys(value).sort()
    .map(key => JSON.stringify(key) + ':' + canonical(value[key])).join(',') + '}';
  const encoded = JSON.stringify(value);
  need(encoded !== undefined, 'evidence_not_json');
  return encoded;
};

// This is a parent capture signature, NOT a signature format claimed to exist
// in NyxID. The registering adapter never receives the capture private key.
export function verifyCapture(envelope, trust, schema, challenge) {
  need(trust?.key_id && trust?.public_key_pem, 'independent_capture_trust_missing');
  need(envelope?.key_id === trust.key_id && envelope.payload?.schema === schema,
    'independent_capture_contract');
  const bytes = canonical(envelope.payload);
  need(Buffer.byteLength(bytes) <= 1024 * 1024, 'independent_capture_too_large');
  need(SHA256.test(envelope.sha256 ?? '') && digest(bytes) === envelope.sha256,
    'independent_capture_hash_mismatch');
  let valid = false;
  try {
    const key = createPublicKey(trust.public_key_pem);
    valid = key.asymmetricKeyType === 'ed25519' && verify(null, Buffer.from(bytes), key,
      Buffer.from(envelope.signature ?? '', 'base64'));
  } catch { /* Fail closed without emitting key or provider material. */ }
  need(valid, 'independent_capture_signature_invalid');
  need(envelope.payload.challenge === challenge, 'independent_capture_challenge_mismatch');
  // Copy verified bytes so subsequent adapter mutation cannot alter the evidence.
  return JSON.parse(bytes);
}
