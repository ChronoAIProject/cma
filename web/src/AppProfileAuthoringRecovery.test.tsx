import { screen } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { profileAuthoringTransport } from './test/profileAuthoring';

afterEach(() => window.history.replaceState({}, '', '/'));

it.each([
  '/profiles/event-workers',
  '/profiles/event-workers/ewp_research',
  '/profiles/agent-profiles/new',
  '/profiles/workspace-profiles/new',
  '/profiles/sandbox-profiles/new',
])('does not start legacy profile authoring work after navigation to %s', async (path) => {
  const test = profileAuthoringTransport();
  test.render('research-owner', path);

  expect(
    await screen.findByRole('heading', { name: 'Configure Agents directly' }),
  ).toBeInTheDocument();
  expect(test.writes()).toEqual([]);
  expect(test.requests.some(({ url }) => /profile|marketplace/.test(url.pathname))).toBe(false);
});
