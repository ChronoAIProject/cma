import { act, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { Presence } from './Presence';
import { Modal } from './Modal';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('notification presence', () => {
  it('skips exit retention when reduced motion is enabled, including a live preference change', () => {
    vi.useFakeTimers();
    const media = new EventTarget();
    let reduced = false;
    vi.spyOn(window, 'matchMedia').mockImplementation(
      () =>
        ({
          get matches() {
            return reduced;
          },
          addEventListener: media.addEventListener.bind(media),
          removeEventListener: media.removeEventListener.bind(media),
        }) as MediaQueryList,
    );
    const message = <p role="status">Saved</p>;
    const { rerender } = render(<Presence>{message}</Presence>);
    rerender(<Presence>{null}</Presence>);
    expect(screen.getByText('Saved')).toBeInTheDocument();
    act(() => {
      reduced = true;
      media.dispatchEvent(new Event('change'));
    });
    expect(screen.queryByText('Saved')).toBeNull();
    rerender(<Presence>{message}</Presence>);
    rerender(<Presence>{null}</Presence>);
    expect(screen.queryByText('Saved')).toBeNull();
    act(() => {
      reduced = false;
      media.dispatchEvent(new Event('change'));
    });
    expect(screen.queryByText('Saved')).toBeNull();
  });

  it('fades a conditionally mounted dialog, releasing and restoring focus on quick reopen', () => {
    vi.useFakeTimers();
    const opener = document.createElement('button');
    document.body.append(opener);
    opener.focus();
    const closed = vi.fn();
    function Details() {
      return (
        <Modal open title="Details" onClose={closed}>
          <input aria-label="Draft" />
        </Modal>
      );
    }
    const { rerender } = render(
      <Presence>
        <Details />
      </Presence>,
    );
    const modal = document.querySelector('.modal');
    rerender(<Presence>{null}</Presence>);
    expect(opener).toHaveFocus();
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(modal).toHaveAttribute('data-presence', 'exiting');
    expect(document.body.style.overflow).not.toBe('hidden');
    act(() => vi.advanceTimersByTime(100));
    rerender(
      <Presence>
        <Details />
      </Presence>,
    );
    expect(screen.getAllByRole('dialog')).toHaveLength(1);
    expect(screen.getByRole('button', { name: 'Close' })).toHaveFocus();
    expect(document.body.style.overflow).toBe('hidden');
    act(() => vi.advanceTimersByTime(200));
    expect(modal).toBeInTheDocument();
    expect(modal).not.toHaveAttribute('inert');
    rerender(<Presence>{null}</Presence>);
    act(() => vi.advanceTimersByTime(200));
    expect(modal).not.toBeInTheDocument();
    expect(opener).toHaveFocus();
    opener.remove();
  });

  it('retains the last message as an inert exit frame and then removes it', () => {
    vi.useFakeTimers();
    const { rerender } = render(
      <Presence>
        <p role="alert">A connection failed</p>
      </Presence>,
    );
    const alert = screen.getByRole('alert');
    expect(alert).toHaveAttribute('data-presence', 'visible');
    rerender(<Presence>{null}</Presence>);
    expect(screen.queryByRole('alert')).toBeNull();
    expect(alert).toHaveAttribute('inert');
    expect(alert).toHaveAttribute('aria-hidden', 'true');
    expect(alert).toHaveAttribute('data-presence', 'exiting');
    act(() => vi.advanceTimersByTime(199));
    expect(alert).toBeInTheDocument();
    act(() => vi.advanceTimersByTime(1));
    expect(alert).not.toBeInTheDocument();
  });

  it('a new message cancels old dismissal and retains its own complete exit time', () => {
    vi.useFakeTimers();
    const { rerender } = render(
      <Presence>
        <p role="status">First</p>
      </Presence>,
    );
    rerender(<Presence>{null}</Presence>);
    act(() => vi.advanceTimersByTime(150));
    rerender(
      <Presence>
        <p role="status">Second</p>
      </Presence>,
    );
    act(() => vi.advanceTimersByTime(100));
    expect(screen.getByRole('status')).toHaveTextContent('Second');
    expect(screen.getByRole('status')).not.toHaveAttribute('inert');
    rerender(<Presence>{null}</Presence>);
    act(() => vi.advanceTimersByTime(199));
    expect(screen.getByText('Second')).toBeInTheDocument();
    act(() => vi.advanceTimersByTime(1));
    expect(screen.queryByText('Second')).toBeNull();
  });

  it('releases focus and scroll locking while the dismissed dialog fades', () => {
    vi.useFakeTimers();
    const opener = document.createElement('button');
    document.body.append(opener);
    opener.focus();
    const closed = vi.fn();
    const { rerender } = render(
      <Modal open title="Details" onClose={closed}>
        <button>Inside</button>
      </Modal>,
    );
    expect(screen.getByRole('button', { name: 'Close' })).toHaveFocus();
    rerender(
      <Modal open={false} title="Details" onClose={closed}>
        <button>Inside</button>
      </Modal>,
    );
    expect(opener).toHaveFocus();
    expect(document.body.style.overflow).not.toBe('hidden');
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(document.querySelector('.modal')).toHaveAttribute('data-presence', 'exiting');
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
    expect(closed).not.toHaveBeenCalled();
    act(() => vi.advanceTimersByTime(200));
    expect(document.querySelector('.modal')).toBeNull();
    opener.remove();
  });

  it('unmounting clears pending exit without a late focus change', () => {
    vi.useFakeTimers();
    const { rerender, unmount } = render(
      <Presence>
        <p role="status">Saved</p>
      </Presence>,
    );
    rerender(<Presence>{null}</Presence>);
    expect(vi.getTimerCount()).toBe(1);
    unmount();
    expect(vi.getTimerCount()).toBe(0);
  });
});
