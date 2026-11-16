import test from 'node:test';
import assert from 'node:assert/strict';
import {textSizeCase, assertReadableText, assertTextSizeScaling, assertTextSizePreference,
  closeTextSizeUsage} from './text-size.mjs';

const ctx = {assert: (value, code) => assert.ok(value, code)};
const scales = {Small: 0.9, Medium: 1, Large: 1.15};
const baseFonts = {menu_docs: [14], menu_signout: [14], menu_size: [12.5],
  eg_channels: [14], eg_consumers: [14], usage_title: [16], usage_range: [14], usage_apply: [14]};
const reading = fonts => ({fonts, visible: true, fits: true, unobscured: true});
const measured = () => Object.fromEntries(Object.entries(scales).map(([name, scale]) => [name,
  Object.fromEntries(Object.entries(baseFonts).map(([key, fonts]) => [key, reading(fonts.map(font => font * scale))]))]));

test('EG-006 retains the frozen text-size journey and existing browser dependency', () => {
  assert.equal(textSizeCase.id, 'C275:EG-006');
  assert.deepEqual(textSizeCase.steps, ['step_1', 'step_2', 'expected_result']);
  assert.deepEqual(textSizeCase.fixtures, ['browser']);
});

test('text size requires matching rendered preference, saved value and selected control', () => {
  for (const [name, scale] of Object.entries(scales)) {
    const value = {size: name.toLowerCase(), stored: name.toLowerCase(), scale: String(scale), checked: 'true'};
    assertTextSizePreference(ctx, value, name);
    for (const field of ['size', 'stored', 'scale', 'checked'])
      assert.throws(() => assertTextSizePreference(ctx, {...value, [field]: null}, name), /applied_saved_and_selected/);
  }
  assert.throws(() => assertTextSizePreference(ctx, {}, 'Unknown'), /applied_saved_and_selected/);
});

test('unchanged compiled fonts cannot pass despite an updated root preference', () => {
  assertTextSizeScaling(ctx, measured());
  const noBuildPlugin = measured();
  noBuildPlugin.Small = structuredClone(noBuildPlugin.Medium);
  noBuildPlugin.Large = structuredClone(noBuildPlugin.Medium);
  assert.throws(() => assertTextSizeScaling(ctx, noBuildPlugin), /rendered_font_scale/);
  for (const key of Object.keys(baseFonts)) {
    const partial = measured(); partial.Large[key] = structuredClone(partial.Medium[key]);
    assert.throws(() => assertTextSizeScaling(ctx, partial), new RegExp('rendered_font_scale_' + key));
  }
  const compounded = measured(); compounded.Large.usage_title.fonts[0] *= 1.15;
  assert.throws(() => assertTextSizeScaling(ctx, compounded), /rendered_font_scale_usage_title/);
});

test('text-size evidence rejects absent surfaces, missing text, clipping and occlusion', () => {
  for (const fonts of [[], [NaN], [Infinity], [0], [-1]])
    assert.throws(() => assertReadableText(ctx, reading(fonts)), /real_rendered_text/);
  for (const key of ['visible', 'fits', 'unobscured'])
    assert.throws(() => assertReadableText(ctx, {...reading([14]), [key]: false}), /visible_unclipped_unobscured/);
  const missing = measured(); delete missing.Small.eg_consumers;
  assert.throws(() => assertTextSizeScaling(ctx, missing), /all_surfaces_measured/);
  const clipped = measured(); clipped.Large.usage_title.fits = false;
  assert.throws(() => assertTextSizeScaling(ctx, clipped), /visible_unclipped_unobscured/);
  const changed = measured(); changed.Small.menu_docs.fonts.push(12.6);
  assert.throws(() => assertTextSizeScaling(ctx, changed), /rendered_font_scale_menu_docs/);
});

test('preference restoration closes the retained Usage modal before reaching Account', async () => {
  const calls = []; let shown = true;
  const dialog = {isVisible: async () => shown,
    getByRole: (role, options) => {
      assert.equal(role, 'button'); assert.deepEqual(options, {name: 'Close', exact: true});
      return {click: async () => {calls.push('close'); shown = false;}};
    },
    waitFor: async options => {assert.deepEqual(options, {state: 'hidden'}); calls.push('hidden');}};
  const page = {getByRole: (role, options) => {
    assert.equal(role, 'dialog'); assert.deepEqual(options, {name: 'Your usage', exact: true}); return dialog;
  }};
  await closeTextSizeUsage(page); await closeTextSizeUsage(page);
  assert.deepEqual(calls, ['close', 'hidden']);
  shown = true; dialog.waitFor = async () => {throw Error('retained_modal_still_visible');};
  await assert.rejects(closeTextSizeUsage(page), /retained_modal_still_visible/);
});
