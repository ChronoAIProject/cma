import { describe, expect, it } from 'vitest';
import vectors from '../../../api/crates/layer2-core/src/sandbox/task_template_vectors.json';
import {
  compileTaskTemplate,
  changeTemplateField,
  emptyTemplate,
  legacyTemplate,
  parseTaskTemplate,
  taskTemplates,
} from './taskTemplateModel';

describe('versioned task templates', () => {
  it.each(vectors)(
    'compiles shared $template.id vector without interpreting brackets',
    ({ template, message }) => {
      expect(compileTaskTemplate(parseTaskTemplate(template))).toEqual({ text: message });
    },
  );
  it.each(taskTemplates)(
    'requires every $id field and refuses only its placeholder',
    (definition) => {
      expect(compileTaskTemplate(emptyTemplate(definition.id)).issue).toBeDefined();
      const fields = Object.fromEntries(definition.fields.map(({ key }) => [key, 'synthetic']));
      for (const { key, placeholder } of definition.fields) {
        for (const value of ['', ' \n\u0085 ', ` ${placeholder} `]) {
          expect(
            compileTaskTemplate(
              parseTaskTemplate({
                id: definition.id,
                version: 1,
                fields: { ...fields, [key]: value },
              }),
            ).issue?.field,
          ).toBe(key);
        }
      }
    },
  );
  it('recognizes only exact legacy templates and enforces compiled UTF-8 size', () => {
    for (const template of taskTemplates)
      expect(legacyTemplate(template.legacy)?.id).toBe(template.id);
    expect(legacyTemplate('Explain [topic] and [1, 2]')).toBeUndefined();
    expect(
      compileTaskTemplate({ id: 'research', version: 1, fields: { topic: '你'.repeat(87382) } })
        .issue,
    ).toBeDefined();
  });
  it.each([
    { id: 'unknown', version: 1, fields: {} },
    { id: 'research', version: 3, fields: { topic: 'valid' } },
    { id: 'research', version: 1, fields: {} },
    { id: 'research', version: 1, fields: { topic: 'valid', extra: 'value' } },
    { id: 'research', version: 1, fields: { topic: 'valid' }, extra: true },
  ])('rejects malformed metadata: %j', (value) => expect(() => parseTaskTemplate(value)).toThrow());
});

it('preserves legacy compilation until the owner edits a draft', () => {
  const retained = parseTaskTemplate({
    id: 'research',
    version: 1,
    fields: { topic: 'Saved topic.' },
  });
  expect(compileTaskTemplate(retained).text).toBe(
    'Research Saved topic. and summarize the key findings, with links to sources.',
  );
  const edited = changeTemplateField(retained, 'topic', 'Updated topic.', true);
  expect(edited.version).toBe(2);
  expect(compileTaskTemplate(edited).text).toBe(
    'Research the following topic:\n\nUpdated topic.\n\nSummarize the key findings, with links to sources.',
  );
  expect(retained.fields).toEqual({ topic: 'Saved topic.' });
});
