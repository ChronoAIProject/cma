import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

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

  it('centers the compact composer field and lets its editor expand upward', () => {
    const rules = sheet('Agent.css');
    const capsule = declaration(rules, '.agent-composer__capsule');
    expect(capsule.getPropertyValue('height')).toBe('38px');
    expect(capsule.getPropertyValue('position')).toBe('relative');
    const editor = declaration(rules, '.agent-composer__editor');
    expect(editor.getPropertyValue('position')).toBe('absolute');
    expect(editor.getPropertyValue('bottom')).toBe('0');
    expect(editor.getPropertyValue('background')).toBe('var(--dsw-specific-input-major)');
    const field = declaration(rules, '.agent-composer textarea');
    expect(field.getPropertyValue('max-height')).toBe('36px');
    expect(field.getPropertyValue('padding')).toBe('6px 12px 6px 16px');
    expect(field.getPropertyValue('font-size')).toBe('16px');
    expect(field.getPropertyValue('line-height')).toBe('24px');
    expect(
      declaration(rules, '.agent-composer textarea::placeholder').getPropertyValue('color'),
    ).toBe('var(--dsw-alias-label-caption)');
  });

  it('uses compact 32px Send and Stop controls with 44px mobile targets', () => {
    const rules = sheet('Agent.css');
    const submit = declaration(rules, '.agent-composer .agent-composer__submit');
    expect(submit.getPropertyValue('width')).toBe('32px');
    expect(submit.getPropertyValue('height')).toBe('32px');
    expect(submit.getPropertyValue('border-radius')).toBe('50%');
    const media = Array.from(rules).find(
      (rule) => rule instanceof CSSMediaRule && rule.conditionText === '(max-width: 30rem)',
    ) as CSSMediaRule;
    const narrow = declaration(media.cssRules, '.agent-composer .agent-composer__submit');
    expect(narrow.getPropertyValue('width')).toBe('44px');
    expect(narrow.getPropertyValue('height')).toBe('44px');
    expect(narrow.getPropertyValue('justify-self')).toBe('center');
  });

  it('retains the original 12px gaps in both metrics rows', () => {
    const rules = sheet('Resources.css');
    const usage = declaration(rules, '.agent-instruments__usage');
    const resources = declaration(rules, '.resources');
    expect(usage.getPropertyValue('grid-template-columns')).toBe('repeat(2, minmax(0, 1fr))');
    expect(resources.getPropertyValue('grid-template-columns')).toBe('repeat(3, minmax(0, 1fr))');
    expect(usage.getPropertyValue('gap')).toBe('12px');
    expect(resources.getPropertyValue('gap')).toBe('12px');
    expect(declaration(rules, '.resources__track').getPropertyValue('height')).toBe('2px');
    expect(declaration(rules, '.agent-instruments').getPropertyValue('color')).toBe(
      'var(--dsw-alias-label-caption)',
    );
  });
});
