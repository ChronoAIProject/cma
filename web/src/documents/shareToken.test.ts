import { beforeEach, expect, it, vi } from 'vitest';
import { documentShareUrl, readDocumentShareToken, retainDocumentShareToken } from './shareToken';
const id = `doc_${'a'.repeat(32)}`;
const token = 'b'.repeat(64);
beforeEach(() => {
  window.history.replaceState({}, '', '/');
  vi.restoreAllMocks();
});
it('retains fragment authority for sign-in and removes it from the visible route', () => {
  window.history.replaceState({}, '', `/artifacts/${id}#token=${token}`);
  expect(readDocumentShareToken(id)).toBe(token);
  retainDocumentShareToken(id, true);
  expect(window.location.hash).toBe('');
  expect(window.location.search).toBe('');
  expect(readDocumentShareToken(id)).toBe(token);
  expect(readDocumentShareToken(`doc_${'c'.repeat(32)}`)).toBeUndefined();
});
it('expires retained authority and rejects invalid link identities and tokens', () => {
  expect(() => documentShareUrl('../unsafe', token)).toThrow();
  expect(() => documentShareUrl(id, 'invalid')).toThrow();
  sessionStorage.setItem(
    `cma:document-share:${id}`,
    JSON.stringify({ token, expiresAt: Date.now() - 1 }),
  );
  expect(readDocumentShareToken(id)).toBeUndefined();
  window.history.replaceState({}, '', `/artifacts/${id}#token=invalid`);
  expect(readDocumentShareToken(id)).toBeUndefined();
});
it('refuses sign-in when fragment authority cannot be safely retained', () => {
  window.history.replaceState({}, '', `/artifacts/${id}#token=${token}`);
  vi.stubGlobal('sessionStorage', {
    setItem() {
      throw new Error('Blocked');
    },
  });
  try {
    expect(() => retainDocumentShareToken(id, true)).toThrow('Allow session storage');
    expect(window.location.hash).toContain(token);
  } finally {
    vi.unstubAllGlobals();
  }
});
