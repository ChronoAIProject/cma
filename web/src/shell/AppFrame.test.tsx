import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fireEvent, render, screen } from '@testing-library/react';
import { act } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { AppFrame } from './AppFrame';
import { DETAILS_MIN, SIDEBAR_COLLAPSED, SIDEBAR_DEFAULT } from './columns';

/**
 * The page frame — the shared style contracts
 *
 * jsdom lays nothing out, so what is checked here is what the frame *decides*:
 * the tracks it writes, what it keeps mounted, and how it behaves when it is
 * measured. The arithmetic behind the numbers is pinned in columns.test.ts.
 */

/** jsdom has no ResizeObserver, and no layout to feed one. */
let observed: (() => void) | undefined;

class FakeResizeObserver {
  constructor(callback: () => void) {
    observed = callback;
  }
  observe() {
    /* the callback is fired by the test, since jsdom never resizes */
  }
  disconnect() {
    observed = undefined;
  }
}

/** Report a frame width, the way the observer would. */
function measure(width: number) {
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue({
    width,
    height: 800,
  } as DOMRect);
  act(() => {
    observed?.();
    // The frame coalesces to one measurement per animation frame.
    vi.advanceTimersByTime(20);
  });
}

function tracks(): string {
  return (document.querySelector('.frame') as HTMLElement).style.gridTemplateColumns;
}

function pointer(type: string, clientX: number): Event {
  const event = new Event(type, { bubbles: true });
  Object.defineProperties(event, {
    clientX: { value: clientX },
    pointerId: { value: 1 },
  });
  return event;
}

function capturePointer(handle: HTMLElement): void {
  let captured = false;
  Object.defineProperties(handle, {
    setPointerCapture: {
      configurable: true,
      value: () => {
        captured = true;
      },
    },
    hasPointerCapture: {
      configurable: true,
      value: () => captured,
    },
    releasePointerCapture: {
      configurable: true,
      value: () => {
        captured = false;
      },
    },
  });
}

function dragAcrossFrames(handle: HTMLElement, positions: readonly number[]): void {
  capturePointer(handle);
  act(() => {
    handle.dispatchEvent(pointer('pointerdown', positions[0] ?? 0));
  });
  for (const position of positions.slice(1)) {
    act(() => {
      handle.dispatchEvent(pointer('pointermove', position));
      vi.advanceTimersByTime(20);
    });
  }
  act(() => {
    handle.dispatchEvent(pointer('pointerup', positions.at(-1) ?? 0));
  });
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.stubGlobal('ResizeObserver', FakeResizeObserver);
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) =>
    setTimeout(() => {
      callback(0);
    }, 0),
  );
  vi.stubGlobal('cancelAnimationFrame', (handle: number) => {
    clearTimeout(handle);
  });
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  observed = undefined;
});

function renderFrame(detailsOpen = true) {
  return render(
    <AppFrame
      detailsOpen={detailsOpen}
      sidebar={<p>navigation</p>}
      center={<p>surface</p>}
      details={<p>details</p>}
      overlay={<p>banner</p>}
    />,
  );
}

