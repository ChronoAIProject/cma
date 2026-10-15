import { render, screen, waitFor, fireEvent, cleanup, act } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { App } from '../App';
import { AuthContextProvider } from '../auth/context';
import { createMockAuthProvider } from '../auth/mock';
import { ServicePage } from './ServicePage';
import { ServiceTabs } from './ServiceTabs';
import { WriteForm } from './data';
import { parseRoute, routePath } from '../routing/routes';
afterEach(() => {
  cleanup();
  window.history.replaceState({}, '', '/');
  vi.unstubAllGlobals();
});
describe('independent CMA service navigation', () => {
  it.each(['refresh', 'logout'] as const)(
    'preserves an in-flight and uncertain write across a failed %s',
    async (operation) => {
      const session = {
        authenticated: true,
        subject: 'owner',
        permissions: ['cma-im:admin'],
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
        <ServicePage service="image-manager">
          {(_, denied) => (
            <WriteForm
              service="image-manager"
              path="/v1/images"
              label="Create image"
              fields={[{ name: 'name', label: 'Image name', required: true }]}
              denied={denied}
              onDone={done}
            />
          )}
        </ServicePage>,
      );
      const input = await screen.findByLabelText('Image name');
      fireEvent.change(input, { target: { value: 'retained image' } });
      fireEvent.click(screen.getByRole('button', { name: 'Create image' }));
      await waitFor(() => expect(writes).toHaveLength(1));
      if (operation === 'refresh') fireEvent.focus(window);
      else fireEvent.click(screen.getByRole('button', { name: 'Sign out of Image Manager' }));
      await screen.findByText(
        operation === 'refresh'
          ? 'Session refresh interrupted'
          : 'Sign-out could not be completed. Try again.',
      );
      expect(screen.getByLabelText('Image name')).toBe(input);
      expect(input).toHaveValue('retained image');
      expect(screen.getByRole('button', { name: 'Submitting…' })).toBeDisabled();
      await act(async () => {
        failWrite(new TypeError('Write response interrupted'));
      });
      expect(await screen.findByRole('button', { name: 'Retry same request' })).toBeEnabled();
      fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
      await waitFor(() => expect(screen.queryByRole('button', { name: 'Try again' })).toBeNull());
      expect(screen.getByLabelText('Image name')).toBe(input);
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
      permissions: ['cma-im:admin'],
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
      <ServicePage service="image-manager">
        {(_, denied) => <button onClick={denied}>Simulate denied write</button>}
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

  it('retains the original uncertain write when switching service tabs', async () => {
    window.history.replaceState({}, '', '/image-manager');
    const writes: RequestInit[] = [];
    const fetcher = vi.fn(async (url: string, init: RequestInit = {}) => {
      if (init.method === 'POST') {
        writes.push(init);
        if (writes.length === 1) throw new TypeError('Network failure');
        return new Response('{}', { status: 201 });
      }
      if (url.startsWith('/api/event-gateway/')) return new Response('{}', { status: 401 });
      return new Response(
        JSON.stringify(
          url.endsWith('/auth/session')
            ? {
                authenticated: true,
                subject: 'owner',
                permissions: ['cma-im:admin'],
                expires_at: Math.floor(Date.now() / 1000) + 3600,
              }
            : { items: [], next_cursor: null },
        ),
        { status: 200 },
      );
    });
    vi.stubGlobal('fetch', fetcher);
    render(
      <AuthContextProvider provider={createMockAuthProvider()}>
        <App />
      </AuthContextProvider>,
    );
    fireEvent.click(await screen.findByText('Create an image'));
    fireEvent.change(screen.getByLabelText('Image name'), { target: { value: 'retained-image' } });
    fireEvent.click(screen.getByRole('button', { name: 'Create image' }));
    await screen.findByRole('button', { name: 'Retry same request' });
    fireEvent.click(screen.getByRole('link', { name: 'Event Gateway' }));
    await screen.findByRole('link', { name: 'Sign in with NyxID' });
    fireEvent.click(screen.getByRole('link', { name: 'Image Manager' }));
    fireEvent.click(screen.getByRole('button', { name: 'Retry same request' }));
    await waitFor(() => expect(writes).toHaveLength(2));
    expect(writes[1]?.body).toBe(writes[0]?.body);
    expect(writes[1]?.headers).toEqual(writes[0]?.headers);
  });

  it('uses stable URLs and leaves modified clicks to the browser', () => {
    const navigate = vi.fn();
    render(<ServiceTabs route={{ kind: 'image_manager' }} onNavigate={navigate} />);
    expect(screen.getByRole('link', { name: 'Image Manager' }).getAttribute('aria-current')).toBe(
      'page',
    );
    fireEvent.click(screen.getByRole('link', { name: 'Event Gateway' }));
    expect(navigate).toHaveBeenCalledWith({ kind: 'event_gateway' });
    for (const kind of ['image_manager', 'event_gateway'] as const)
      expect(parseRoute(routePath({ kind }))).toEqual({ kind });
  });
  it('shows one home brand with its adjacent build and preserves modified navigation', () => {
    const navigate = vi.fn();
    render(<ServiceTabs route={{ kind: 'image_manager' }} onNavigate={navigate} />);
    const home = screen.getByRole('link', { name: 'CMA home' });
    const build = screen.getByLabelText(/^Build /);
    expect(home).toHaveAttribute('href', '/');
    expect(home.nextElementSibling).toBe(build);
    expect(build.tagName).toBe('SUB');
    fireEvent.click(home, { ctrlKey: true });
    expect(navigate).not.toHaveBeenCalled();
    fireEvent.click(home);
    expect(navigate).toHaveBeenCalledWith({ kind: 'home' });
    fireEvent.click(screen.getByRole('link', { name: 'Agent Manager' }));
    expect(navigate).toHaveBeenLastCalledWith({ kind: 'agent_manager' });
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
  it('retries an uncertain image write with its original body and key', async () => {
    const done = vi.fn();
    const fetcher = vi
      .fn()
      .mockRejectedValueOnce(new TypeError('Network failure'))
      .mockResolvedValueOnce(new Response('{}', { status: 201 }));
    vi.stubGlobal('fetch', fetcher);
    render(
      <WriteForm
        service="image-manager"
        path="/v1/images"
        label="Create image"
        fields={[{ name: 'name', label: 'Image name', required: true }]}
        denied={vi.fn()}
        onDone={done}
      />,
    );
    fireEvent.change(screen.getByLabelText('Image name'), { target: { value: 'my-image' } });
    fireEvent.click(screen.getByRole('button', { name: 'Create image' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Retry same request' }));
    await waitFor(() => expect(done).toHaveBeenCalledOnce());
    expect(fetcher.mock.calls[0]?.[0]).toBe('/api/image-manager/v1/images');
    expect(fetcher.mock.calls[1]?.[1].body).toBe(fetcher.mock.calls[0]?.[1].body);
    expect(fetcher.mock.calls[1]?.[1].headers['Idempotency-Key']).toBe(
      fetcher.mock.calls[0]?.[1].headers['Idempotency-Key'],
    );
  });
});
