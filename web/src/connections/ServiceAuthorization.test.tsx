import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { navigateToServiceAuthorization } from '../auth/serviceAuthorization';
import { taskDraftStorageKey } from '../create/taskDraft';
import { useConnectionGate } from './useConnectionGate';

vi.mock('../auth/serviceAuthorization', async (load) => ({
  ...(await load<typeof import('../auth/serviceAuthorization')>()),
  navigateToServiceAuthorization: vi.fn(),
}));

const target = { type: 'sandbox', id: 'sandbox-a' } as const;
const draftKey = taskDraftStorageKey('owner-a');
const consentUrl = 'https://provider.example/approve';

function Draft({ submit }: { readonly submit: () => void }) {
  const connection = useConnectionGate();
  return (
    <>
      <input readOnly aria-label="Draft" value={sessionStorage.getItem(draftKey) ?? ''} />
      <button onClick={() => connection.run(target, submit)}>Send draft</button>
      {connection.dialog}
    </>
  );
}

function setup(granted: () => Promise<Response>) {
  const submit = vi.fn();
  const popup = { opener: {}, close: vi.fn(), closed: false };
  vi.spyOn(window, 'open').mockReturnValue(popup as unknown as Window);
  const fetcher = vi.fn(async (path: string, options: RequestInit) => {
    if (path === '/auth/service-authorization/start') {
      expect(options).toMatchObject({ method: 'POST', credentials: 'same-origin' });
      expect(JSON.parse(String(options.body))).toEqual({ return_to: '/', target });
      return granted();
    }
    expect(path).toBe('/auth/connections/check');
    return Response.json({
      owner: 'owner-a',
      ready: false,
      service_authorization_recovery: true,
      services: [
        {
          service: 'api-github',
          display_name: 'GitHub',
          kind: 'nyxid',
          state: 'missing',
          connection_mode: 'hosted',
        },
      ],
    });
  });
  vi.stubGlobal('fetch', fetcher);
  sessionStorage.setItem(draftKey, 'Review the repository');
  const view = render(<Draft submit={submit} />);
  fireEvent.click(screen.getByRole('button', { name: 'Send draft' }));
  return { view, submit, popup };
}

afterEach(() => {
  sessionStorage.clear();
  vi.clearAllMocks();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

it('leaves the original draft and discards the queued action before navigating', async () => {
  const { view, submit, popup } = setup(async () =>
    Response.json({ owner: 'owner-a', hosted_url: consentUrl }),
  );
  vi.mocked(navigateToServiceAuthorization).mockImplementationOnce(() => {
    expect(submit).not.toHaveBeenCalled();
    expect(sessionStorage.getItem(draftKey)).toBe('Review the repository');
  });
  fireEvent.click(await screen.findByRole('button', { name: 'Authorize required services' }));
  await waitFor(() =>
    expect(navigateToServiceAuthorization).toHaveBeenCalledWith(popup, consentUrl),
  );
  expect(popup.opener).toBeNull();
  expect(popup.close).not.toHaveBeenCalled();
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  view.unmount();
  render(<Draft submit={submit} />);
  expect(screen.getByRole('textbox', { name: 'Draft' })).toHaveValue('Review the repository');
  expect(submit).not.toHaveBeenCalled();
});

it('preserves the draft when the authorization window is closed before consent loads', async () => {
  let finish!: (response: Response) => void;
  const { submit, popup } = setup(
    () =>
      new Promise<Response>((resolve) => {
        finish = resolve;
      }),
  );
  fireEvent.click(await screen.findByRole('button', { name: 'Authorize required services' }));
  popup.closed = true;
  await act(async () => finish(Response.json({ owner: 'owner-a', hosted_url: consentUrl })));
  expect(navigateToServiceAuthorization).not.toHaveBeenCalled();
  expect(submit).not.toHaveBeenCalled();
  expect(screen.getByRole('textbox', { name: 'Draft' })).toHaveValue('Review the repository');
});

it('ignores a delayed start response after closing the dialog', async () => {
  let finish!: (response: Response) => void;
  const { submit } = setup(
    () =>
      new Promise<Response>((resolve) => {
        finish = resolve;
      }),
  );
  fireEvent.click(await screen.findByRole('button', { name: 'Authorize required services' }));
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  await act(async () => finish(Response.json({ owner: 'owner-a', hosted_url: consentUrl })));
  expect(navigateToServiceAuthorization).not.toHaveBeenCalled();
  expect(submit).not.toHaveBeenCalled();
  expect(sessionStorage.getItem(draftKey)).toBe('Review the repository');
});

it('rejects a response for another account without leaving the draft', async () => {
  const { submit } = setup(async () => Response.json({ owner: 'owner-b', hosted_url: consentUrl }));
  fireEvent.click(await screen.findByRole('button', { name: 'Authorize required services' }));
  expect(await screen.findByRole('alert')).toHaveTextContent('Your draft is still here.');
  expect(navigateToServiceAuthorization).not.toHaveBeenCalled();
  expect(submit).not.toHaveBeenCalled();
  expect(sessionStorage.getItem(draftKey)).toBe('Review the repository');
});
