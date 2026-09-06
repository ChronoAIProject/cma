import { StrictMode } from 'react';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { App } from './App';
import { AuthContextProvider } from './auth/context';
import { createMockAuthProvider } from './auth/mock';
import { CmaClientProvider } from './api/context';
import { agentClient } from './test/agent';
import { PUBLIC_PREVIEW, PUBLIC_RECORD, publicClient } from './test/public';

afterEach(() => {
  window.history.replaceState({}, '', '/');
  sessionStorage.clear();
  vi.unstubAllGlobals();
});

it('keeps the public landing outside the auth gate and admits only the explicit sign-in intent', async () => {
  window.history.replaceState({}, '', '/t/public-example/plc_readme');
  vi.stubGlobal(
    'fetch',
    vi.fn(
      async () =>
        new Response(
          JSON.stringify({
            social_providers: [],
            email_signup: true,
            invite_code_required: false,
            register_available: true,
          }),
        ),
    ),
  );
  const visitor = publicClient();
  const provider = createMockAuthProvider();
  const view = render(
    <StrictMode>
      <AuthContextProvider provider={provider}>
        <CmaClientProvider client={agentClient()} publicClient={visitor}>
          <App />
        </CmaClientProvider>
      </AuthContextProvider>
    </StrictMode>,
  );
  await screen.findByRole('heading', { name: PUBLIC_PREVIEW.title });
  expect(screen.queryByRole('button', { name: 'Sign in' })).not.toBeInTheDocument();
  expect(visitor.requests.createOrResume).not.toHaveBeenCalled();
  await userEvent.setup().click(await screen.findByRole('button', { name: 'Open in CMA' }));
  await waitFor(() => expect(window.location.pathname).toBe(`/launches/${PUBLIC_RECORD.id}`));
  expect(visitor.requests.createOrResume).toHaveBeenCalledOnce();
  expect(await screen.findByText('Preparing software')).toBeInTheDocument();
  view.unmount();
});
