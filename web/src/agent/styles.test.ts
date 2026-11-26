import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { assert, describe, expect, it } from 'vitest';

function sheet(name: string): CSSRuleList {
  const element = document.createElement('style');
  element.textContent = readFileSync(resolve(process.cwd(), 'src/agent', name), 'utf8');
  document.head.append(element);
  const rules = element.sheet!.cssRules;
  element.remove();
  return rules;
}

function declaration(rules: CSSRuleList, selector: string): CSSStyleDeclaration {
  const rule = Array.from(rules).find(
    (candidate) => candidate instanceof CSSStyleRule && candidate.selectorText === selector,
  );
  expect(rule, selector).toBeDefined();
  return (rule as CSSStyleRule).style;
}

describe('conversation geometry with readable labels', () => {
  it('retains the original Thinking spacing and capped body without lowering contrast', () => {
    const rules = sheet('Thinking.css');
    expect(declaration(rules, '.thinking').getPropertyValue('margin-bottom')).toBe('');
    const body = declaration(rules, '.thinking__body');
    expect(body.getPropertyValue('max-height')).toBe('120px');
    expect(body.getPropertyValue('padding')).toBe('12px');
    expect(body.getPropertyValue('font-size')).toBe('13px');
    expect(body.getPropertyValue('line-height')).toBe('20px');
    expect(body.getPropertyValue('color')).toBe('var(--dsw-alias-label-secondary)');
    expect(declaration(rules, '.thinking__activitybody').getPropertyValue('color')).toBe(
      'var(--dsw-alias-label-secondary)',
    );
  });

  it('draws the composer as one rounded box whose draft grows to a cap and scrolls', () => {
    const rules = sheet('Composer.css');
    const box = declaration(rules, '.agent-composer__box');
    expect(box.getPropertyValue('border-radius')).toBe('16px');
    expect(box.getPropertyValue('border')).toBe('1px solid var(--dsw-alias-border-l2)');
    expect(box.getPropertyValue('background')).toBe('var(--dsw-specific-menu)');
    expect(box.getPropertyValue('box-shadow')).toBe('var(--dsw-shadow-lv1)');
    const field = declaration(rules, '.agent-composer textarea');
    expect(field.getPropertyValue('min-height')).toBe('40px');
    expect(field.getPropertyValue('max-height')).toBe('min(336px, 40dvh)');
    expect(field.getPropertyValue('overflow-y')).toBe('auto');
    expect(field.getPropertyValue('padding')).toBe('8px 16px');
    expect(field.getPropertyValue('font-size')).toBe('16px');
    expect(field.getPropertyValue('line-height')).toBe('24px');
    expect(
      declaration(rules, '.agent-composer textarea::placeholder').getPropertyValue('color'),
    ).toBe('var(--dsw-alias-label-caption)');
    const stopScope = "'stop-scope stop-scope stop-scope stop-scope'";
    const settingsPanel = "'settings-panel settings-panel settings-panel settings-panel'";
    const status = "'status status status status'";
    expect(
      declaration(rules, '.agent-composer__controls')
        .getPropertyValue('grid-template-areas')
        .replace(/\s+/g, ' '),
    ).toBe([stopScope, "'attach steering settings send'", settingsPanel, status].join(' '));

    const narrow = Array.from(rules)
      .filter((rule) => rule instanceof CSSContainerRule)
      .find((rule) => rule.conditionText === 'agent-conversation (max-width: 30rem)');
    assert.isDefined(narrow, 'narrow conversation container rule');
    expect(
      declaration(narrow.cssRules, '.agent-composer__controls')
        .getPropertyValue('grid-template-areas')
        .replace(/\s+/g, ' '),
    ).toBe(
      [
        "'settings settings settings settings'",
        stopScope,
        "'attach steering . send'",
        settingsPanel,
        status,
      ].join(' '),
    );
    expect(declaration(rules, '.agent-composer__stop-scope').getPropertyValue('grid-area')).toBe(
      'stop-scope',
    );
  });

  it('uses a 32px Send circle with 44px mobile targets and a neutral empty state', () => {
    const rules = sheet('Composer.css');
    const submit = declaration(rules, '.agent-composer .agent-composer__submit');
    expect(submit.getPropertyValue('width')).toBe('32px');
    expect(submit.getPropertyValue('height')).toBe('32px');
    expect(submit.getPropertyValue('border-radius')).toBe('50%');
    const empty = declaration(rules, '.agent-composer .agent-composer__submit:disabled');
    expect(empty.getPropertyValue('background')).toBe('var(--dsw-alias-bg-layer-3)');
    expect(empty.getPropertyValue('opacity')).toBe('1');
    const media = Array.from(rules).find(
      (rule) => rule instanceof CSSMediaRule && rule.conditionText === '(max-width: 30rem)',
    ) as CSSMediaRule;
    const narrow = declaration(
      media.cssRules,
      '.agent-composer .agent-composer__submit:not(.agent-composer__submit--stop)',
    );
    expect(narrow.getPropertyValue('width')).toBe('44px');
    const stop = declaration(rules, '.agent-composer .agent-composer__submit--stop');
    expect(stop.getPropertyValue('background')).toBe('var(--dsw-alias-workspace-control-bg)');
  });

  it('retains the original 12px gap and track in the resource metrics row', () => {
    const rules = sheet('Resources.css');
    const resources = declaration(rules, '.resources');
    expect(resources.getPropertyValue('grid-template-columns')).toBe('repeat(3, minmax(0, 1fr))');
    expect(resources.getPropertyValue('gap')).toBe('12px');
    expect(declaration(rules, '.resources__track').getPropertyValue('height')).toBe('2px');
  });

  it('draws the context ring with a neutral track, an accent arc and an inverted tooltip', () => {
    const rules = sheet('ContextRing.css');
    expect(declaration(rules, '.context-ring__track').getPropertyValue('stroke')).toBe(
      'var(--dsw-alias-border-l2)',
    );
    expect(declaration(rules, '.context-ring__arc').getPropertyValue('stroke')).toBe(
      'var(--dsw-alias-state-business-primary)',
    );
    const tooltip = declaration(rules, '.context-ring__tooltip');
    expect(tooltip.getPropertyValue('background')).toBe('var(--dsw-alias-tooltip-bg)');
    expect(tooltip.getPropertyValue('color')).toBe('var(--dsw-alias-label-primary-inverted)');
    expect(tooltip.getPropertyValue('font-variant-numeric')).toBe('tabular-nums');
  });

  it('keeps the compact toolbar in flow without a hidden hover banner', () => {
    const rules = sheet('AgentHeader.css');
    const toolbar = declaration(rules, '.agent-toolbar');
    expect(toolbar.getPropertyValue('display')).toBe('flex');
    expect(toolbar.getPropertyValue('min-height')).toBe('48px');
    expect(toolbar.getPropertyValue('position')).toBe('');
    expect(toolbar.getPropertyValue('opacity')).toBe('');
  });
});
