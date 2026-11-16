import {define, openBrowser, account, visible, uiReceipt, same} from './helpers.mjs';

const sizes = [['Small', 0.9], ['Medium', 1], ['Large', 1.15]];
const sampleKeys = ['menu_docs', 'menu_signout', 'menu_size', 'eg_channels', 'eg_consumers',
  'usage_title', 'usage_range', 'usage_apply'];
const menu = page => page.getByRole('menu', {name: 'Account', exact: true});
const choice = (page, name) => menu(page).getByRole('menuitemradio', {name, exact: true});
const usage = page => page.getByRole('dialog', {name: 'Your usage', exact: true});

export function assertTextSizePreference(ctx, value, name) {
  const scale = sizes.find(([label]) => label === name)?.[1];
  ctx.assert(scale !== undefined && value.size === name.toLowerCase()
    && value.stored === name.toLowerCase() && Number(value.scale) === scale
    && value.checked === 'true', 'text_size_applied_saved_and_selected');
}

export function assertReadableText(ctx, value) {
  ctx.assert(Array.isArray(value.fonts) && value.fonts.length > 0
    && value.fonts.every(font => Number.isFinite(font) && font > 0), 'text_size_real_rendered_text');
  ctx.assert(value.visible === true && value.fits === true && value.unobscured === true,
    'text_size_text_visible_unclipped_unobscured');
}

// The build plugin scales consuming elements, not only the root variable. A
// missing compiled plugin or one unscaled surface must fail the actual journey.
export function assertTextSizeScaling(ctx, measurements) {
  for (const [name, scale] of sizes) {
    const samples = measurements[name];
    ctx.assert(samples && same(Object.keys(samples).sort(), [...sampleKeys].sort()), 'text_size_all_surfaces_measured');
    for (const key of sampleKeys) {
      const value = samples[key], reference = measurements.Medium?.[key];
      assertReadableText(ctx, value);
      ctx.assert(reference && value.fonts.length === reference.fonts.length
        && value.fonts.every((font, index) => Math.abs(font - reference.fonts[index] * scale) <= 0.08),
      'text_size_rendered_font_scale_' + key);
    }
  }
}

export async function measureReadableText(ctx, locator, interactive = false) {
  await visible(ctx, locator, 'text_size_sample_visible');
  await locator.scrollIntoViewIfNeeded();
  if (interactive) await locator.click({trial: true});
  const value = await locator.evaluate(async element => {
    await document.fonts.ready;
    const fonts = [], boxes = [];
    const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
    let visible = true, fits = true, unobscured = true;
    const within = (box, clip, x, y) => (!x || (box.left >= clip.left - 1 && box.right <= clip.right + 1))
      && (!y || (box.top >= clip.top - 1 && box.bottom <= clip.bottom + 1));
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      if (!node.textContent.trim() || node.parentElement.closest('[aria-hidden="true"], svg')) continue;
      const style = getComputedStyle(node.parentElement), range = document.createRange();
      range.selectNodeContents(node);
      const rectangles = [...range.getClientRects()].filter(rect => rect.width > 0 && rect.height > 0);
      fonts.push(Number.parseFloat(style.fontSize));
      visible &&= rectangles.length > 0 && style.visibility === 'visible';
      for (const rect of rectangles) {
        fits &&= within(rect, {left: 0, top: 0, right: innerWidth, bottom: innerHeight}, true, true);
        for (let parent = node.parentElement; parent; parent = parent.parentElement) {
          const css = getComputedStyle(parent), bound = parent.getBoundingClientRect();
          visible &&= Number(css.opacity) > 0 && css.display !== 'none' && css.visibility === 'visible';
          const clip = {left: bound.left + parent.clientLeft, top: bound.top + parent.clientTop,
            right: bound.left + parent.clientLeft + parent.clientWidth,
            bottom: bound.top + parent.clientTop + parent.clientHeight};
          fits &&= within(rect, clip, /^(hidden|clip|auto|scroll)$/.test(css.overflowX),
            /^(hidden|clip|auto|scroll)$/.test(css.overflowY));
        }
        const hit = document.elementFromPoint((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);
        unobscured &&= hit !== null && element.contains(hit);
        boxes.push({width: rect.width, height: rect.height});
      }
    }
    return {fonts, boxes, visible, fits, unobscured};
  });
  assertReadableText(ctx, value);
  return value;
}

