import { act, fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { DocumentNavigationGuard } from '../documents/DocumentNavigationGuard';
import { requestNavigation } from '../routing/navigationGuard';
import { AuthContextProvider, useAuth } from './context';
import type { AuthProvider, AuthState } from './types';

const signedIn: AuthState = {
  status: 'authenticated',
  user: { id: 'owner', handle: 'owner', capabilities: [], visitor: false },
};
function protectedUnload() {
  const event = new Event('beforeunload', { cancelable: true });
  window.dispatchEvent(event);
  return event.defaultPrevented;
}
function View() {
  const { state, signOut } = useAuth();
  return state.status === 'authenticated' ? (
    <>
      <input aria-label="Private draft" defaultValue="Unsaved work" />
      <DocumentNavigationGuard id={`doc_${'a'.repeat(32)}`} protectedChanges saving={false} />
      <button onClick={() => void signOut()}>Sign out</button>
    </>
  ) : (
    <p>{state.status}</p>
  );
}
it('releases the guard synchronously before sign-out redirects', async () => {
  const signOut = vi.fn(async () => {
    expect(protectedUnload()).toBe(false);
  });
  const provider: AuthProvider = { current: async () => signedIn, signIn: async () => {}, signOut };
  render(
    <AuthContextProvider provider={provider}>
      <View />
    </AuthContextProvider>,
  );
  await screen.findByLabelText('Private draft');
  expect(protectedUnload()).toBe(true);
  fireEvent.click(screen.getByRole('button', { name: 'Sign out' }));
  await screen.findByText('anonymous');
  expect(signOut).toHaveBeenCalledOnce();
  expect(screen.queryByLabelText('Private draft')).not.toBeInTheDocument();
});
it('retains a same-owner renewal but drops a pending Stay prompt on authoritative identity loss', async () => {
  const current = vi.fn<() => Promise<AuthState>>().mockResolvedValue(signedIn);
  const provider: AuthProvider = { current, signIn: async () => {}, signOut: async () => {} };
  render(
    <AuthContextProvider provider={provider}>
      <View />
    </AuthContextProvider>,
  );
  const draft = await screen.findByLabelText('Private draft');
  await act(async () => window.dispatchEvent(new Event('focus')));
  expect(screen.getByLabelText('Private draft')).toBe(draft);
  expect(protectedUnload()).toBe(true);
  const navigate = vi.fn();
  act(() => requestNavigation({ url: '/artifacts', state: null }, navigate));
  expect(screen.getByRole('button', { name: 'Stay' })).toBeVisible();
  current.mockResolvedValue({ status: 'anonymous' });
  await act(async () => window.dispatchEvent(new Event('focus')));
  expect(screen.queryByRole('button', { name: 'Stay' })).not.toBeInTheDocument();
  expect(screen.queryByLabelText('Private draft')).not.toBeInTheDocument();
  expect(protectedUnload()).toBe(false);
  expect(navigate).not.toHaveBeenCalled();
});
