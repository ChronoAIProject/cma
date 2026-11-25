import type { TaskTemplate, TemplateIssue } from './taskTemplateModel';

export function spreadsheetLinkIssue(template: TaskTemplate): TemplateIssue | undefined {
  if (template.id !== 'spreadsheet') return;
  const raw = template.fields.link;
  const controls = Array.from(raw).some((character) => {
    const code = character.charCodeAt(0);
    return code < 32 || (code >= 127 && code <= 159);
  });
  const value = raw.replace(/^\p{White_Space}+|\p{White_Space}+$/gu, '');
  if (!value || value === '[link]') return;
  const issue = {
    field: 'link',
    message: 'Enter a complete HTTP or HTTPS link to your spreadsheet.',
  };
  if (controls || raw.includes('\\') || /\s|\p{White_Space}/u.test(value)) return issue;
  const authority = /^https?:\/\/([^/?#]+)/i.exec(value)?.[1];
  if (!authority || authority.includes('@')) return issue;
  try {
    const url = new URL(value);
    if (
      (url.protocol === 'http:' || url.protocol === 'https:') &&
      url.hostname &&
      !url.username &&
      !url.password
    )
      return;
  } catch {
    return issue;
  }
  return issue;
}