export async function closeTextSizeUsage(page) {
  const dialog = usage(page);
  if (await dialog.isVisible()) {
    await dialog.getByRole('button', {name: 'Close', exact: true}).click();
    await dialog.waitFor({state: 'hidden'});
  }
}

async function selectSize(ctx, page, name) {
  await account(page); await choice(page, name).click();
  const value = await page.evaluate(() => ({size: document.documentElement.dataset.systemTextSize,
    scale: document.documentElement.style.getPropertyValue('--cma-system-font-scale'),
    stored: localStorage.getItem('cma:system-text-size')}));
  value.checked = await choice(page, name).getAttribute('aria-checked');
  assertTextSizePreference(ctx, value, name);
}

export const textSizeCase = define(6, 'browser', ['browser'], async ctx => {
  let page, original;
  const measurements = {};
  await ctx.step('step_1', async () => {
    ({page} = await openBrowser(ctx)); await account(page);
    const selected = (await menu(page).getByRole('menuitemradio', {checked: true}).allTextContents())
      .map(value => value.trim()).filter(value => sizes.some(([name]) => name === value));
    ctx.assert(selected.length === 1, 'text_size_initial_preference_unique'); original = selected[0];
    ctx.defer('restore_text_size', async () => {
      await closeTextSizeUsage(page); await selectSize(ctx, page, original);
      await page.keyboard.press('Escape');
    });
    for (const [name] of sizes) {
      await selectSize(ctx, page, name);
      const samples = measurements[name] = {};
      samples.menu_docs = await measureReadableText(ctx, menu(page).getByRole('menuitem', {name: 'Docs', exact: true}), true);
      samples.menu_signout = await measureReadableText(ctx, menu(page).getByRole('menuitem', {name: 'Sign out', exact: true}), true);
      samples.menu_size = await measureReadableText(ctx, choice(page, name), true);
      await page.keyboard.press('Escape');
      const nav = page.getByRole('navigation', {name: 'Event Gateway navigation', exact: true});
      samples.eg_channels = await measureReadableText(ctx, nav.getByRole('button', {name: 'Channels', exact: true}), true);
      samples.eg_consumers = await measureReadableText(ctx, nav.getByRole('button', {name: 'Consumers', exact: true}), true);
    }
  });
  await ctx.step('step_2', async () => {
    for (const [name] of sizes) {
      await selectSize(ctx, page, name);
      await menu(page).getByRole('menuitem', {name: 'Usage', exact: true}).click();
      const dialog = usage(page), close = dialog.getByRole('button', {name: 'Close', exact: true});
      await visible(ctx, dialog, 'text_size_usage_visible');
      // Trial click waits for modal entrance and confirms the close control is
      // reachable without dismissing the surface before its text is measured.
      await close.click({trial: true});
      const samples = measurements[name];
      samples.usage_title = await measureReadableText(ctx, dialog.getByRole('heading', {name: 'Your usage', exact: true}));
      samples.usage_range = await measureReadableText(ctx, dialog.getByRole('button', {name: 'Usage time range', exact: true}), true);
      samples.usage_apply = await measureReadableText(ctx, dialog.getByRole('button', {name: 'Apply usage filters', exact: true}), true);
      await uiReceipt(ctx, page, 'text_size_usage_receipt');
      await closeTextSizeUsage(page);
      await visible(ctx, page.getByRole('navigation', {name: 'Event Gateway navigation', exact: true}), 'text_size_gateway_context_retained');
    }
    assertTextSizeScaling(ctx, measurements);
    await ctx.observe('text_size_rendered_measurements', async () => measurements);
    await selectSize(ctx, page, original); await page.keyboard.press('Escape');
  });
  await ctx.step('expected_result', async () => {
    await page.reload(); await account(page);
    const value = await page.evaluate(() => ({size: document.documentElement.dataset.systemTextSize,
      scale: document.documentElement.style.getPropertyValue('--cma-system-font-scale'),
      stored: localStorage.getItem('cma:system-text-size')}));
    value.checked = await choice(page, original).getAttribute('aria-checked');
    assertTextSizePreference(ctx, value, original);
    const restored = await measureReadableText(ctx, menu(page).getByRole('menuitem', {name: 'Docs', exact: true}), true);
    ctx.assert(restored.fonts.every((font, index) => Math.abs(font - measurements[original].menu_docs.fonts[index]) <= 0.08),
      'text_size_restored_rendered_scale_after_reload');
    await uiReceipt(ctx, page);
  });
});
