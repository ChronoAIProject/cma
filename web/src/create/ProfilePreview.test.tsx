import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { creationClient, CREATION_AGENT } from '../test/creation';
import { ProfilePreview } from './ProfilePreview';
import type { CreationProfile } from './client';

afterEach(() => vi.useRealTimers());

const choice = {
  id: CREATION_AGENT.revision.profileId,
  revision: CREATION_AGENT.revision.revision,
  name: CREATION_AGENT.name,
  source: 'own' as const,
};

it.each([0, 250])(
  'dismisses a pending or open preview when selecting the profile after %d ms',
  async (delay) => {
    vi.useFakeTimers();
    const select = vi.fn();
    render(
      <ul>
        <ProfilePreview client={creationClient()} kind="agent" choice={choice}>
          <button onClick={select}>Select profile</button>
        </ProfilePreview>
      </ul>,
    );
    const button = screen.getByRole('button');
    fireEvent.focus(button);
    await act(() => vi.advanceTimersByTimeAsync(delay));
    fireEvent.click(button);
    await act(() => vi.advanceTimersByTimeAsync(300));
    expect(select).toHaveBeenCalledOnce();
    expect(screen.queryByRole('region')).not.toBeInTheDocument();
  },
);

it.each([
  { headerBottom: 90, footerTop: 800, height: 440 },
  { headerBottom: 120, footerTop: 360, height: 240 },
])('keeps the preview between dialog controls %j', async ({ headerBottom, footerTop, height }) => {
  vi.useFakeTimers();
  const geometry = vi
    .spyOn(HTMLElement.prototype, 'getBoundingClientRect')
    .mockImplementation(function (this: HTMLElement) {
      if (this.classList.contains('modal__header'))
        return new DOMRect(24, 24, 342, headerBottom - 24);
      if (this.classList.contains('modal__footer')) return new DOMRect(24, footerTop, 342, 40);
      return new DOMRect(49, footerTop - 150, 292, 44);
    });
  const measured = vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockReturnValue(height);
  try {
    render(
      <div role="dialog" aria-label="New workspace">
        <div className="modal__header">New workspace</div>
        <ul>
          <ProfilePreview client={creationClient()} kind="agent" choice={choice}>
            <button>Select profile</button>
          </ProfilePreview>
        </ul>
        <div className="modal__footer">
          <button>Create workspace</button>
        </div>
      </div>,
    );
    fireEvent.mouseEnter(screen.getByRole('listitem'));
    await act(() => vi.advanceTimersByTimeAsync(200));
    const preview = screen.getByRole('region');
    expect(Number.parseFloat(preview.style.top)).toBeGreaterThanOrEqual(headerBottom);
    expect(Number.parseFloat(preview.style.top) + height).toBeLessThanOrEqual(footerTop);
    expect(preview.style.maxHeight).toBe(
      `${Math.min(footerTop, innerHeight - 12) - headerBottom}px`,
    );
  } finally {
    geometry.mockRestore();
    measured.mockRestore();
  }
});

it('opens pinned details at a stable position, keeps them while hovered and closes on Escape', async () => {
  vi.useFakeTimers();
  const readProfile = vi.fn(creationClient().readProfile);
  render(
    <ul>
      <ProfilePreview client={creationClient({ readProfile })} kind="agent" choice={choice}>
        <a href="/profiles/agent-profiles/preview">Research agent</a>
      </ProfilePreview>
    </ul>,
  );
  const row = screen.getByRole('listitem');
  fireEvent.mouseEnter(row);
  expect(readProfile).not.toHaveBeenCalled();
  await act(() => vi.advanceTimersByTimeAsync(200));
  const preview = screen.getByRole('region', { name: `${choice.name} profile details` });
  expect(within(preview).getByText('Model')).toBeInTheDocument();
  expect(readProfile).toHaveBeenCalledWith(
    'agent',
    choice,
    expect.objectContaining({ signal: expect.any(AbortSignal) }),
  );
  const position = preview.getAttribute('style');
  fireEvent.mouseMove(row, { clientX: 800, clientY: 700 });
  expect(preview.getAttribute('style')).toBe(position);
  fireEvent.mouseLeave(row);
  fireEvent.mouseEnter(preview);
  await act(() => vi.advanceTimersByTimeAsync(200));
  expect(preview).toBeInTheDocument();
  within(preview).getByLabelText('Profile settings').focus();
  fireEvent.keyDown(document, { key: 'Escape' });
  expect(screen.queryByRole('region')).not.toBeInTheDocument();
  expect(screen.getByRole('link', { name: 'Research agent' })).toHaveFocus();
  await act(() => vi.advanceTimersByTimeAsync(250));
  expect(screen.queryByRole('region')).not.toBeInTheDocument();
});

it('aborts a closed preview and never shows a late or mismatched profile', async () => {
  vi.useFakeTimers();
  let complete: ((value: CreationProfile) => void) | undefined;
  let signal: AbortSignal | undefined;
  const client = creationClient({
    readProfile: (_kind, _choice, options) => {
      signal = options?.signal;
      return new Promise((resolve) => {
        complete = resolve;
      });
    },
  });
  render(
    <ul>
      <ProfilePreview client={client} kind="agent" choice={choice}>
        <button>Profile</button>
      </ProfilePreview>
    </ul>,
  );
  fireEvent.focus(screen.getByRole('button'));
  await act(() => vi.advanceTimersByTimeAsync(200));
  expect(screen.getByRole('status')).toHaveTextContent('Loading profile details');
  fireEvent.mouseLeave(screen.getByRole('listitem'));
  await act(() => vi.advanceTimersByTimeAsync(150));
  expect(signal?.aborted).toBe(true);
  await act(async () => complete?.(CREATION_AGENT));
  expect(screen.queryByRole('region')).not.toBeInTheDocument();
  fireEvent.mouseEnter(screen.getByRole('listitem'));
  await act(() => vi.advanceTimersByTimeAsync(200));
  await act(async () =>
    complete?.({
      ...CREATION_AGENT,
      revision: { ...CREATION_AGENT.revision, profileId: 'another-profile' },
    } as CreationProfile),
  );
  expect(screen.getByRole('status')).toHaveTextContent('Profile details are unavailable');
  expect(screen.queryByText('Model')).not.toBeInTheDocument();
});
