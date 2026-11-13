import { act, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { AuthContextProvider, useAuth } from './context';
import type { AuthProvider, AuthState } from './types';

function Status() {
  return <p>{useAuth().state.status}</p>;
}

function provider(signInSilently?: () => boolean): AuthProvider {
  return {
    current: async () => ({ status: 'anonymous' }),
    signIn: async () => {},
    signOut: async () => {},
    signInSilently,
  };
}

describe('the identity check', () => {
  it('stays checking while a silent sign-in leaves the page', async () => {
    const signInSilently = vi.fn(() => true);
    render(
      <AuthContextProvider provider={provider(signInSilently)}>
        <Status />
      </AuthContextProvider>,
    );
    await vi.waitFor(() => expect(signInSilently).toHaveBeenCalledTimes(1));
    expect(screen.getByText('checking')).toBeInTheDocument();
  });

  it('reports anonymous when no silent sign-in was started', async () => {
    render(
      <AuthContextProvider provider={provider(() => false)}>
        <Status />
      </AuthContextProvider>,
    );
    expect(await screen.findByText('anonymous')).toBeInTheDocument();
  });

  it('reports anonymous for a provider that cannot sign in silently', async () => {
    render(
      <AuthContextProvider provider={provider()}>
        <Status />
      </AuthContextProvider>,
    );
    expect(await screen.findByText('anonymous')).toBeInTheDocument();
  });

  it('shows the landing when a silent sign-in never leaves the page', async () => {
    vi.useFakeTimers();
    try {
      render(
        <AuthContextProvider provider={provider(() => true)}>
          <Status />
        </AuthContextProvider>,
      );
      await act(async () => {});
      expect(screen.getByText('checking')).toBeInTheDocument();
      await act(async () => {
        vi.advanceTimersByTime(10000);
      });
      expect(screen.getByText('anonymous')).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it('ignores focus while a silent sign-in is leaving the page', async () => {
    const current = vi.fn(async () => ({ status: 'anonymous' }) as const);
    render(
      <AuthContextProvider provider={{ ...provider(() => true), current }}>
        <Status />
      </AuthContextProvider>,
    );
    await vi.waitFor(() => expect(current).toHaveBeenCalledTimes(1));
    await act(async () => {
      window.dispatchEvent(new Event('focus'));
    });
    expect(current).toHaveBeenCalledTimes(1);
    expect(screen.getByText('checking')).toBeInTheDocument();
  });

  it('never leaves the page from a later refresh', async () => {
    const signInSilently = vi.fn(() => false);
    render(
      <AuthContextProvider provider={provider(signInSilently)}>
        <Status />
      </AuthContextProvider>,
    );
    expect(await screen.findByText('anonymous')).toBeInTheDocument();
    signInSilently.mockReturnValue(true);
    await act(async () => {
      window.dispatchEvent(new Event('focus'));
    });
    expect(screen.getByText('anonymous')).toBeInTheDocument();
    expect(signInSilently).toHaveBeenCalledTimes(1);
  });

  it('still checks when focus arrives before the opening read resolves', async () => {
    const reads: ((state: AuthState) => void)[] = [];
    const current = vi.fn(() => new Promise<AuthState>((resolve) => reads.push(resolve)));
    const signInSilently = vi.fn(() => true);
    render(
      <AuthContextProvider provider={{ ...provider(signInSilently), current }}>
        <Status />
      </AuthContextProvider>,
    );
    await vi.waitFor(() => expect(current).toHaveBeenCalledTimes(1));
    await act(async () => {
      window.dispatchEvent(new Event('focus'));
    });
    expect(current).toHaveBeenCalledTimes(1);
    await act(async () => {
      for (const resolve of reads) resolve({ status: 'anonymous' });
    });
    expect(signInSilently).toHaveBeenCalledTimes(1);
    expect(screen.getByText('checking')).toBeInTheDocument();
  });
});

describe('silent sign-in navigation and draft races', () => {
  it('checks after SPA navigation from a skipped page, never on lifecycle refreshes', async () => {
    const silent = vi.fn(() => false);
    render(
      <AuthContextProvider provider={provider(silent)}>
        <Status />
      </AuthContextProvider>,
    );
    expect(await screen.findByText('anonymous')).toBeInTheDocument();
    for (const name of ['focus', 'online', 'visibilitychange']) {
      await act(async () => {
        (name === 'visibilitychange' ? document : window).dispatchEvent(new Event(name));
      });
    }
    expect(silent).toHaveBeenCalledTimes(1);
    silent.mockReturnValue(true);
    await act(async () => {
      window.dispatchEvent(new Event('cma:navigate'));
    });
    expect(silent).toHaveBeenCalledTimes(2);
  });

  it('retains the opening chance across an identity outage', async () => {
    const silent = vi.fn(() => true);
    const current = vi
      .fn<() => Promise<AuthState>>()
      .mockResolvedValueOnce({
        status: 'error',
        reason: 'identity_unavailable',
        message: 'temporary',
      })
      .mockResolvedValue({ status: 'anonymous' });
    render(
      <AuthContextProvider provider={{ ...provider(silent), current }}>
        <Status />
      </AuthContextProvider>,
    );
    expect(await screen.findByText('error')).toBeInTheDocument();
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 3600));
    });
    expect(silent).toHaveBeenCalledTimes(1);
  });

  it('shows anonymous without an error if starting silent navigation throws', async () => {
    render(
      <AuthContextProvider
        provider={provider(() => {
          throw new Error('navigation refused');
        })}
      >
        <Status />
      </AuthContextProvider>,
    );
    expect(await screen.findByText('anonymous')).toBeInTheDocument();
  });

  it('keeps unsubmitted drafts and creation keys through a stopped silent navigation', async () => {
    vi.useFakeTimers();
    sessionStorage.setItem('cma:task-viewer', 'owner-a');
    sessionStorage.setItem('cma:task-draft:owner-a', 'unsubmitted input');
    sessionStorage.setItem('cma:agent-creation:owner-a', 'original-command');
    try {
      render(
        <AuthContextProvider provider={provider(() => true)}>
          <Status />
        </AuthContextProvider>,
      );
      await act(async () => {});
      await act(async () => {
        vi.advanceTimersByTime(10000);
      });
      expect(screen.getByText('anonymous')).toBeInTheDocument();
      expect(sessionStorage.getItem('cma:task-draft:owner-a')).toBe('unsubmitted input');
      expect(sessionStorage.getItem('cma:agent-creation:owner-a')).toBe('original-command');
    } finally {
      vi.useRealTimers();
      sessionStorage.clear();
    }
  });

  it('does not expose the previous owner draft after a different owner signs in', async () => {
    sessionStorage.setItem('cma:task-viewer', 'owner-a');
    sessionStorage.setItem('cma:task-draft:owner-a', 'private input');
    const current = vi
      .fn<() => Promise<AuthState>>()
      .mockResolvedValueOnce({ status: 'anonymous' })
      .mockResolvedValue({
        status: 'authenticated',
        user: { id: 'owner-b', handle: 'b', capabilities: [], visitor: false },
      });
    render(
      <AuthContextProvider provider={{ ...provider(() => false), current }}>
        <Status />
      </AuthContextProvider>,
    );
    expect(await screen.findByText('anonymous')).toBeInTheDocument();
    expect(sessionStorage.getItem('cma:task-draft:owner-a')).toBe('private input');
    await act(async () => {
      window.dispatchEvent(new Event('focus'));
    });
    expect(screen.getByText('authenticated')).toBeInTheDocument();
    expect(sessionStorage.getItem('cma:task-draft:owner-a')).toBeNull();
    sessionStorage.clear();
  });
});

