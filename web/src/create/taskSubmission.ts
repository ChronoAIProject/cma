import type { KeyboardEvent } from 'react';
import { validRepository, validRepositoryRef } from './repository';
import { compileTaskTemplate, type TaskTemplate, type TemplateIssue } from './taskTemplateModel';

export function taskSubmission(
  draft: {
    readonly text: string;
    readonly template?: TaskTemplate;
    readonly machineIncluded: boolean;
    readonly repositoryUrl: string;
    readonly repositoryRef: string;
  },
  support: { machine: boolean; repository?: boolean; template?: boolean },
): { text: string; issue?: TemplateIssue } {
  const compiled = draft.template ? compileTaskTemplate(draft.template) : { text: draft.text };
  const message =
    draft.machineIncluded && !support.machine
      ? 'A shared desktop requires a new Sandbox. Keep the task and clear Shared desktop in the Cloud computer menu.'
      : draft.template && support.template !== true
        ? 'Template tasks are unavailable on this service. Your fields are saved; you can also write a task without a template.'
        : !compiled.text.trim() && !compiled.issue
          ? 'Describe a task before starting.'
          : new TextEncoder().encode(compiled.text).length > 262144
            ? 'Your task exceeds the 262144-byte limit.'
            : draft.repositoryUrl && support.repository === undefined
              ? 'Checking repository support. Wait for the check to finish before starting.'
              : draft.repositoryUrl && support.repository === false
                ? 'Repository tasks are unavailable on this service. Clear the repository to start an empty workspace.'
                : !validRepository(draft.repositoryUrl)
                  ? 'Enter an HTTPS GitHub repository URL.'
                  : draft.repositoryRef &&
                      (!draft.repositoryUrl || !validRepositoryRef(draft.repositoryRef))
                    ? 'Enter a valid repository ref with a repository URL.'
                    : undefined;
  return { text: compiled.text, issue: compiled.issue ?? (message ? { message } : undefined) };
}
export function submitTaskOnShortcut(event: KeyboardEvent<HTMLFormElement>) {
  if (
    event.key !== 'Enter' ||
    (!event.metaKey && !event.ctrlKey) ||
    event.nativeEvent.isComposing ||
    event.nativeEvent.keyCode === 229
  )
    return;
  event.preventDefault();
  event.currentTarget.requestSubmit();
}
