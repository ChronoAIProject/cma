import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { App } from './App';
import { AuthContextProvider } from './auth/context';
import { readFatherDraft } from './bots/fatherDraft';

afterEach(() => window.history.replaceState(null, '', '/'));

it('requires sign-in and retains the suggestion without passing its data to OAuth', async () => {
  const draft = { v: 1, name: 'My assistant', instructions: 'Help me plan research.' };
  window.history.replaceState(
    null,
    '',
    `/bots/new?setup=${encodeURIComponent(JSON.stringify(draft))}`,
  );
  const signIn = vi.fn(async () => {});
  render(
    <AuthContextProvider
      provider={{
        current: async () => ({ status: 'anonymous' }),
        signIn,
        signOut: async () => {},
      }}
    >
      <App />
    </AuthContextProvider>,
  );
  const user = userEvent.setup();
  expect(await screen.findByRole('button', { name: 'Get Started' })).toBeEnabled();
  expect(screen.queryByLabelText('Agent instructions')).not.toBeInTheDocument();
  expect(sessionStorage.length).toBe(0);
  await user.click(screen.getByRole('button', { name: 'Get Started' }));
  await waitFor(() => expect(signIn).toHaveBeenCalledWith('/bots/new', undefined));
  window.history.replaceState(null, '', '/bots/new');
  expect(readFatherDraft()).toEqual(draft);
});
