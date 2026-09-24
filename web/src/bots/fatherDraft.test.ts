import { afterEach, expect, it } from 'vitest';
import {
  clearFatherDraft,
  parseFatherDraft,
  readFatherDraft,
  retainFatherDraftForSignIn,
  validateFatherDraft,
} from './fatherDraft';

const draft = { v: 1, name: '研究助手', instructions: 'Explain papers.\n请用中文回答。' };
const search = (value: unknown) => `?setup=${encodeURIComponent(JSON.stringify(value))}`;
afterEach(() => window.history.replaceState(null, '', '/'));

it('accepts only a bounded plain-text suggestion and never interprets markup', () => {
  expect(parseFatherDraft(search(draft))).toEqual(draft);
  const markup = { ...draft, instructions: '<img src=x onerror=alert(1)> is quoted text.' };
  expect(parseFatherDraft(search(markup))).toEqual(markup);
  expect(parseFatherDraft('?unrelated=one')).toBeUndefined();
  expect(() => validateFatherDraft({ ...draft, name: '字'.repeat(27) })).toThrow();
  expect(() => validateFatherDraft({ ...draft, instructions: '字'.repeat(1366) })).toThrow();
  expect(() => validateFatherDraft({ ...draft, name: 'two\nlines' })).toThrow();
});

it.each([
  { ...draft, owner: 'other-person' },
  { ...draft, services: ['admin'] },
  { ...draft, callback_url: 'https://example.test' },
  { ...draft, token: 'not-a-credential' },
  { ...draft, v: 2 },
  { ...draft, name: '' },
  { ...draft, instructions: ' ' },
  null,
  [],
])('rejects unauthorized fields or malformed setup data: %j', (value) => {
  expect(() => parseFatherDraft(search(value))).toThrow();
});

it('rejects ambiguous and oversized links', () => {
  expect(() => parseFatherDraft(`${search(draft)}&setup=null`)).toThrow();
  expect(() => parseFatherDraft('?setup=%7B')).toThrow();
  expect(() => parseFatherDraft(`?setup=${'x'.repeat(25000)}`)).toThrow();
});

it('preserves only the reviewed suggestion through a clean sign-in return path', () => {
  window.history.replaceState(null, '', `/bots/new${search(draft)}`);
  expect(sessionStorage.length).toBe(0);
  retainFatherDraftForSignIn();
  window.history.replaceState(null, '', '/bots/new');
  expect(readFatherDraft()).toEqual(draft);
  clearFatherDraft();
  expect(readFatherDraft()).toBeUndefined();
  expect(sessionStorage.length).toBe(0);
});

it('does not capture setup data on unrelated routes or erase unrelated URL parameters', () => {
  window.history.replaceState(null, '', `/profiles${search(draft)}`);
  retainFatherDraftForSignIn();
  expect(sessionStorage.length).toBe(0);
  window.history.replaceState(null, '', `/bots/new${search(draft)}&from=telegram`);
  clearFatherDraft();
  expect(window.location.search).toBe('?from=telegram');
});
