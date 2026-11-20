import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { useLayoutEffect } from 'react';
import { DocumentNavigationGuard } from '../documents/DocumentNavigationGuard';
import { navigateAppPath } from './navigationGuard';
import { useAppRouter, type AppRouter } from './routes';

const id = `doc_${'a'.repeat(32)}`;
const path = `/artifacts/${id}`;
let router: AppRouter;
function Harness({ dirty = true }: { dirty?: boolean }) {
  const current = useAppRouter();
  useLayoutEffect(() => {
    router = current;
  }, [current]);
  const displayed = current.backgroundRoute ?? current.route;
  return (
    <>
      <output>{current.route.kind}</output>
      {displayed.kind === 'document' ? (
        <>
          <input aria-label="Draft" defaultValue="Keep my selection" />
          <DocumentNavigationGuard id={id} protectedChanges={dirty} saving={false} />
        </>
      ) : null}
    </>
  );
}
function setup(state: unknown = null) {
  history.replaceState(state, '', '/artifacts');
  const view = render(<Harness />);
  act(() => router.navigate({ kind: 'document', documentId: id }));
  return view;
}
afterEach(() => vi.unstubAllGlobals());

it.each([false, true])(
  'retains URL, state, editor and selection on Stay (replace=%s)',
  (replace) => {
    setup();
    const input = screen.getByLabelText('Draft') as HTMLInputElement;
    input.focus();
    input.setSelectionRange(2, 7);
    const saved = history.state;
    const length = history.length;
    act(() => router.navigate({ kind: 'artifacts' }, { replace }));
    expect(screen.getByRole('dialog', { name: 'Leave without saving?' })).toBeVisible();
    expect(location.pathname).toBe(path);
    fireEvent.click(screen.getByRole('button', { name: 'Stay' }));
    expect(screen.getByLabelText('Draft')).toBe(input);
    expect([input.selectionStart, input.selectionEnd]).toEqual([2, 7]);
    expect(history.state).toEqual(saved);
    expect(history.length).toBe(length);
  },
);

it('keeps one pending target and commits exactly one confirmed navigation', () => {
  setup();
  const push = vi.spyOn(history, 'pushState');
  act(() => {
    router.navigate({ kind: 'artifacts' });
    router.navigate({ kind: 'docs' });
    navigateAppPath(`/artifacts/doc_${'b'.repeat(32)}`);
  });
  expect(screen.getAllByRole('dialog')).toHaveLength(1);
  expect(push).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Leave without saving' }));
  expect(location.pathname).toBe('/artifacts');
  expect(push).toHaveBeenCalledOnce();
  push.mockRestore();
});

it('preserves Usage background state without prompting while the editor remains mounted', async () => {
  setup();
  const input = screen.getByLabelText('Draft');
  act(() => router.navigate({ kind: 'usage' }));
  expect(history.state.usageBackground).toBe(path);
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(screen.getByLabelText('Draft')).toBe(input);
  act(() => router.closeUsage?.());
  await waitFor(() => expect(location.pathname).toBe(path));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  act(() => history.forward());
  await waitFor(() => expect(location.pathname).toBe('/usage'));
  const usage = history.state;
  act(() => router.navigate({ kind: 'artifacts' }));
  fireEvent.click(screen.getByRole('button', { name: 'Stay' }));
  expect(history.state).toEqual(usage);
  expect(screen.getByLabelText('Draft')).toBe(input);
});

it('restores then replays indexed Back and Forward without duplicate entries', async () => {
  const view = setup();
  const input = screen.getByLabelText('Draft');
  const source = history.state;
  const length = history.length;
  act(() => history.back());
  await screen.findByRole('dialog');
  expect(location.pathname).toBe(path);
  expect(history.state).toEqual(source);
  fireEvent.click(screen.getByRole('button', { name: 'Stay' }));
  expect(screen.getByLabelText('Draft')).toBe(input);
  act(() => history.back());
  await waitFor(() => expect(screen.getByRole('dialog')).toBeVisible());
  fireEvent.click(screen.getByRole('button', { name: 'Leave without saving' }));
  await waitFor(() => expect(router.route.kind).toBe('artifacts'));
  act(() => history.forward());
  await waitFor(() => expect(router.route.kind).toBe('document'));
  view.rerender(<Harness dirty={false} />);
  act(() => router.navigate({ kind: 'docs' }));
  act(() => history.back());
  await waitFor(() => expect(router.route.kind).toBe('document'));
  view.rerender(<Harness />);
  act(() => history.forward());
  await screen.findByRole('dialog');
  expect(location.pathname).toBe(path);
  fireEvent.click(screen.getByRole('button', { name: 'Leave without saving' }));
  await waitFor(() => expect(router.route.kind).toBe('docs'));
  expect(history.length).toBe(length + 1);
});

it('restores a multi-entry traversal before asking and cancels another Back while asking', async () => {
  history.replaceState(null, '', '/docs');
  render(<Harness />);
  act(() => router.navigate({ kind: 'artifacts' }));
  act(() => router.navigate({ kind: 'document', documentId: id }));
  const state = history.state;
  act(() => history.go(-2));
  await screen.findByRole('dialog');
  expect(history.state).toEqual(state);
  act(() => history.back());
  await waitFor(() => expect(location.pathname).toBe(path));
  fireEvent.click(screen.getByRole('button', { name: 'Leave without saving' }));
  await waitFor(() => expect(router.route.kind).toBe('docs'));
});

it('uses the browser entry index to restore an older unindexed entry without replacing it', async () => {
  vi.stubGlobal('navigation', {
    get currentEntry() {
      return { index: history.state?.nativeIndex ?? history.state?.cmaNavigation?.index ?? 0 };
    },
  });
  history.replaceState({ nativeIndex: 0, legacy: 'preserved' }, '', '/docs');
  history.pushState(null, '', '/artifacts');
  render(<Harness />);
  act(() => router.navigate({ kind: 'document', documentId: id }));
  // The unindexed legacy entry is one browser slot before the indexed app entry.
  history.replaceState({ ...history.state, nativeIndex: 2 }, '');
  act(() => window.dispatchEvent(new Event('cma:protect-navigation')));
  act(() => history.go(-2));
  await screen.findByRole('dialog');
  expect(location.pathname).toBe(path);
  fireEvent.click(screen.getByRole('button', { name: 'Leave without saving' }));
  await waitFor(() => expect(router.route.kind).toBe('docs'));
  expect(history.state.legacy).toBe('preserved');
});

it('drops a stale pending confirmation and unload protection when clean or unmounted', () => {
  const view = setup();
  act(() => router.navigate({ kind: 'docs' }));
  view.rerender(<Harness dirty={false} />);
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  const event = new Event('beforeunload', { cancelable: true });
  window.dispatchEvent(event);
  expect(event.defaultPrevented).toBe(false);
  view.unmount();
  history.replaceState(null, '', path);
  render(<Harness dirty={false} />);
  act(() => router.navigate({ kind: 'docs' }));
  expect(location.pathname).toBe('/docs');
});
