// @vitest-environment node
import { describe, expect, it, vi } from 'vitest';
import { ChannelRecordManagementSchema } from '../cmaeg/types.js';
import { Cmaeg, ProtocolError, ValidationError } from '../index.js';

const recordId = '00000000-0000-4000-8000-000000000001';
const owner = '00000000-0000-4000-8000-000000000002';
const valid = {
  state: 'valid',
  name: 'Fixture channel',
  owner: { kind: 'organization', subject: owner },
  provider: { slug: 'fixture-provider', level: 'l1' },
  creation: 'complete',
  address: {
    state: 'available',
    channel_id: 'fixture-channel-id',
    endpoints: {
      custom_event_url: 'https://gateway.example/custom',
      nyxid_callback_url: 'https://gateway.example/callback',
    },
  },
};
const snapshot = (record: unknown) => ({ record_id: recordId, version: 7, record });

describe('channel record management', () => {
  it.each([
    valid,
    { ...valid, creation: 'legacy', address: { state: 'not_recorded' } },
    {
      ...valid,
      creation: 'legacy',
      address: { state: 'not_recorded' },
      retirement: { expected_version: 3, state: 'cleanup_unconfirmed' },
    },
    { ...valid, creation: 'legacy', address: { state: 'not_recorded' }, retirement: null },
    { ...valid, creation: 'incomplete', address: { state: 'unverifiable' } },
    {
      ...valid,
      creation: 'incomplete',
      pending_operation: 'delete',
      retirement_expected_version: 6,
    },
    { ...valid, retirement_expected_version: null },
    { ...valid, pending_operation: null },
    ...['create', 'update', 'profile_revision', 'delete', 'audit_routing', 'audit_attachments'].map(
      (pending_operation) => ({ ...valid, pending_operation }),
    ),
    { state: 'invalid' },
  ])('decodes the stored record variant %#', (record) => {
    expect(ChannelRecordManagementSchema(snapshot(record))).toStrictEqual(snapshot(record));
  });

  it('projects only the fields belonging to an invalid record', () => {
    expect(ChannelRecordManagementSchema(snapshot({ ...valid, state: 'invalid' }))).toEqual(
      snapshot({ state: 'invalid' }),
    );
  });

  it.each([
    snapshot({ state: 'unknown' }),
    snapshot({ state: 'valid' }),
    snapshot({ ...valid, creation: 'pending' }),
    snapshot({ ...valid, address: { state: 'available' } }),
    snapshot({ ...valid, address: { state: 'unknown' } }),
    snapshot({ ...valid, pending_operation: 'retry' }),
    { ...snapshot(valid), version: -1 },
    { ...snapshot(valid), version: Number.MAX_SAFE_INTEGER + 1 },
  ])('rejects malformed data %#', (response) => {
    expect(() => ChannelRecordManagementSchema(response)).toThrow(ProtocolError);
  });

  it('reads the management route with the optional organization scope', async () => {
    const response = snapshot(valid);
    const fetch = vi
      .fn<typeof globalThis.fetch>()
      .mockImplementation(async () => Response.json(response));
    const client = new Cmaeg({ baseUrl: 'https://gateway.example', fetch });
    await expect(client.getChannelRecordManagement(recordId, { owner })).resolves.toEqual(response);
    await expect(client.getChannelRecordManagement(recordId)).resolves.toEqual(response);
    expect(fetch).toHaveBeenCalledTimes(2);
    for (const [url, options] of fetch.mock.calls) {
      expect(url).toBe(`https://gateway.example/v1/channel-records/${recordId}/management`);
      expect(options?.method).toBe('GET');
      expect(options?.body).toBeUndefined();
    }
    expect(new Headers(fetch.mock.calls[0]?.[1]?.headers).get('X-CMAEG-Owner')).toBe(owner);
    expect(new Headers(fetch.mock.calls[1]?.[1]?.headers).has('X-CMAEG-Owner')).toBe(false);
    expect(() => client.getChannelRecordManagement(recordId, { owner: 'two words' })).toThrow(
      ValidationError,
    );
    expect(fetch).toHaveBeenCalledTimes(2);
  });
  it('retires the exact record version using the requested owner scope', async () => {
    const fetch = vi
      .fn<typeof globalThis.fetch>()
      .mockResolvedValue(new Response(null, { status: 204 }));
    const client = new Cmaeg({ baseUrl: 'https://gateway.example', fetch });
    await client.retireLegacyChannelRecord(recordId, 3, { owner });
    expect(fetch).toHaveBeenCalledTimes(1);
    const [url, options] = fetch.mock.calls[0]!;
    expect(url).toBe(
      `https://gateway.example/v1/channel-records/${recordId}/management?expected_version=3`,
    );
    expect(options?.method).toBe('DELETE');
    expect(new Headers(options?.headers).get('X-CMAEG-Owner')).toBe(owner);
  });
});
