import { afterEach, expect, it, vi } from 'vitest';
import {
  clearConnectionReturn,
  readConnectionReturn,
  rememberConnectionReturn,
} from './connectionReturn';

afterEach(() => {
  sessionStorage.clear();
  vi.restoreAllMocks();
});

it.each([
  'invalid',
  'null',
  '{}',
  '{"handoff_id":""}',
  '{"handoff_id":"one","expires_at":"invalid"}',
  '{"handoff_id":"one","expires_at":"2000-01-01T00:00:00Z"}',
])('rejects malformed or expired connection return %s', (raw) => {
  sessionStorage.setItem('cma:connection-return:request:service', raw);
  expect(readConnectionReturn('request', 'service')).toBeUndefined();
});

it('isolates recovery by request and service and removes it after completion', () => {
  const value = { handoff_id: 'one', expires_at: new Date(Date.now() + 60_000).toISOString() };
  rememberConnectionReturn('request', 'service', value);
  expect(readConnectionReturn('other', 'service')).toBeUndefined();
  expect(readConnectionReturn('request', 'other')).toBeUndefined();
  expect(readConnectionReturn('request', 'service')).toEqual(value);
  clearConnectionReturn('request', 'service');
  expect(readConnectionReturn('request', 'service')).toBeUndefined();
});

it('keeps recovery optional when browser storage is blocked', () => {
  vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
    throw new Error('blocked');
  });
  vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
    throw new Error('blocked');
  });
  vi.spyOn(Storage.prototype, 'removeItem').mockImplementation(() => {
    throw new Error('blocked');
  });
  expect(readConnectionReturn('request', 'service')).toBeUndefined();
  expect(() =>
    rememberConnectionReturn('request', 'service', { handoff_id: 'one', expires_at: 'unused' }),
  ).not.toThrow();
  expect(() => clearConnectionReturn('request', 'service')).not.toThrow();
});
