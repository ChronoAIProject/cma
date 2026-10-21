import { afterEach, expect, it } from 'vitest';
import { readSandboxTab, rememberSandboxTab, sandboxTabPath } from './sandboxTabRoute';

afterEach(() => {
  window.history.replaceState(null, '', '/');
});

it.each([
  ['?tab=workspaces', 'workspaces'],
  ['?tab=access&other=1', 'access'],
  ['?tab=activity', 'activity'],
  ['?tab=overview', 'overview'],
  ['?tab=settings', undefined],
  ['?tab=', undefined],
  ['', undefined],
] as const)('reads %j as %s', (search, tab) => {
  expect(readSandboxTab(search)).toBe(tab);
});

it('gives Overview the bare Sandbox path and every other tab a query', () => {
  expect(sandboxTabPath('sbx 1', 'overview')).toBe('/sandboxes/sbx%201');
  expect(sandboxTabPath('sbx_1', 'access')).toBe('/sandboxes/sbx_1?tab=access');
});

it('rewrites the current entry in place while the address is still the Sandbox', () => {
  window.history.replaceState(null, '', '/sandboxes/sbx_1');
  const length = window.history.length;
  rememberSandboxTab('sbx_1', 'workspaces');
  expect(window.location.search).toBe('?tab=workspaces');
  rememberSandboxTab('sbx_1', 'overview');
  expect(window.location.pathname).toBe('/sandboxes/sbx_1');
  expect(window.location.search).toBe('');
  expect(window.history.length).toBe(length);
});

it('never stamps a tab onto another page', () => {
  window.history.replaceState(null, '', '/agents/agt_1');
  rememberSandboxTab('sbx_1', 'access');
  expect(window.location.pathname).toBe('/agents/agt_1');
  expect(window.location.search).toBe('');
});
