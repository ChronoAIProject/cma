import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { useAppRouter, routePath } from '../routing/routes';
import { DocumentNavigationGuard } from '../documents/DocumentNavigationGuard';
import { DocsLink } from './DocsLink';

it('uses the guarded application route and preserves the editor when the user stays', async () => {
  const id = `doc_${'a'.repeat(32)}`;
  const path = `/artifacts/${id}`;
  history.replaceState(null, '', path);
  function Editor() {
    const router = useAppRouter();
    return (
      <>
        <output>{routePath(router.route)}</output>
        <input aria-label="Draft" defaultValue="Keep my edits" />
        <DocumentNavigationGuard id={id} protectedChanges saving={false} />
        <DocsLink view="find-your-outputs">Help finding your outputs</DocsLink>
      </>
    );
  }
  render(<Editor />);
  const user = userEvent.setup();
  const link = screen.getByRole('link', { name: 'Help finding your outputs' });
  expect(link).toHaveAttribute('href', '/docs/find-your-outputs');
  link.focus();
  await user.keyboard('{Enter}');
  expect(screen.getByRole('dialog', { name: 'Leave without saving?' })).toBeVisible();
  expect(location.pathname).toBe(path);
  await user.click(screen.getByRole('button', { name: 'Stay' }));
  expect(screen.getByLabelText('Draft')).toHaveValue('Keep my edits');
  await user.click(link);
  await user.click(screen.getByRole('button', { name: 'Leave without saving' }));
  expect(location.pathname).toBe('/docs/find-your-outputs');
  expect(screen.getByRole('status')).toHaveTextContent('/docs/find-your-outputs');
});

it.each(['ctrlKey', 'metaKey', 'shiftKey', 'altKey'])(
  'leaves %s clicks to the browser without changing the current route',
  (modifier) => {
    const navigate = vi.fn();
    render(
      <DocsLink view="start-a-task" onNavigate={navigate}>
        Start guide
      </DocsLink>,
    );
    const link = screen.getByRole('link', { name: 'Start guide' });
    let intercepted: boolean | undefined;
    const observe = (event: MouseEvent) => {
      intercepted = event.defaultPrevented;
      event.preventDefault();
    };
    document.addEventListener('click', observe, { once: true });
    fireEvent.click(link, { [modifier]: true });
    expect(intercepted).toBe(false);
    expect(navigate).not.toHaveBeenCalled();
  },
);