it.each(['focus', 'online', 'visibilitychange'])(
  'does not inherit navigation permission on %s after a navigation outage',
  async (event) => {
    const silent = vi.fn(() => false);
    const current = vi
      .fn<() => Promise<AuthState>>()
      .mockResolvedValueOnce({ status: 'anonymous' })
      .mockResolvedValueOnce({ status: 'error', reason: 'identity_unavailable', message: 'outage' })
      .mockResolvedValue({ status: 'anonymous' });
    render(
      <AuthContextProvider provider={{ ...provider(silent), current }}>
        <Status />
      </AuthContextProvider>,
    );
    expect(await screen.findByText('anonymous')).toBeInTheDocument();
    await act(async () => {
      window.dispatchEvent(new Event('cma:navigate'));
    });
    expect(screen.getByText('error')).toBeInTheDocument();
    silent.mockReturnValue(true);
    await act(async () => {
      (event === 'visibilitychange' ? document : window).dispatchEvent(new Event(event));
    });
    expect(screen.getByText('anonymous')).toBeInTheDocument();
    expect(silent).toHaveBeenCalledTimes(1);
  },
);

it('queues an explicit navigation check behind a background read without overlapping reads', async () => {
  const silent = vi.fn(() => false);
  let finish: (state: AuthState) => void = () => {};
  const current = vi
    .fn<() => Promise<AuthState>>()
    .mockResolvedValueOnce({ status: 'anonymous' })
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    )
    .mockResolvedValue({ status: 'anonymous' });
  render(
    <AuthContextProvider provider={{ ...provider(silent), current }}>
      <Status />
    </AuthContextProvider>,
  );
  expect(await screen.findByText('anonymous')).toBeInTheDocument();
  await act(async () => {
    window.dispatchEvent(new Event('focus'));
  });
  await act(async () => {
    window.dispatchEvent(new Event('cma:navigate'));
  });
  expect(current).toHaveBeenCalledTimes(2);
  silent.mockReturnValue(true);
  await act(async () => {
    finish({ status: 'anonymous' });
  });
  expect(current).toHaveBeenCalledTimes(3);
  expect(silent).toHaveBeenCalledTimes(2);
  expect(screen.getByText('checking')).toBeInTheDocument();
});

it('clears private drafts before calling the navigation-producing sign-out provider', async () => {
  sessionStorage.setItem('cma:task-viewer', 'owner-a');
  sessionStorage.setItem('cma:task-draft:owner-a', 'private input');
  sessionStorage.setItem('cma:agent-creation:owner-a', 'private command');
  let finish = () => {};
  const signOut = vi.fn(() => {
    expect(sessionStorage.getItem('cma:task-draft:owner-a')).toBeNull();
    expect(sessionStorage.getItem('cma:agent-creation:owner-a')).toBeNull();
    expect(sessionStorage.getItem('cma:task-viewer')).toBeNull();
    return new Promise<void>((resolve) => {
      finish = resolve;
    });
  });
  function SignOut() {
    const auth = useAuth();
    return <button onClick={() => void auth.signOut()}>Sign out</button>;
  }
  render(
    <AuthContextProvider provider={{ ...provider(), signOut }}>
      <SignOut />
    </AuthContextProvider>,
  );
  await act(async () => {
    screen.getByRole('button').click();
  });
  expect(signOut).toHaveBeenCalledTimes(1);
  await act(async () => {
    finish();
  });
});
