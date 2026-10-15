import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ServicePage } from './ServicePage';

afterEach(() => vi.unstubAllGlobals());
const session = (subject = 'owner') => ({
  authenticated: true,
  subject,
  permissions: ['cma-eg:channels:read'],
  expires_at: 9999999999,
});
it('opens Event Gateway from the CMA session without an independent sign-in flow', async () => {
  const fetcher = vi.fn().mockResolvedValue(new Response(JSON.stringify(session())));
  vi.stubGlobal('fetch', fetcher);
  render(
    <ServicePage service="event-gateway">{(value) => <p>{value.subject} channels</p>}</ServicePage>,
  );
  expect(await screen.findByText('owner channels')).toBeInTheDocument();
  expect(screen.queryByRole('link', { name: 'Sign in with NyxID' })).not.toBeInTheDocument();
  expect(screen.queryByRole('link', { name: 'Sign out of CMA' })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: /Sign out/ })).not.toBeInTheDocument();
  expect(fetcher).toHaveBeenCalledWith(
    '/api/event-gateway/auth/session',
    expect.objectContaining({ credentials: 'same-origin' }),
  );
});
it('returns a signed-out visitor through the Agent Manager OAuth entry point', async () => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('{}', { status: 401 })));
  render(<ServicePage service="event-gateway">{() => <p>private channels</p>}</ServicePage>);
  expect(await screen.findByRole('link', { name: 'Sign in with NyxID' })).toHaveAttribute(
    'href',
    '/auth/login?return_to=%2Fevent-gateway',
  );
  expect(screen.queryByText('private channels')).not.toBeInTheDocument();
});
it('keeps authenticated forms mounted and passes a retry notice into the portal frame', async () => {
  const fetcher = vi
    .fn()
    .mockResolvedValueOnce(new Response(JSON.stringify(session())))
    .mockRejectedValueOnce(new TypeError('Session refresh interrupted'))
    .mockResolvedValueOnce(new Response(JSON.stringify(session())));
  vi.stubGlobal('fetch', fetcher);
  render(
    <ServicePage service="event-gateway" renderStatus={(content) => <aside>{content}</aside>}>
      {(_value, _denied, notice) => (
        <main>
          {notice}
          <label>
            Draft
            <input />
          </label>
        </main>
      )}
    </ServicePage>,
  );
  const input = await screen.findByLabelText('Draft');
  fireEvent.change(input, { target: { value: 'Unsaved channel' } });
  await act(async () => {
    fireEvent.focus(window);
  });
  expect(await screen.findByRole('alert')).toHaveTextContent('Session refresh interrupted');
  expect(screen.getByLabelText('Draft')).toBe(input);
  expect(input).toHaveValue('Unsaved channel');
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
  });
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(screen.getByLabelText('Draft')).toBe(input);
});
it('replaces owned content on an account change and removes it when the common session expires', async () => {
  const fetcher = vi
    .fn()
    .mockResolvedValueOnce(new Response(JSON.stringify(session())))
    .mockResolvedValueOnce(new Response(JSON.stringify(session('new-owner'))))
    .mockResolvedValueOnce(new Response('{}', { status: 401 }));
  vi.stubGlobal('fetch', fetcher);
  render(
    <ServicePage service="event-gateway">{(value) => <p>{value.subject} channels</p>}</ServicePage>,
  );
  await screen.findByText('owner channels');
  await act(async () => {
    fireEvent.focus(window);
  });
  expect(await screen.findByText('new-owner channels')).toBeInTheDocument();
  expect(screen.queryByText('owner channels')).not.toBeInTheDocument();
  await act(async () => {
    fireEvent.focus(window);
  });
  await screen.findByRole('link', { name: 'Sign in with NyxID' });
  expect(screen.queryByText('new-owner channels')).not.toBeInTheDocument();
});
