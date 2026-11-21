import type RFB from '@novnc/novnc';

/** Release only this viewer's held inputs before changing to view-only. */
export function desktopInput(rfb: RFB, target: HTMLElement) {
  const heldKeys = new Map<string, number>();
  const original = rfb.sendKey;
  let pointer: { x: number; y: number; buttons: number } | undefined;
  rfb.sendKey = (keysym, code, down) => {
    if (!rfb.viewOnly && code) {
      if (down === true) heldKeys.set(code, keysym);
      if (down === false) heldKeys.delete(code);
    }
    original.call(rfb, keysym, code, down);
  };
  const track = (event: MouseEvent) => {
    if (!rfb.viewOnly) pointer = { x: event.clientX, y: event.clientY, buttons: event.buttons };
  };
  const events = ['mousedown', 'mousemove', 'mouseup'] as const;
  for (const name of events) target.addEventListener(name, track, true);
  const release = () => {
    for (const [code, keysym] of heldKeys) original.call(rfb, keysym, code, false);
    heldKeys.clear();
    if (pointer?.buttons) {
      target.querySelector('canvas')?.dispatchEvent(
        new MouseEvent('mouseup', {
          bubbles: true,
          clientX: pointer.x,
          clientY: pointer.y,
          buttons: 0,
        }),
      );
    }
    pointer = undefined;
    rfb.blur();
  };
  return {
    release,
    dispose: () => {
      release();
      rfb.sendKey = original;
      for (const name of events) target.removeEventListener(name, track, true);
    },
  };
}
