/** Detect a lost browser display without treating an idle or black desktop as lost. */
export function watchDesktopDisplay(target: HTMLElement, rescale: () => void, lost: () => void) {
  let stopped = false;
  let misses = 0;
  const stop = () => {
    stopped = true;
    window.clearInterval(timer);
    target.removeEventListener('contextlost', contextLost, true);
    document.removeEventListener('visibilitychange', visible);
  };
  const unavailable = () => {
    if (stopped) return;
    stop();
    lost();
  };
  const contextLost = () => unavailable();
  const check = () => {
    if (stopped) return;
    const bounds = target.getBoundingClientRect();
    // Hidden tabs/panes are allowed to have no rendered size. Check again when
    // visible; never interrupt the shared desktop merely because it is hidden.
    if (document.hidden || bounds.width === 0 || bounds.height === 0) {
      misses = 0;
      return;
    }
    const canvas = target.querySelector('canvas');
    let painted = false;
    if (canvas && canvas.width > 0 && canvas.height > 0) {
      if (!canvas.getBoundingClientRect().width || !canvas.getBoundingClientRect().height)
        rescale();
      const size = canvas.getBoundingClientRect();
      try {
        // All remote RFB pixels are opaque, including a legitimately black or
        // unchanged screen. Two sparse alpha reads avoid continuous GPU reads
        // and allow a transient framebuffer resize to finish repainting.
        painted =
          size.width > 0 &&
          size.height > 0 &&
          !!canvas
            .getContext('2d')
            ?.getImageData(Math.floor(canvas.width / 2), Math.floor(canvas.height / 2), 1, 1)
            .data[3];
      } catch {
        // A lost context can reject reads before its contextlost event arrives.
      }
    }
    misses = painted ? 0 : misses + 1;
    if (misses >= 2) unavailable();
  };
  const visible = () => {
    if (!document.hidden) {
      rescale();
      check();
    }
  };
  const timer = window.setInterval(check, 2_000);
  target.addEventListener('contextlost', contextLost, true);
  document.addEventListener('visibilitychange', visible);
  return stop;
}
