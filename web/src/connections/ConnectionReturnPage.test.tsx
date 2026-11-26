import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ConnectionReturnPage } from './ConnectionReturnPage';
import { readSameTabReturn, rememberSameTabReturn } from './sameTabReturn';

const key = 'cma.connection-same-tab.v1';
const future = () => new Date(Date.now() + 600000).toISOString();

beforeEach(() => {
  vi.stubEnv('VITE_CONNECTION_INTENT_RECOVERY_ENABLED', 'true');
  sessionStorage.clear();
  window.history.replaceState(null, '', '/launches/original-launch');
});
afterEach(() => {
  vi.unstubAllEnvs();
  vi.restoreAllMocks();
  sessionStorage.clear();
  window.history.replaceState(null, '', '/');
});

it('returns to the saved route without trusting callback parameters or deleting recovery', () => {
  sessionStorage.setItem('unrelated-draft', 'keep');
  sessionStorage.setItem('cma.connection-recovery.v1.original', 'original intent');
  expect(rememberSameTabReturn('original-launch', future())).toBe(true);
  window.history.replaceState(
    null,
    '',
    '/connection-return?status=completed&return_to=https://other.example.test',
  );
  render(<ConnectionReturnPage />);
  const link = screen.getByRole('link', { name: 'Return to CMA' });
  expect(link).toHaveAttribute('href', '/launches/original-launch');
  expect(link).not.toHaveAttribute('target');
  expect(screen.getByText(/verify your connection before continuing/)).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Close this window' })).not.toBeInTheDocument();
  link.addEventListener('click', (event) => event.preventDefault());
  fireEvent.click(link);
  expect(readSameTabReturn()).toBeUndefined();
  expect(sessionStorage.getItem('unrelated-draft')).toBe('keep');
  expect(sessionStorage.getItem('cma.connection-recovery.v1.original')).toBe('original intent');
});

it.each([undefined, 'false', 'TRUE'])('retains the inert popup callback with flag %s', (flag) => {
  expect(rememberSameTabReturn('original-launch', future())).toBe(true);
  vi.stubEnv('VITE_CONNECTION_INTENT_RECOVERY_ENABLED', flag);
  render(<ConnectionReturnPage />);
  expect(screen.queryByRole('link')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Close this window' })).toBeInTheDocument();
});

it.each([
  'https://other.example.test',
  '//other.example.test',
  '/\\other.example.test',
  '/connection-return?status=completed',
  '/launches/original-launch#secret',
])('refuses unsafe or recursive return path %s', (path) => {
  sessionStorage.setItem(key, JSON.stringify({ path, expires: Date.now() + 600000 }));
  expect(readSameTabReturn()).toBeUndefined();
});

it.each(['not-json', 'null', '{"path":"/launches/old","expires":1}'])(
  'ignores malformed or expired navigation hint %s',
  (raw) => {
    sessionStorage.setItem(key, raw);
    render(<ConnectionReturnPage />);
    expect(screen.queryByRole('link')).not.toBeInTheDocument();
  },
);

it('refuses to leave when the return route cannot be stored', () => {
  vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
    throw new Error('storage unavailable');
  });
  expect(rememberSameTabReturn('original-launch', future())).toBe(false);
});

it('does not retain fragments or accept an expired handoff', () => {
  window.history.replaceState(null, '', '/launches/original-launch#private-fragment');
  expect(rememberSameTabReturn('original-launch', future())).toBe(true);
  expect(readSameTabReturn()).toBe('/launches/original-launch');
  expect(rememberSameTabReturn('original-launch', new Date(Date.now() - 1).toISOString())).toBe(
    false,
  );
});