describe('the page frame', () => {
  it('measures once on mount before the observer reports a resize', () => {
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue({
      width: 1440,
      height: 800,
    } as DOMRect);

    renderFrame(false);

    expect(tracks()).toBe(`${SIDEBAR_DEFAULT}px ${1440 - SIDEBAR_DEFAULT}px 0px`);
  });

  it('draws its first measured width at once and eases only later changes', () => {
    // Easing out of the zero-width placeholder grew the centre in on every
    // mount. Its controls slid under the pointer, and a press released over
    // another element never became a click.
    const resolved: { tracks: string; measuring: boolean }[] = [];
    let width = 1440;
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (
      this: HTMLElement,
    ) {
      if (this.classList.contains('frame'))
        resolved.push({
          tracks: this.style.gridTemplateColumns,
          measuring: this.hasAttribute('data-measuring'),
        });
      return { width, height: 800 } as DOMRect;
    });

    renderFrame(false);

    const measured = `${SIDEBAR_DEFAULT}px ${1440 - SIDEBAR_DEFAULT}px 0px`;
    expect(resolved).toContainEqual({ tracks: measured, measuring: true });
    const frame = document.querySelector('.frame');
    expect(frame).not.toHaveAttribute('data-measuring');
    expect(tracks()).toBe(measured);

    width = 900;
    act(() => {
      window.dispatchEvent(new Event('resize'));
      vi.advanceTimersByTime(20);
    });
    expect(tracks()).toBe(`${SIDEBAR_COLLAPSED}px ${900 - SIDEBAR_COLLAPSED}px 0px`);
    expect(frame).not.toHaveAttribute('data-measuring');

    const css = readFileSync(resolve(process.cwd(), 'src/shell/AppFrame.css'), 'utf8');
    expect(css).toMatch(/\.frame\[data-measuring\]\s*\{[^}]*transition: none/);
    expect(css).toMatch(/\.frame\[data-measuring\] \.frame__handle\s*\{[^}]*transition: none/);
  });

  it('keeps easing off until a frame mounted without a width is first measured', () => {
    let width = 0;
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(
      () => ({ width, height: 800 }) as DOMRect,
    );
    renderFrame(false);
    const frame = document.querySelector('.frame');
    expect(frame).toHaveAttribute('data-measuring');

    width = 1440;
    act(() => {
      observed?.();
      vi.advanceTimersByTime(20);
    });

    expect(tracks()).toBe(`${SIDEBAR_DEFAULT}px ${1440 - SIDEBAR_DEFAULT}px 0px`);
    expect(frame).not.toHaveAttribute('data-measuring');
  });

  it('writes the solved widths as its own tracks', () => {
    renderFrame();
    measure(1600);
    expect(tracks()).toBe(
      `${SIDEBAR_DEFAULT}px ${1600 - SIDEBAR_DEFAULT - DETAILS_MIN}px ${DETAILS_MIN}px`,
    );
  });

  it('remeasures its box when a window resize arrives before the observer', () => {
    let width = 1440;
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(
      () => ({ width, height: 800 }) as DOMRect,
    );
    renderFrame(false);
    expect(tracks()).toBe(`${SIDEBAR_DEFAULT}px ${1440 - SIDEBAR_DEFAULT}px 0px`);

    width = 900;
    act(() => {
      window.dispatchEvent(new Event('resize'));
      vi.advanceTimersByTime(20);
    });

    expect(tracks()).toBe(`${SIDEBAR_COLLAPSED}px ${900 - SIDEBAR_COLLAPSED}px 0px`);
  });

  it('recovers immediately when a background-loaded frame returns to the foreground', () => {
    let width = 0;
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(
      () => ({ width, height: 800 }) as DOMRect,
    );
    const visibility = vi.spyOn(document, 'visibilityState', 'get');
    visibility.mockReturnValue('hidden');
    renderFrame(false);
    expect(tracks()).toBe(`${SIDEBAR_DEFAULT}px 0px 0px`);

    width = 1440;
    act(() => {
      observed?.();
    });
    // The observer's animation frame is intentionally left throttled, as it
    // can be in a real background tab.
    expect(tracks()).toBe(`${SIDEBAR_DEFAULT}px 0px 0px`);

    visibility.mockReturnValue('visible');
    act(() => {
      document.dispatchEvent(new Event('visibilitychange'));
    });

    expect(tracks()).toBe(`${SIDEBAR_DEFAULT}px ${1440 - SIDEBAR_DEFAULT}px 0px`);
    act(() => {
      vi.advanceTimersByTime(20);
    });

    width = 900;
    act(() => {
      window.dispatchEvent(new Event('focus'));
    });
    expect(tracks()).toBe(`${SIDEBAR_COLLAPSED}px ${900 - SIDEBAR_COLLAPSED}px 0px`);
  });

  it('keeps the details column mounted when it is closed', () => {
    // §9: closed is zero width, never unmounted. Unmounting throws away
    // whatever the column had scrolled to and rebuilt on every open.
    renderFrame(false);
    measure(1600);
    expect(screen.getByText('details')).toBeInTheDocument();
    expect(tracks().endsWith('0px')).toBe(true);
  });

  it('drops the seam a zero-width column would otherwise paint', () => {
    // A 1px left border on a 0px column is a 1px line down the page.
    renderFrame(false);
    measure(1600);
    expect(document.querySelector('.frame')).toHaveAttribute('data-details-collapsed');
  });

  it('collapses the sidebar to its rail on a narrow frame', () => {
    renderFrame();
    measure(900);
    expect(tracks().startsWith(`${SIDEBAR_COLLAPSED}px`)).toBe(true);
  });

  it('lets somebody open it anyway, and the auto-collapse does not fight back', () => {
    /*
     * Without the override the toggle does nothing below the breakpoint: the
     * rule would win on the next render and shut the panel again, which reads
     * as a broken control rather than as a deliberate layout.
     */
    renderFrame();
    measure(900);

    const toggle = screen.getByRole('button', { name: /show navigation/i });
    act(() => {
      toggle.click();
    });

    expect(tracks().startsWith(`${SIDEBAR_DEFAULT}px`)).toBe(true);
    expect(screen.getByRole('button', { name: /hide navigation/i })).toHaveAttribute(
      'aria-expanded',
      'true',
    );
  });

  it.each([390, 800, 1440])(
    'locks landing navigation at %dpx even after a prior explicit expansion',
    (width) => {
      const onChange = vi.fn();
      const draw = (locked: boolean) => (
        <AppFrame
          detailsOpen={false}
          sidebarLocked={locked}
          onSidebarCollapsedChange={onChange}
          sidebar={({ onToggle }) => <button onClick={onToggle}>External toggle</button>}
          center={<p>landing</p>}
        />
      );
      const view = render(draw(false));
      measure(width);
      act(() =>
        screen
          .getByRole('button', { name: width < 640 ? 'Show navigation' : 'External toggle' })
          .click(),
      );
      view.rerender(draw(true));
      onChange.mockClear();
      if (width >= 640) act(() => screen.getByRole('button', { name: 'External toggle' }).click());
      expect(tracks().startsWith(`${width < 640 ? 0 : SIDEBAR_COLLAPSED}px`)).toBe(true);
      expect(screen.queryByRole('button', { name: /show navigation/i })).toBeNull();
      expect(screen.queryByRole('separator', { name: 'Resize navigation' })).toBeNull();
      expect(document.querySelector('.frame')).not.toHaveAttribute('data-sidebar-overlay');
      expect(onChange).not.toHaveBeenCalled();
    },
  );

  it('replaces the rail on a phone with a top bar and opens the sidebar as a sheet', () => {
    render(
      <AppFrame
        detailsOpen={false}
        phoneTitle="Agent Manager"
        sidebar={({ collapsed, onToggle }) => (
          <>
            <p>{collapsed ? 'sidebar rail' : 'sidebar panel'}</p>
            <button type="button" onClick={onToggle}>
              {collapsed ? 'Show navigation' : 'Hide navigation'}
            </button>
          </>
        )}
        center={<p>surface</p>}
      />,
    );
    measure(390);

    // No rail: the collapsed sidebar takes no width and leaves the tab order.
    expect(tracks().startsWith('0px')).toBe(true);
    expect(document.querySelector('.frame__sidebar')).toHaveAttribute('hidden');
    expect(screen.getByText('Agent Manager')).toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: 'Show navigation' })).toHaveLength(1);
    act(() => {
      screen.getByRole('button', { name: 'Show navigation' }).click();
    });

    expect(screen.getByText('sidebar panel')).toBeInTheDocument();
    expect(document.querySelector('.frame')).toHaveAttribute('data-sidebar-overlay');
    expect(document.querySelector('.frame__center')).toHaveAttribute('inert');
    expect(screen.getAllByRole('button', { name: 'Hide navigation' })).toHaveLength(1);

    // The scrim closes it, as does Escape.
    act(() => {
      (document.querySelector('.frame__scrim') as HTMLElement).click();
    });
    expect(document.querySelector('.frame')).not.toHaveAttribute('data-sidebar-overlay');
    expect(document.querySelector('.frame__center')).not.toHaveAttribute('inert');
    act(() => {
      screen.getByRole('button', { name: 'Show navigation' }).click();
    });
    act(() => {
      document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
    });
    expect(document.querySelector('.frame')).not.toHaveAttribute('data-sidebar-overlay');
    expect(document.querySelector('.frame__scrim')).toBeNull();
  });

  it('keeps the rail and no top bar between the phone and the auto-collapse widths', () => {
    render(
      <AppFrame
        detailsOpen={false}
        sidebar={({ collapsed }) => <p>{collapsed ? 'sidebar rail' : 'sidebar panel'}</p>}
        center={<p>surface</p>}
      />,
    );
    measure(800);
    expect(tracks().startsWith(`${SIDEBAR_COLLAPSED}px`)).toBe(true);
    expect(document.querySelector('.frame__topbar')).toBeNull();
    expect(document.querySelector('.frame__sidebar')).not.toHaveAttribute('hidden');
  });

  it('offers no way back when the sidebar has not collapsed', () => {
    // The toggle exists to undo the automatic collapse. At a width where
    // nothing collapsed, it would be a second control for the same thing.
    renderFrame();
    measure(1600);
    expect(screen.queryByRole('button', { name: /navigation/i })).toBeNull();
  });

  it('clips rather than scrolls, so bringing content into view never shifts the page', () => {
    // A scrollable frame moved 16px sideways when a header was scrolled into
    // view, because the closed details column still counted toward its width.
    const css = readFileSync(resolve(process.cwd(), 'src/shell/AppFrame.css'), 'utf8');
    expect(css).toMatch(/\.frame\s*\{[^}]*overflow: clip/);
    expect(css).not.toMatch(/\.frame\s*\{[^}]*overflow: hidden/);
  });

  it('gives each resizable border one named handle', () => {
    // Named, because a bare strip is invisible to anybody not using a pointer.
    renderFrame();
    measure(1600);
    const handles = screen.getAllByRole('separator');
    expect(handles.map((handle) => handle.getAttribute('aria-label'))).toEqual([
      'Resize navigation',
      'Resize the details column',
    ]);
  });

  it('draws a grip on the details edge only', () => {
    const css = readFileSync(resolve(process.cwd(), 'src/shell/AppFrame.css'), 'utf8');
    const grips = [...css.matchAll(/\.frame__handle([^,{]*)::after/g)].map(
      (match) => match[1] ?? '',
    );
    expect(grips.length).toBeGreaterThan(0);
    expect(grips.every((selector) => selector.includes("[data-side='details']"))).toBe(true);
    expect(css).toMatch(
      /\.frame__handle\[data-side='details'\]::after\s*\{[^}]*width: 12px[^}]*height: 32px[^}]*border-radius: 10px/,
    );
  });

  it('anchors a multi-frame drag to the width at pointer-down', () => {
    renderFrame();
    measure(1600);

    const sidebar = screen.getByRole('separator', { name: 'Resize navigation' });
    dragAcrossFrames(sidebar, [SIDEBAR_DEFAULT, 300, 320, 340]);
    expect(tracks().startsWith('340px ')).toBe(true);

    const details = screen.getByRole('separator', { name: 'Resize the details column' });
    dragAcrossFrames(details, [1300, 1280, 1260, 1240]);
    expect(tracks().endsWith(' 360px')).toBe(true);
  });

  it('resizes Files by keyboard with an announced bounded width and preserves the center node', () => {
    const view = renderFrame();
    measure(1600);
    const center = screen.getByText('surface');
    const handle = screen.getByRole('separator', { name: 'Resize the details column' });
    expect(handle).toHaveAttribute('tabindex', '0');
    expect(handle).toHaveAttribute('aria-valuenow', '300');
    fireEvent.keyDown(handle, { key: 'ArrowLeft' });
    expect(handle).toHaveAttribute('aria-valuenow', '316');
    fireEvent.keyDown(handle, { key: 'End' });
    expect(handle).toHaveAttribute('aria-valuenow', '520');
    fireEvent.keyDown(handle, { key: 'Home' });
    expect(handle).toHaveAttribute('aria-valuenow', '300');
    view.rerender(
      <AppFrame
        detailsOpen={false}
        sidebar={<p>navigation</p>}
        center={<p>surface</p>}
        details={<p>details</p>}
      />,
    );
    expect(screen.getByText('surface')).toBe(center);
  });

  it('does not treat an unmeasured frame as compact or move focus into its details', () => {
    let width = 0;
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(
      () => ({ width, height: 800 }) as DOMRect,
    );
    render(
      <AppFrame
        detailsOpen
        compactDetailsOverlay
        onDetailsClose={() => {}}
        sidebar={<p>navigation</p>}
        center={<p>surface</p>}
        details={<button type="button">panel action</button>}
      />,
    );
    const frame = document.querySelector('.frame');
    const action = screen.getByRole('button', { name: 'panel action' });
    expect(frame).not.toHaveAttribute('data-details-overlay');
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(action).not.toHaveFocus();

    width = 900;
    act(() => {
      observed?.();
      vi.advanceTimersByTime(20);
    });
    expect(frame).toHaveAttribute('data-details-overlay');
    expect(action).toHaveFocus();
  });

  it('offers no details handle when there is no details column', () => {
    renderFrame(false);
    measure(1600);
    expect(screen.getAllByRole('separator')).toHaveLength(1);
  });

  it('draws the overlay above every column, and lets clicks through where it is empty', () => {
    renderFrame();
    measure(1600);
    expect(screen.getByText('banner')).toBeInTheDocument();
    expect(document.querySelector('.frame__overlay')?.parentElement).toHaveClass('frame');
  });
});
