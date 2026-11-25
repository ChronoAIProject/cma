import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { createClient } from '@cma/client';
import { AgentShareAccept } from './AgentShareAccept';
import {
  clearAgentShareToken,
  newAgentShareToken,
  retainAgentShareForSignIn,
  retainAgentShareToken,
} from './agentShareToken';

const token = 'ab'.repeat(32);
const link = {
  id: 'share-one',
  agent_id: 'agent-one',
  audience: { kind: 'public' },
  mode: 'view',
  shared_by: { subject: 'owner', display_name: 'Alex', avatar_url: null },
  created_at: '2026-10-22T00:00:00Z',
  revoked_at: null,
  revision: 1,
  accepted_count: 1,
};
beforeEach(() => {
  sessionStorage.clear();
  window.history.replaceState(null, '', `/agent-manager/share/share-one#${token}`);
});
afterEach(() => {
  sessionStorage.clear();
  window.history.replaceState(null, '', '/');
  vi.restoreAllMocks();
});

it('retains a link through OAuth without forwarding its secret and clears it after redemption', async () => {
  retainAgentShareForSignIn('share-one');
  expect(window.location.hash).toBe('');
  expect(window.location.pathname).toBe('/agent-manager/share/share-one');
  const accepted = vi.fn();
  const fetch = vi.fn<typeof globalThis.fetch>(async () => Response.json(link));
  const client = createClient({ baseUrl: 'https://cma.example', fetch });
  render(<AgentShareAccept client={client} shareId="share-one" onAccepted={accepted} />);
  await waitFor(() => expect(accepted).toHaveBeenCalledWith('agent-one'));
  expect(fetch).toHaveBeenCalledOnce();
  expect(String(fetch.mock.calls[0]?.[0])).not.toContain(token);
  expect(JSON.parse(String(fetch.mock.calls[0]?.[1]?.body))).toEqual({ token });
  expect(retainAgentShareToken('share-one')).toBeUndefined();
  expect(sessionStorage.length).toBe(0);
});

it('keeps an uncertain redemption retryable and does not treat a failed attempt as acceptance', async () => {
  const user = userEvent.setup();
  const accepted = vi.fn();
  const fetch = vi
    .fn()
    .mockResolvedValueOnce(Response.json({ detail: 'Unavailable' }, { status: 503 }))
    .mockImplementation(async () => Response.json(link));
  const client = createClient({ baseUrl: 'https://cma.example', fetch });
  render(<AgentShareAccept client={client} shareId="share-one" onAccepted={accepted} />);
  await screen.findByRole('button', { name: 'Try again' });
  expect(accepted).not.toHaveBeenCalled();
  expect(retainAgentShareToken('share-one')).toBe(token);
  await user.click(screen.getByRole('button', { name: 'Try again' }));
  await waitFor(() => expect(accepted).toHaveBeenCalledWith('agent-one'));
  expect(sessionStorage.length).toBe(0);
});

it.each([403, 404])('removes a refused or revoked link secret for response %s', async (status) => {
  const accepted = vi.fn();
  const client = createClient({
    baseUrl: 'https://cma.example',
    fetch: async () => Response.json({ detail: 'Access removed' }, { status }),
  });
  render(<AgentShareAccept client={client} shareId="share-one" onAccepted={accepted} />);
  expect(await screen.findByRole('alert')).toHaveTextContent('not shared with your account');
  expect(screen.queryByRole('button', { name: 'Try again' })).not.toBeInTheDocument();
  expect(accepted).not.toHaveBeenCalled();
  expect(sessionStorage.length).toBe(0);
});

it('rejects expired or another link’s retained token and generates 32 random bytes', () => {
  expect(newAgentShareToken()).toMatch(/^[a-f0-9]{64}$/);
  retainAgentShareForSignIn('share-one');
  expect(retainAgentShareToken('another-link')).toBeUndefined();
  vi.spyOn(Date, 'now').mockReturnValue(Date.now() + 31 * 60 * 1000);
  expect(retainAgentShareToken('share-one')).toBeUndefined();
  clearAgentShareToken('share-one');
});

it('leaves the original fragment available when sign-in cannot write session storage', () => {
  const write = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
    throw new DOMException('Storage is unavailable', 'SecurityError');
  });
  expect(() => retainAgentShareForSignIn('share-one')).toThrow('Allow session storage');
  expect(window.location.hash).toBe(`#${token}`);
  write.mockRestore();
  retainAgentShareForSignIn('share-one');
  expect(window.location.hash).toBe('');
  expect(retainAgentShareToken('share-one')).toBe(token);
});
