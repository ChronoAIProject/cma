export type TopographicColor = readonly [number, number, number];
export type TopographicPalette = {
  line: TopographicColor;
  accent: TopographicColor;
};

function resolveColor(host: HTMLElement, token: string): TopographicColor | null {
  const probe = document.createElement('span');
  probe.style.color = `var(${token})`;
  probe.hidden = true;
  host.append(probe);
  try {
    const value = getComputedStyle(probe).color;
    const channels =
      /^rgb\(\s*(\d+(?:\.\d+)?)\s*,\s*(\d+(?:\.\d+)?)\s*,\s*(\d+(?:\.\d+)?)\s*\)$/.exec(value);
    if (!channels) return null;
    const [r, g, b] = channels.slice(1).map(Number);
    if (r === undefined || g === undefined || b === undefined) return null;
    if ([r, g, b].some((value) => !Number.isFinite(value) || value < 0 || value > 255)) return null;
    return [r / 255, g / 255, b / 255];
  } finally {
    probe.remove();
  }
}

export function readTopographicPalette(host: HTMLElement): TopographicPalette | null {
  const style = getComputedStyle(host);
  if (
    !style.getPropertyValue('--dsw-alias-border-l2').trim() ||
    !style.getPropertyValue('--dsw-alias-brand-primary').trim()
  )
    return null;
  const line = resolveColor(host, '--dsw-alias-border-l2');
  const accent = resolveColor(host, '--dsw-alias-brand-primary');
  return line && accent ? { line, accent } : null;
}
