import { useState } from 'react';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { agentDraftKey, saveAgentDraft } from '../agent/draftStorage';
import { AuthContextProvider, useAuth } from './context';
import { IdentityError, IdentityNotice } from './IdentityNotice';
import type { AuthProvider, AuthState } from './types';

const signedIn: AuthState = {
  status: 'authenticated',
  user: {
    id: 'fixture-owner',
    handle: 'Fixture',
    capabilities: [],
    visitor: false,
  },
};
const outage: AuthState = {
  status: 'error',
  reason: 'identity_unavailable',
  message: 'The identity service is temporarily unavailable. Retrying shortly.',
};

function Draft() {
  const [draft, setDraft] = useState('');
  return (
    <textarea aria-label="Draft" value={draft} onChange={(event) => setDraft(event.target.value)} />
  );
}
function View() {
  const { state, signIn, signOut } = useAuth();
  return (
    <>
      <IdentityNotice />
      {state.status === 'authenticated' ? (
        <>
          <Draft />
          <button onClick={() => void signOut()}>Sign out</button>
        </>
      ) : state.status === 'error' ? (
        <IdentityError state={state} />
      ) : (
        <>
          <p>{state.status}</p>
          {state.status === 'anonymous' && <button onClick={() => void signIn()}>Sign in</button>}
        </>
      )}
    </>
  );
}
function setup(initial: AuthState = signedIn) {
  const current = vi.fn<() => Promise<AuthState>>().mockResolvedValue(initial);
  const provider: AuthProvider = { current, signIn: async () => {}, signOut: async () => {} };
  const view = render(
    <AuthContextProvider provider={provider}>
      <View />
    </AuthContextProvider>,
  );
  return { current, ...view };
}
beforeEach(() => {
  vi.useFakeTimers();
  vi.spyOn(Math, 'random').mockReturnValue(0);
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  sessionStorage.clear();
});

it.each(['same owner', 'different owner'] as const)(
  'retains the hidden Agent draft through anonymous checks until the %s signs in',
  async (nextOwner) => {
    const { current, unmount } = setup();
    await act(async () => {});
    const key = agentDraftKey(signedIn.user.id, 'agt_draft');
    saveAgentDraft(signedIn.user.id, 'agt_draft', {
      revision: 'draft-revision',
      content: 'Unsent Agent draft',
      attachments: [],
    });
    const saved = sessionStorage.getItem(key);
    expect(saved).not.toBeNull();

    current.mockResolvedValue({ status: 'anonymous' });
    await act(async () => fireEvent(window, new Event('focus')));
    expect(screen.getByText('anonymous')).toBeInTheDocument();
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
    expect(sessionStorage.getItem(key)).toBe(saved);

    unmount();
    const reopened = setup({ status: 'anonymous' });
    await act(async () => {});
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Sign in' })));
    expect(screen.getByText('anonymous')).toBeInTheDocument();
    expect(sessionStorage.getItem(key)).toBe(saved);

    reopened.current.mockResolvedValue({
      ...signedIn,
      user: { ...signedIn.user, id: nextOwner === 'same owner' ? signedIn.user.id : 'next-owner' },
    });
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Sign in' })));
    expect(screen.getByRole('textbox')).toBeVisible();
    expect(sessionStorage.getItem(key)).toBe(nextOwner === 'same owner' ? saved : null);
  },
);

it('keeps the exact draft node through outage, automatic retries and recovery', async () => {
  const { current } = setup();
  await act(async () => {});
  const draft = screen.getByRole('textbox', { name: 'Draft' });
  fireEvent.change(draft, { target: { value: 'My unsent message' } });
  current.mockResolvedValue(outage);
  await act(async () => {
    fireEvent(window, new Event('focus'));
  });
  expect(screen.getByText(/Your drafts are still here/)).toBeInTheDocument();
  expect(screen.getByRole('textbox')).toBe(draft);
  await act(async () => {
    await vi.advanceTimersByTimeAsync(3000);
  });
  expect(current).toHaveBeenCalledTimes(3);
  expect(draft).toHaveValue('My unsent message');
  current.mockResolvedValue(signedIn);
  await act(async () => {
    await vi.advanceTimersByTimeAsync(6000);
  });
  expect(screen.getByRole('textbox')).toBe(draft);
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  await act(async () => {
    await vi.advanceTimersByTimeAsync(60000);
  });
  expect(current).toHaveBeenCalledTimes(4);
});

it('retries initial identity failure and stops retries after explicit rejection', async () => {
  const { current } = setup(outage);
  await act(async () => {});
  expect(screen.getByRole('heading')).toHaveTextContent(
    'identity service is temporarily unavailable',
  );
  current.mockResolvedValue({ status: 'anonymous' });
  await act(async () => {
    await vi.advanceTimersByTimeAsync(3000);
  });
  expect(screen.getByText('anonymous')).toBeInTheDocument();
  expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
  await act(async () => {
    await vi.advanceTimersByTimeAsync(60000);
  });
  expect(current).toHaveBeenCalledTimes(2);
});

it('coalesces focus, online and manual retry while a check is pending', async () => {
  const { current } = setup(outage);
  await act(async () => {});
  let finish: (state: AuthState) => void = () => {};
  current.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Retry now' }));
    fireEvent(window, new Event('focus'));
    fireEvent(window, new Event('online'));
    fireEvent.click(screen.getByRole('button', { name: 'Retry now' }));
    await vi.advanceTimersByTimeAsync(3000);
  });
  expect(current).toHaveBeenCalledTimes(2);
  await act(async () => {
    finish(signedIn);
  });
  expect(screen.getByRole('textbox')).toBeInTheDocument();
});

it('ignores a pending result after sign-out and cancels timers on unmount', async () => {
  const { current, unmount } = setup();
  await act(async () => {});
  let finish: (state: AuthState) => void = () => {};
  current.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  await act(async () => {
    fireEvent(window, new Event('focus'));
  });
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Sign out' }));
  });
  await act(async () => {
    finish(signedIn);
  });
  expect(screen.getByText('anonymous')).toBeInTheDocument();
  unmount();
  await act(async () => {
    await vi.advanceTimersByTimeAsync(60000);
  });
  expect(current).toHaveBeenCalledTimes(2);
});
