import { StrictMode, useEffect } from 'react';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { useViewArrival } from './arrivalMotion';
import { ServiceBody } from '../services/ServiceBody';
import { observeAnimations } from '../test/animations';

let motion: ReturnType<typeof observeAnimations>;
beforeEach(() => {
  motion = observeAnimations();
});
afterEach(() => motion.restore());

function View({ identity }: { identity: string }) {
  const ref = useViewArrival(identity, true, false);
  return (
    <main
      ref={ref}
      style={
        {
          '--ds-transition-duration-slow': '0.3s',
          '--ds-ease-in-out': 'cubic-bezier(0.4, 0, 0.2, 1)',
        } as React.CSSProperties
      }
    >
      <input aria-label="Draft" />
    </main>
  );
}

it('fades changed views without replacing the draft, focus or scroll position', () => {
  const draw = (identity: string) => (
    <StrictMode>
      <View identity={identity} />
    </StrictMode>
  );
  const { rerender } = render(draw('/agents/one'));
  const field = screen.getByRole('textbox', { name: 'Draft' });
  const surface = screen.getByRole('main');
  fireEvent.change(field, { target: { value: 'Keep this draft' } });
  field.focus();
  surface.scrollTop = 140;
  expect(motion.records).toHaveLength(0);
  rerender(draw('/agents/two'));
  expect(motion.records).toHaveLength(1);
  expect(motion.records[0]?.element).toBe(surface);
  expect(motion.records[0]?.frames).toEqual([{ opacity: 0 }, { opacity: 1 }]);
  expect(motion.records[0]?.options).toEqual({
    duration: 300,
    easing: 'cubic-bezier(0.4, 0, 0.2, 1)',
  });
  expect(screen.getByRole('textbox')).toBe(field);
  expect(field).toHaveValue('Keep this draft');
  expect(field).toHaveFocus();
  expect(surface.scrollTop).toBe(140);
  rerender(draw('/agents/two'));
  expect(motion.records).toHaveLength(1);
  expect(motion.records[0]?.animation.cancel).not.toHaveBeenCalled();
});

it('fades a held portal on re-entry and cancels an interrupted fade without remounting work', () => {
  const mounted = vi.fn();
  const unmounted = vi.fn();
  function Work() {
    useEffect(() => {
      mounted();
      return unmounted;
    }, []);
    return <input aria-label="Image name" />;
  }
  const draw = (visible: boolean) => (
    <ServiceBody visible={visible}>
      <Work />
    </ServiceBody>
  );
  const { rerender, unmount } = render(draw(true));
  const field = screen.getByRole('textbox');
  fireEvent.change(field, { target: { value: 'Draft image' } });
  expect(motion.records).toHaveLength(1);
  rerender(draw(false));
  expect(field).not.toBeVisible();
  expect(motion.records[0]?.animation.cancel).toHaveBeenCalledOnce();
  rerender(draw(true));
  expect(motion.records).toHaveLength(2);
  expect(motion.records[1]?.element).toBe(motion.records[0]?.element);
  expect(screen.getByRole('textbox')).toBe(field);
  expect(field).toHaveValue('Draft image');
  expect(mounted).toHaveBeenCalledOnce();
  expect(unmounted).not.toHaveBeenCalled();
  unmount();
  expect(motion.records[1]?.animation.cancel).toHaveBeenCalledOnce();
});

it('cancels rapid route changes and live reduced-motion requests without replaying on preference reset', () => {
  const { rerender } = render(<View identity="one" />);
  rerender(<View identity="two" />);
  rerender(<View identity="three" />);
  expect(motion.records).toHaveLength(2);
  expect(motion.records[0]?.animation.cancel).toHaveBeenCalledOnce();
  act(() => motion.reduce(true));
  expect(motion.records[1]?.animation.cancel).toHaveBeenCalledOnce();
  rerender(<View identity="four" />);
  expect(motion.records).toHaveLength(2);
  act(() => motion.reduce(false));
  expect(motion.records).toHaveLength(2);
  rerender(<View identity="five" />);
  expect(motion.records).toHaveLength(3);
});

it('never animates a route change inside a hidden retained portal', () => {
  const draw = (identity: string) => (
    <div hidden>
      <View identity={identity} />
    </div>
  );
  const { rerender } = render(draw('one'));
  rerender(draw('two'));
  expect(motion.records).toHaveLength(0);
});
