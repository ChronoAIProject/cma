import { vi } from 'vitest';

class ObservedAnimation extends EventTarget {
  playState: AnimationPlayState = 'running';
  readonly cancel = vi.fn(() => {
    this.playState = 'idle';
    this.dispatchEvent(new Event('cancel'));
  });
  finish() {
    this.playState = 'finished';
    this.dispatchEvent(new Event('finish'));
  }
}

/** Observe browser animation ownership without replacing DOM, focus or React lifetimes. */
export function observeAnimations() {
  const original = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'animate');
  const media = new EventTarget();
  let reduced = false;
  const nativeMatch = window.matchMedia.bind(window);
  const preference = {
    get matches() {
      return reduced;
    },
    addEventListener: media.addEventListener.bind(media),
    removeEventListener: media.removeEventListener.bind(media),
  } as MediaQueryList;
  const match = vi
    .spyOn(window, 'matchMedia')
    .mockImplementation((query) =>
      query === '(prefers-reduced-motion: reduce)' ? preference : nativeMatch(query),
    );
  const records: {
    element: HTMLElement;
    frames: Keyframe[] | PropertyIndexedKeyframes | null;
    options: number | KeyframeAnimationOptions | undefined;
    animation: ObservedAnimation;
  }[] = [];
  Object.defineProperty(HTMLElement.prototype, 'animate', {
    configurable: true,
    value(
      this: HTMLElement,
      frames: Keyframe[] | PropertyIndexedKeyframes | null,
      options?: number | KeyframeAnimationOptions,
    ) {
      const animation = new ObservedAnimation();
      records.push({ element: this, frames, options, animation });
      return animation;
    },
  });
  return {
    records,
    reduce(value: boolean) {
      reduced = value;
      media.dispatchEvent(new Event('change'));
    },
    restore() {
      match.mockRestore();
      if (original) Object.defineProperty(HTMLElement.prototype, 'animate', original);
      else delete (HTMLElement.prototype as Partial<HTMLElement>).animate;
    },
  };
}
