import { expect, it } from 'vitest';
import { spreadsheetLinkIssue } from './spreadsheetLinkValidation';
import { taskSubmission } from './taskSubmission';
import { compileTaskTemplate, type TaskTemplate } from './taskTemplateModel';

const spreadsheet = (link: string): Extract<TaskTemplate, { id: 'spreadsheet' }> => ({
  id: 'spreadsheet',
  version: 1,
  fields: { link },
});
function submission(template: TaskTemplate) {
  return taskSubmission(
    { text: '', template, machineIncluded: false, repositoryUrl: '', repositoryRef: '' },
    { machine: false, template: true },
  );
}

it.each([
  'not-a-url',
  '/tmp/data.csv',
  '//example.com/data.csv',
  'example.com/data.csv',
  'javascript:alert(1)',
  'data:text/csv,a,b',
  'file:///tmp/data.csv',
  'ftp://example.com/data.csv',
  'https:example.com',
  'https:/example.com',
  'https:///example.com',
  'https:////example.com',
  'https://',
  'https://?file=data.csv',
  'https://#sheet',
  'https://example.com:invalid/data.csv',
  'https://example.com:65536/data.csv',
  'https://[::1/data.csv',
  'https://exa%mple.com/data.csv',
  'https://reader@example.com/data.csv',
  'https://@example.com/data.csv',
  'https://:@example.com/data.csv',
  'https://example.com/data set.csv',
  'https://example.com/data\u2003set.csv',
  'https://example.com/data\\set.csv',
  'https:\\example.com/data.csv',
  '\nhttps://example.com/data.csv',
  'https://example.com/data.csv\t',
])('rejects fresh spreadsheet input without URL parser repair: %j', (link) => {
  const template = spreadsheet(link);
  expect(submission(template).issue).toEqual({
    field: 'link',
    message: 'Enter a complete HTTP or HTTPS link to your spreadsheet.',
  });
  expect(compileTaskTemplate(template).issue).toBeUndefined();
  expect(template.fields.link).toBe(link);
});

it('rejects every C0/C1 control before trimming or URL parsing', () => {
  const controls = [
    ...Array.from({ length: 32 }, (_, n) => n),
    ...Array.from({ length: 33 }, (_, n) => n + 127),
  ];
  for (const code of controls) {
    const character = String.fromCharCode(code);
    for (const link of [
      `${character}https://example.com/data.csv`,
      `https://example.com/da${character}ta.csv`,
      `https://example.com/data.csv${character}`,
    ])
      expect(spreadsheetLinkIssue(spreadsheet(link)), JSON.stringify(link)).toHaveProperty(
        'field',
        'link',
      );
  }
});

it.each([
  ' https://example.com/synthetic.csv ',
  'http://example.com/data.csv',
  'HTTPS://EXAMPLE.COM:443/a/../data.csv',
  'https://example.com:8443/data.csv',
  'http://192.0.2.1/data.csv',
  'https://[2001:db8::1]:8443/data.csv',
  'https://例子.测试/表格.csv',
  'https://example.com/data%20set.csv',
  'https://example.com/@sheet?owner=reader@example.com#tab',
  'https://example.com/download?signature=synthetic%2Fvalue&part=2#sheet',
  'https://docs.google.com/spreadsheets/d/synthetic/edit#gid=0',
  '\u2003https://example.com/data.csv\u3000',
])('accepts remote link syntax without rewriting draft or compiled bytes: %j', (link) => {
  const template = spreadsheet(link);
  const before = JSON.stringify(template);
  const compiled = compileTaskTemplate(template);
  expect(spreadsheetLinkIssue(template)).toBeUndefined();
  expect(submission(template)).toEqual({ ...compiled, issue: undefined });
  expect(JSON.stringify(template)).toBe(before);
});

it.each(['', '   ', '\t\n', '[link]', ' [link] '])(
  'preserves the existing required/placeholder error for %j',
  (link) => {
    expect(spreadsheetLinkIssue(spreadsheet(link))).toBeUndefined();
    expect(submission(spreadsheet(link)).issue).toEqual({
      field: 'link',
      message: 'Enter spreadsheet link before starting.',
    });
  },
);

it('preserves the compiler byte limit and leaves other templates unchanged', () => {
  expect(submission(spreadsheet('https://example.com/' + '界'.repeat(87382))).issue?.message).toBe(
    'Your task exceeds the 262144-byte limit.',
  );
  const research: TaskTemplate = { id: 'research', version: 1, fields: { topic: 'not-a-url' } };
  expect(spreadsheetLinkIssue(research)).toBeUndefined();
  expect(submission(research).issue).toBeUndefined();
});
