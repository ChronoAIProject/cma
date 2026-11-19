import { afterEach, expect, it, vi } from 'vitest';
import {
  outboundSeparateMessageEnabled,
  outboundStatus,
  validOutboundReceipt,
  type OutboundReceipt,
} from './outboundStatus';

const receipt = (patch: Partial<OutboundReceipt> = {}): OutboundReceipt => ({
  delivery_id: 'delivery',
  route_id: 'route',
  status: 'committed',
  ...patch,
});
afterEach(() => vi.unstubAllEnvs());

it.each([
  ['provider_failure', 'The provider reported an error.'],
  ['response_lost', 'The provider response was lost.'],
  ['receipt_invalid', 'The provider did not return a valid acknowledgement.'],
])('explains %s without granting a resend', (reason, message) => {
  const state = outboundStatus(receipt({ delivery: { outcome: 'unknown', reason } }));
  expect(state).toEqual({
    message: expect.stringContaining(message),
    send: false,
    cancel: false,
    check: true,
    newMessage: false,
    separateMessage: true,
  });
  expect(state.message).toContain('some or all of the message may have been sent');
});

it.each([
  undefined,
  null,
  { outcome: 'unknown' },
  { outcome: 'unknown', reason: 'delivery_in_progress_or_uncertain' },
] as const)('keeps missing or pending delivery evidence uncertain: %j', (delivery) => {
  expect(outboundStatus(receipt({ delivery }))).toMatchObject({
    message: expect.stringContaining('in progress or uncertain'),
    check: true,
    newMessage: false,
    separateMessage: false,
    send: false,
  });
});

it.each(['unknown', 'rejected'] as const)('does not display arbitrary %s reasons', (outcome) => {
  const state = outboundStatus(
    receipt({ delivery: { outcome, reason: '<html>private sentinel</html>' } }),
  );
  expect(state.message).toContain('could not be verified');
  expect(state.message).not.toContain('sentinel');
  expect(state).toMatchObject({ newMessage: false, separateMessage: false, send: false });
});

it.each(['no_credential', 'outbound_authority_changed', undefined])(
  'allows a fresh explicit message after rejection: %s',
  (reason) => {
    expect(outboundStatus(receipt({ delivery: { outcome: 'rejected', reason } }))).toMatchObject({
      message: expect.stringContaining('rejected without confirmed delivery'),
      newMessage: true,
      send: false,
      check: false,
    });
  },
);

it('distinguishes confirmed, cancelled, ready, uncertain and unsupported rendering', () => {
  expect(outboundStatus(receipt({ delivery: { outcome: 'confirmed' } }))).toMatchObject({
    message: 'The platform accepted the message.',
    newMessage: true,
    check: false,
  });
  expect(outboundStatus(receipt({ status: 'cancelled' }))).toMatchObject({
    message: 'Cancelled before sending.',
    newMessage: true,
    send: false,
  });
  const ready = receipt({
    status: 'prepared',
    expires_at: 2000,
    rendering: { outcome: 'degraded' },
  });
  expect(outboundStatus(ready, false, false, 1000)).toMatchObject({
    send: true,
    cancel: true,
    newMessage: false,
  });
  expect(outboundStatus(ready, true, false, 1000)).toMatchObject({
    send: false,
    cancel: false,
    check: true,
  });
  expect(
    outboundStatus({ ...ready, rendering: { outcome: 'unsupported' } }, false, false, 1000),
  ).toMatchObject({
    message: expect.stringContaining('could not be rendered'),
    send: false,
    cancel: true,
    newMessage: false,
  });
});

it('uses expiry to block send without inferring non-delivery from browser time', () => {
  const prepared = receipt({ status: 'prepared', expires_at: 100 });
  expect(outboundStatus(prepared, false, false, 100000)).toMatchObject({
    message: expect.stringContaining('may have expired'),
    send: false,
    cancel: true,
    newMessage: false,
  });
  expect(outboundStatus(prepared, true, true, 0)).toMatchObject({
    message: expect.stringContaining('can no longer be sent'),
    send: false,
    cancel: true,
    newMessage: false,
  });
  expect(
    outboundStatus(
      { ...prepared, status: 'committed', delivery: { outcome: 'confirmed' } },
      true,
      true,
    ),
  ).toMatchObject({ newMessage: true });
});

it.each([
  null,
  [],
  {},
  { status: ['prepared'] },
  { status: 'delivered' },
  { status: 'expired' },
  { expires_at: '100' },
  { expires_at: null },
  { expires_at: -1 },
  { expires_at: 1.5 },
  { expires_at: Number.MAX_SAFE_INTEGER },
  { rendering: null },
  { rendering: { outcome: 'failed' } },
  { rendering: { outcome: ['rendered'] } },
  { delivery: { outcome: 'rate_limited', retry_after: 10 } },
  { delivery: { outcome: 'unsupported' } },
  { delivery: { outcome: 'future' } },
  { delivery: { outcome: ['confirmed'] } },
  { delivery: { outcome: 'unknown', reason: {} } },
  { route_id: '' },
  { delivery_id: '' },
])('refuses malformed and future receipt values without safe replay: %j', (patch) => {
  const value =
    patch === null || Array.isArray(patch) || Object.keys(patch).length === 0
      ? patch
      : { ...receipt(), ...patch };
  expect(validOutboundReceipt(value)).toBe(false);
  expect(outboundStatus(value as OutboundReceipt)).toMatchObject({
    send: false,
    newMessage: false,
    separateMessage: false,
  });
});

it('reads old receipts and additive fields without trusting unknown reason text', () => {
  expect(validOutboundReceipt(receipt())).toBe(true);
  expect(
    validOutboundReceipt({
      ...receipt(),
      expires_at: 0,
      rendering: { outcome: 'rendered', blocks: [] },
      future: true,
    }),
  ).toBe(true);
  expect(
    outboundStatus(receipt({ status: 'prepared', delivery: { outcome: 'confirmed' } })),
  ).toMatchObject({ newMessage: false, send: false });
});

it.each([undefined, '', 'false', 'TRUE', '1', 'true'])(
  'requires explicit true for separate-message admission: %s',
  (value) => {
    vi.stubEnv('VITE_OUTBOUND_SEPARATE_MESSAGE_ENABLED', value);
    expect(outboundSeparateMessageEnabled()).toBe(value === 'true');
  },
);
