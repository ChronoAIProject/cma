import { describe, expect, it } from 'vitest';
import type { Channel } from '../ChannelEditor';
import { channelRequest, type Fields } from './model';
import {
  hmacCredentialIdMessage,
  hmacKeyIdMessage,
  hmacReferenceMessage,
  hmacSecretMessage,
  validateHmacCredentials,
} from './hmacCredentials';

const secret = btoa('synthetic signing material');
const source = (key_id = 'events') => ({ type: 'custom', auth: { type: 'hmac', key_id } });
const credentials = {
  agent_key: 'synthetic-provider-value',
  hmac_secrets: [{ key_id: 'events', secret }],
};
const saved = { version: 3 } as Channel;

describe('HMAC base64 format (#3198)', () => {
  it.each([
    '',
    'printable-secret!',
    'Zg',
    'Zg=',
    'Zh==',
    'Zm9=',
    'Zg==\n',
    ' Zg==',
    '_w==',
    '-w==',
    '秘密',
  ])('rejects invalid standard base64 without reflecting the supplied value (%j)', (value) => {
    const message = hmacSecretMessage(value);
    expect(message).toBeTruthy();
    if (value.length > 4) expect(message).not.toContain(value);
  });

  it.each(['Zg==', 'Zm8=', 'Zm9v', '/w==', '+w==', secret, 'A'.repeat(4096)])(
    'accepts canonical base64, including the encoded length boundary',
    (value) => expect(hmacSecretMessage(value)).toBeUndefined(),
  );

  it('refuses values over the backend limit before transport', () => {
    expect(hmacSecretMessage('A'.repeat(4100))).toContain('4,096');
    expect(() =>
      channelRequest(
        { sources: [source()] },
        { ...credentials, hmac_secrets: [{ key_id: 'events', secret: 'invalid secret!' }] },
      ),
    ).toThrow('/credentials/hmac_secrets/0/secret: Use standard base64');
  });
});

describe('initial keys and replacement (#3196)', () => {
  it('serializes initial credentials without an edit-only replacement flag or secret transformation', () => {
    const body = channelRequest(
      { sources: [source()] },
      credentials,
      undefined,
      'synthetic-record',
    );
    expect(body.credentials).toEqual(credentials);
    expect(body).not.toHaveProperty('replace_hmac');
  });

  it('retains omitted saved keys and ignores an unselected local credential draft', () => {
    const body = channelRequest(
      { sources: [source()] },
      { hmac_secrets: [{ key_id: 'unsubmitted', secret: 'invalid draft' }] },
      saved,
    );
    expect(body).not.toHaveProperty('credentials');
  });

  it('validates an explicit complete replacement and permits clearing an unused list', () => {
    expect(
      channelRequest({ sources: [source()] }, { ...credentials, replace_hmac: true }, saved)
        .credentials,
    ).toEqual(credentials);
    expect(
      channelRequest({ sources: [] }, { replace_hmac: true, hmac_secrets: [] }, saved).credentials,
    ).toEqual({ hmac_secrets: [] });
    expect(() =>
      channelRequest({ sources: [source()] }, { replace_hmac: true, hmac_secrets: [] }, saved),
    ).toThrow('/sources/0/auth/key_id');
  });
});

describe('credential references (#3201)', () => {
  it('refuses a missing/mismatched source reference on create and replacement', () => {
    for (const create of [true, false]) {
      const values = { ...credentials, replace_hmac: !create };
      expect(() => validateHmacCredentials({ sources: [source('other')] }, values, create)).toThrow(
        '/sources/0/auth/key_id: Add an HMAC credential with this exact key ID',
      );
    }
    expect(() => channelRequest({ sources: [source()] }, { agent_key: 'synthetic' })).toThrow(
      '/sources/0/auth/key_id',
    );
  });

  it('checks every HMAC source against the shared list using exact IDs', () => {
    const draft = { sources: [source(), source('second')] };
    expect(() => validateHmacCredentials(draft, credentials, true)).toThrow(
      '/sources/1/auth/key_id',
    );
    const values = {
      ...credentials,
      hmac_secrets: [...credentials.hmac_secrets, { key_id: 'second', secret }],
    };
    expect(() => validateHmacCredentials(draft, values, true)).not.toThrow();
    for (const key of ['Events', 'events '])
      expect(hmacReferenceMessage(key, credentials, true)).toContain('exact key ID');
    expect(hmacReferenceMessage('events', credentials, true)).toBeUndefined();
  });

  it('does not guess which secrets exist when an edit retains saved credentials', () => {
    expect(hmacReferenceMessage('retained-key', {}, false)).toBeUndefined();
    expect(() =>
      validateHmacCredentials({ sources: [source('retained-key')] }, {}, false),
    ).not.toThrow();
  });

  it('enforces unique IDs and the backend UTF-8 byte boundaries', () => {
    expect(hmacKeyIdMessage('')).toBeTruthy();
    expect(hmacKeyIdMessage('界'.repeat(170))).toBeUndefined();
    expect(hmacKeyIdMessage('界'.repeat(171))).toBeTruthy();
    expect(hmacKeyIdMessage('a'.repeat(512))).toBeUndefined();
    expect(hmacKeyIdMessage('a'.repeat(513))).toBeTruthy();
    const duplicate = [...credentials.hmac_secrets, ...credentials.hmac_secrets];
    expect(hmacCredentialIdMessage('events', duplicate, 0)).toContain('different key ID');
    expect(() => validateHmacCredentials({}, { hmac_secrets: duplicate }, true)).toThrow(
      '/credentials/hmac_secrets/0/key_id',
    );
  });

  it('filters inactive HMAC drafts before validating or serializing agent-key auth (#3197)', () => {
    const draft = { sources: [{ type: 'custom', auth: { type: 'nyxid_key', key_hashes: [] } }] };
    const values = {
      agent_key: 'synthetic-provider',
      replace_hmac: true,
      hmac_secrets: [{ key_id: '', secret: 'invalid hidden draft' }],
    };
    expect(channelRequest(draft, values).credentials).toEqual({ agent_key: 'synthetic-provider' });
  });

  it('keeps credentials used by another active HMAC source or result destination', () => {
    const draft: Fields = {
      sources: [{ type: 'custom', auth: { type: 'nyxid_key', key_hashes: [] } }, source()],
    };
    expect(channelRequest(draft, credentials).credentials).toEqual(credentials);
    expect(
      channelRequest(
        {
          sources: [],
          reply: {
            sinks: [
              { type: 'result_webhook', url: 'https://example.test/result', hmac_key_id: 'events' },
            ],
          },
        },
        credentials,
      ).credentials,
    ).toEqual(credentials);
  });
});
