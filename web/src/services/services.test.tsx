import { render, screen, waitFor, fireEvent, cleanup, act } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { App } from '../App';
import { AuthContextProvider } from '../auth/context';
import { createMockAuthProvider } from '../auth/mock';
import { ServicePage } from './ServicePage';
import { ServiceMenu } from './ServiceMenu';
import { WriteForm } from './data';
import { parseRoute, routePath } from '../routing/routes';
afterEach(() => {
  cleanup();
  window.history.replaceState({}, '', '/');
  vi.unstubAllGlobals();
});
describe('independent CMA service navigation', () => {
  it.each(['refresh'] as const)(
    'preserves an in-flight and uncertain write across a failed %s',
    async (operation) => {
      const session = {
        authenticated: true,
        subject: 'owner',
        permissions: ['cma-eg:admin'],
        expires_at: Math.floor(Date.now() / 1000) + 3600,
      };
      let failWrite!: (error: Error) => void;
      const pendingWrite = new Promise<Response>((_resolve, reject) => {
        failWrite = reject;
      });
      const writes: RequestInit[] = [];
      let sessionChecks = 0;
      vi.stubGlobal(
        'fetch',
        vi.fn(async (url: string, init: RequestInit = {}) => {
          if (url.endsWith('/auth/session')) {
            if (++sessionChecks === 2 && operation === 'refresh')
              throw new TypeError('Session refresh interrupted');
            return new Response(JSON.stringify(session));
          }
          if (url.endsWith('/auth/logout')) throw new TypeError('Logout interrupted');
          writes.push(init);
          return writes.length === 1 ? pendingWrite : new Response('{}', { status: 201 });
        }),
      );
      const done = vi.fn();
      render(
        <ServicePage service="event-gateway">
          {(_, denied) => (
            <WriteForm
              service="event-gateway"
              path="/v1/channels"
              label="Create channel"
              fields={[{ name: 'name', label: 'Channel name', required: true }]}
              denied={denied}
              onDone={done}
            />
          )}
        </ServicePage>,
      );
      const input = await screen.findByLabelText('Channel name');
      fireEvent.change(input, { target: { value: 'retained channel' } });
      fireEvent.click(screen.getByRole('button', { name: 'Create channel' }));
      await waitFor(() => expect(writes).toHaveLength(1));
      fireEvent.focus(window);
      await screen.findByText('Session refresh interrupted');
      expect(screen.getByLabelText('Channel name')).toBe(input);
      expect(input).toHaveValue('retained channel');
      expect(screen.getByRole('button', { name: 'Submitting…' })).toBeDisabled();
      await act(async () => {
        failWrite(new TypeError('Write response interrupted'));
      });
      expect(await screen.findByRole('button', { name: 'Retry same request' })).toBeEnabled();
      fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
      await waitFor(() => expect(screen.queryByRole('button', { name: 'Try again' })).toBeNull());
      expect(screen.getByLabelText('Channel name')).toBe(input);
      fireEvent.click(screen.getByRole('button', { name: 'Retry same request' }));
      await waitFor(() => expect(done).toHaveBeenCalledOnce());
      expect(writes[1]?.body).toBe(writes[0]?.body);
      expect(writes[1]?.headers).toEqual(writes[0]?.headers);
    },
  );

  it('does not restore a denied service view from an earlier session check', async () => {
    const session = {
      authenticated: true,
      subject: 'owner',
      permissions: ['cma-eg:admin'],
      expires_at: Math.floor(Date.now() / 1000) + 3600,
    };
    let finishCheck!: (response: Response) => void;
    const pendingCheck = new Promise<Response>((resolve) => {
      finishCheck = resolve;
    });
    const fetcher = vi
      .fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(session)))
      .mockReturnValueOnce(pendingCheck);
    vi.stubGlobal('fetch', fetcher);
    render(
      <ServicePage service="event-gateway">
        {(_, denied) => <button onClick={() => denied()}>Simulate denied write</button>}
      </ServicePage>,
    );
    await screen.findByRole('button', { name: 'Simulate denied write' });
    fireEvent.focus(window);
    await waitFor(() => expect(fetcher).toHaveBeenCalledTimes(2));
    fireEvent.click(screen.getByRole('button', { name: 'Simulate denied write' }));
    await act(async () => {
      finishCheck(new Response(JSON.stringify(session)));
    });
    expect(screen.getByRole('link', { name: 'Sign in with NyxID' })).toBeVisible();
    expect(screen.queryByRole('button', { name: 'Simulate denied write' })).toBeNull();
  });

  it('keeps Image Manager unavailable through direct navigation without opening its service', async () => {
    window.history.replaceState({}, '', '/image-manager');
    const fetcher = vi.fn();
    vi.stubGlobal('fetch', fetcher);
    render(
      <AuthContextProvider provider={createMockAuthProvider({ initiallyAuthenticated: true })}>
        <App />
      </AuthContextProvider>,
    );
    await screen.findByRole('button', { name: /account menu/i });
    fireEvent.click(screen.getByRole('button', { name: /^Service: / }));
    expect(screen.queryByRole('menuitemradio', { name: /^Image Manager/ })).not.toBeInTheDocument();
    expect(screen.queryByText('Create an image')).not.toBeInTheDocument();
    expect(fetcher).not.toHaveBeenCalled();
  });

  it('uses enabled service URLs, and preserves modified clicks', () => {
    const navigate = vi.fn();
    render(<ServiceMenu route={{ kind: 'event_gateway' }} onNavigate={navigate} />);
    fireEvent.click(screen.getByRole('button', { name: 'Service: Event Gateway' }));
    expect(screen.getByRole('menuitemradio', { name: 'Event Gateway' })).toHaveAttribute(
      'aria-checked',
      'true',
    );
    expect(screen.getByRole('menuitemradio', { name: 'Event Gateway' })).toHaveAttribute(
      'href',
      '/event-gateway',
    );
    expect(screen.queryByRole('menuitemradio', { name: /^Image Manager/ })).not.toBeInTheDocument();
    const agent = screen.getByRole('menuitemradio', { name: 'Agent Manager' });
    expect(agent).toHaveAttribute('href', '/agent-manager');
    fireEvent.click(agent, { ctrlKey: true });
    expect(navigate).not.toHaveBeenCalled();
    fireEvent.click(agent);
    expect(navigate).toHaveBeenCalledWith({ kind: 'agent_manager' });
    for (const kind of ['agent_manager', 'event_gateway'] as const)
      expect(parseRoute(routePath({ kind }))).toEqual({ kind });
    expect(parseRoute('/image-manager')).toEqual({ kind: 'not_found', pathname: '/image-manager' });
  });

  it('names a consumer route as Event Gateway in the service menu', () => {
    render(<ServiceMenu route={{ kind: 'bots' }} onNavigate={vi.fn()} />);
    expect(screen.getByRole('button', { name: 'Service: Event Gateway' })).toBeInTheDocument();
  });

  it('a gateway authorization failure remains local to the gateway', async () => {
    const fetcher = vi.fn().mockResolvedValue(new Response('', { status: 401 }));
    vi.stubGlobal('fetch', fetcher);
    render(<ServicePage service="event-gateway">{() => <p>private channels</p>}</ServicePage>);
    expect(await screen.findByRole('link', { name: 'Sign in with NyxID' })).toHaveAttribute(
      'href',
      '/auth/login?return_to=%2Fevent-gateway',
    );
    expect(screen.queryByText('private channels')).toBeNull();
    expect(fetcher.mock.calls[0]?.[0]).toBe('/api/event-gateway/auth/session');
  });
  it('retries an uncertain channel write with its original body and key', async () => {
    const done = vi.fn();
    const fetcher = vi
      .fn()
      .mockRejectedValueOnce(new TypeError('Network failure'))
      .mockResolvedValueOnce(new Response('{}', { status: 201 }));
    vi.stubGlobal('fetch', fetcher);
    render(
      <WriteForm
        service="event-gateway"
        path="/v1/channels"
        label="Create channel"
        fields={[{ name: 'name', label: 'Channel name', required: true }]}
        denied={vi.fn()}
        onDone={done}
      />,
    );
    fireEvent.change(screen.getByLabelText('Channel name'), { target: { value: 'my-channel' } });
    fireEvent.click(screen.getByRole('button', { name: 'Create channel' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Retry same request' }));
    await waitFor(() => expect(done).toHaveBeenCalledOnce());
    expect(fetcher.mock.calls[0]?.[0]).toBe('/api/event-gateway/v1/channels');
    expect(fetcher.mock.calls[1]?.[1].body).toBe(fetcher.mock.calls[0]?.[1].body);
    expect(fetcher.mock.calls[1]?.[1].headers['Idempotency-Key']).toBe(
      fetcher.mock.calls[0]?.[1].headers['Idempotency-Key'],
    );
  });
});
