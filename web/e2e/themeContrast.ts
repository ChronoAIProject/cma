import { expect, type Locator } from '@playwright/test';

/** Check the actual rendered text paints, including inherited dialog/menu backgrounds. */
export async function expectReadableText(surface: Locator) {
  const result = await surface.evaluate((root) => {
    const rgba = (value: string) => {
      const parts = value.match(/[\d.]+/g)?.map(Number);
      if (!parts || parts.length < 3) throw new Error(`Unsupported computed paint: ${value}`);
      return [parts[0]!, parts[1]!, parts[2]!, parts[3] ?? 1];
    };
    const over = (front: number[], back: number[]) => {
      const alpha = front[3]!;
      return [0, 1, 2].map((index) => front[index]! * alpha + back[index]! * (1 - alpha)).concat(1);
    };
    const luminance = (value: number[]) =>
      value.slice(0, 3).reduce((sum, channel, index) => {
        const c = channel / 255;
        return (
          sum +
          (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4) *
            [0.2126, 0.7152, 0.0722][index]!
        );
      }, 0);
    const failures: string[] = [];
    let checked = 0;
    const walk = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    for (let text = walk.nextNode(); text; text = walk.nextNode()) {
      if (!text.textContent?.trim()) continue;
      const node = text.parentElement;
      if (
        !node ||
        node.closest(
          '[aria-hidden="true"], [inert], [hidden], :disabled, [aria-disabled="true"], .sr-only',
        )
      )
        continue;
      if (node.getBoundingClientRect().width === 0 || node.getBoundingClientRect().height === 0)
        continue;
      const parents: Element[] = [];
      for (let parent: Element | null = node; parent; parent = parent.parentElement)
        parents.unshift(parent);
      if (
        parents.some(
          (parent) =>
            getComputedStyle(parent).opacity === '0' ||
            getComputedStyle(parent).visibility === 'hidden',
        )
      )
        continue;
      let background = [255, 255, 255, 1];
      for (const parent of parents)
        background = over(rgba(getComputedStyle(parent).backgroundColor), background);
      const style = getComputedStyle(node);
      const foreground = over(
        rgba(node instanceof SVGElement ? style.fill : style.color),
        background,
      );
      const levels = [luminance(foreground), luminance(background)].sort((a, b) => a - b);
      const ratio = (levels[1]! + 0.05) / (levels[0]! + 0.05);
      checked++;
      if (ratio < 4.5)
        failures.push(
          `${text.textContent.trim().slice(0, 70)}: ${ratio.toFixed(2)}:1 (${style.color})`,
        );
    }
    return { checked, failures };
  });
  expect(result.checked).toBeGreaterThan(0);
  expect(result.failures).toEqual([]);
}
