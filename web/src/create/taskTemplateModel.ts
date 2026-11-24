import { taskTemplate, type TaskTemplate } from '../api-client/agents/task-template';
export type { TaskTemplate } from '../api-client/agents/task-template';
export type TaskTemplateId = TaskTemplate['id'];
export interface TemplateIssue {
  readonly field?: string;
  readonly message: string;
}
const field = (key: string, label: string, placeholder: string) => ({ key, label, placeholder });
export const taskTemplates = [
  {
    id: 'research',
    name: 'Research a topic',
    fields: [field('topic', 'Topic', '[topic]')],
    legacy: 'Research [topic] and summarize the key findings, with links to sources.',
  },
  {
    id: 'spreadsheet',
    name: 'Analyze a spreadsheet',
    fields: [field('link', 'Spreadsheet link', '[link]')],
    legacy: 'Analyze the spreadsheet at [link] and point out trends, outliers and what to do next.',
  },
  {
    id: 'document',
    name: 'Write a document',
    fields: [
      field('type', 'Document type', '[type of document]'),
      field('subject', 'Subject', '[subject]'),
      field('audience', 'Audience', '[audience]'),
    ],
    legacy: 'Write a [type of document] about [subject] for [audience].',
  },
  {
    id: 'website',
    name: 'Build a website',
    fields: [field('project', 'Project', '[project]')],
    legacy: 'Build a one-page website for [project] and share a preview link.',
  },
  {
    id: 'code',
    name: 'Write code',
    fields: [
      field('language', 'Language', '[language]'),
      field('behavior', 'What should the code do?', '[does what]'),
    ],
    legacy: 'Write a [language] script that [does what].',
  },
] as const;
export function templateDefinition(id: TaskTemplateId) {
  return taskTemplates.find((template) => template.id === id)!;
}
export function emptyTemplate(id: TaskTemplateId, version: 1 | 2 = 1): TaskTemplate {
  return taskTemplate({
    id,
    version,
    fields: Object.fromEntries(templateDefinition(id).fields.map(({ key }) => [key, ''])),
  });
}
export function legacyTemplate(text: string): TaskTemplate | undefined {
  const match = taskTemplates.find((template) => template.legacy === text);
  return match ? emptyTemplate(match.id) : undefined;
}
export function changeTemplateField(
  template: TaskTemplate,
  field: string,
  value: string,
  allowV2 = false,
): TaskTemplate {
  if ((template.fields as Readonly<Record<string, string>>)[field] === value) return template;
  return taskTemplate({
    ...template,
    version: allowV2 ? 2 : template.version,
    fields: { ...template.fields, [field]: value },
  });
}
const trim = (value: string) => value.replace(/^\p{White_Space}+|\p{White_Space}+$/gu, '');
export function compileTaskTemplate(template: TaskTemplate): {
  text: string;
  issue?: TemplateIssue;
} {
  const fields: Readonly<Record<string, string>> = template.fields;
  for (const { key, label, placeholder } of templateDefinition(template.id).fields) {
    const value = trim(fields[key] ?? '');
    if (!value || value === placeholder)
      return {
        text: '',
        issue: { field: key, message: `Enter ${label.toLowerCase()} before starting.` },
      };
    if (new TextEncoder().encode(value).length > 262144)
      return {
        text: '',
        issue: { field: key, message: 'Your task exceeds the 262144-byte limit.' },
      };
  }
  const value = (key: string) => trim(fields[key]!);
  const text = (() => {
    switch (template.id) {
      case 'research':
        if (template.version === 2)
          return `Research the following topic:\n\n${value('topic')}\n\nSummarize the key findings, with links to sources.`;
        return `Research ${value('topic')} and summarize the key findings, with links to sources.`;
      case 'spreadsheet':
        return `Analyze the spreadsheet at ${value('link')} and point out trends, outliers and what to do next.`;
      case 'document':
        return `Write a ${value('type')} about ${value('subject')} for ${value('audience')}.`;
      case 'website':
        return `Build a one-page website for ${value('project')} and share a preview link.`;
      case 'code':
        if (template.version === 2) {
          const behavior = value('behavior');
          return `Write a ${value('language')} script for the following task:\n\n${behavior}${/[.!?。！？]$/u.test(behavior) ? '' : '.'}`;
        }
        return `Write a ${value('language')} script that ${value('behavior')}.`;
    }
  })();
  return new TextEncoder().encode(text).length > 262144
    ? { text, issue: { message: 'Your task exceeds the 262144-byte limit.' } }
    : { text };
}
export function parseTaskTemplate(value: unknown) {
  return taskTemplate(value);
}
