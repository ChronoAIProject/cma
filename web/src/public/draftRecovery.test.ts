import { afterEach, expect, it, vi } from 'vitest';
import { clearDraftRecovery, readDraftRecovery, writeDraftRecovery } from './draftRecovery';

afterEach(() => {
  vi.unstubAllGlobals();
  sessionStorage.clear();
  vi.restoreAllMocks();
});

const recovery = {
  owner: 'original-owner',
  confirmedDigest: 'approved-one',
  command: {
    key: 'original-command-key',
    body: { trust_confirmed_digest: 'approved-one', acknowledgements: ['reviewed'] },
  },
};

it('retains the original owner and exact unresolved Start packet per Launch without expiring it', () => {
  writeDraftRecovery('launch-one', recovery);
  writeDraftRecovery('launch-two', { owner: 'second-owner' });
  expect(readDraftRecovery('launch-one')).toEqual(recovery);
  expect(readDraftRecovery('launch-two')).toEqual({ owner: 'second-owner' });
  clearDraftRecovery('launch-two');
  expect(readDraftRecovery('launch-two')).toBeUndefined();
  expect(readDraftRecovery('launch-one')).toEqual(recovery);
});

it.each([
  null,
  { owner: '' },
  { owner: 'owner', confirmedDigest: false },
  { owner: 'owner', command: {} },
  { owner: 'owner', command: { key: 'key', body: null } },
  { owner: 'owner', command: { key: 'key', body: { acknowledgements: [42] } } },
])('rejects malformed stored Draft recovery %j', (value) => {
  sessionStorage.setItem('cma:draft-credential-recovery:launch-one', JSON.stringify(value));
  expect(() => readDraftRecovery('launch-one')).toThrow('Saved request could not be read');
});

it('distinguishes an absent record from malformed JSON and a denied storage getter', () => {
  expect(readDraftRecovery('launch-one')).toBeUndefined();
  sessionStorage.setItem('cma:draft-credential-recovery:launch-one', '{');
  expect(() => readDraftRecovery('launch-one')).toThrow('Saved request could not be read');
  expect(sessionStorage.getItem('cma:draft-credential-recovery:launch-one')).toBe('{');
  vi.spyOn(window, 'sessionStorage', 'get').mockImplementation(() => {
    throw new DOMException('Synthetic storage denial', 'SecurityError');
  });
  expect(() => readDraftRecovery('launch-one')).toThrow('Saved request could not be read');
  vi.restoreAllMocks();
});

it('preserves the original body property order through a storage round trip', () => {
  const original = {
    ...recovery,
    command: {
      ...recovery.command,
      body: { acknowledgements: ['reviewed'], trust_confirmed_digest: 'approved-one' },
    },
  };
  writeDraftRecovery('launch-one', original);
  expect(JSON.stringify(readDraftRecovery('launch-one')?.command?.body)).toBe(
    JSON.stringify(original.command.body),
  );
});

it('reports an unavailable write before dispatch and never hides an accepted result on cleanup', () => {
  vi.stubGlobal('sessionStorage', {
    setItem: () => {
      throw new Error('unavailable');
    },
    removeItem: () => {
      throw new Error('unavailable');
    },
  });
  expect(() => writeDraftRecovery('launch-one', recovery)).toThrow('unavailable');
  expect(() => clearDraftRecovery('launch-one')).not.toThrow();
});
