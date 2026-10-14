import { screen } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { profileAuthoringTransport } from './test/profileAuthoring';

afterEach(() => window.history.replaceState({}, '', '/'));

it.each([
  '/profiles',
  '/profiles/agent-profiles/agp_research',
  '/profiles/workspace-profiles/wkp_research',
  '/profiles/sandbox-profiles/sbp_research',
])('retires the legacy profile authoring route %s', async (path) => {
  const test = profileAuthoringTransport();
  test.render('research-owner', path);

  expect(
    await screen.findByRole('heading', { name: 'Configure Agents directly' }),
  ).toBeInTheDocument();
  expect(test.writes()).toEqual([]);
  expect(test.requests.some(({ url }) => /profile|marketplace/.test(url.pathname))).toBe(false);
});
