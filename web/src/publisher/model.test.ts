import { describe, expect, it } from 'vitest';
import type { TriggerDraft, TriggerRepositorySelection } from '@chronoai/cma-sdk';
import {
  applyRepository,
  contentRequest,
  editorFromDraft,
  editorIsDirty,
  newPublisherEditor,
  parseGithubRepositoryUrl,
  validatePublisherEditor,
} from './model';

const REPOSITORY: TriggerRepositorySelection = {
  repository: {
    repository_id: 10,
    repository_node_id: 'R_upstream',
    owner_id: 20,
    owner_node_id: 'U_owner',
    canonical_owner: 'upstream-owner',
    canonical_name: 'project',
    visibility: 'public',
    allow_forking: true,
    default_branch: 'main',
    selected_ref: 'main',
  },
  source_sha: 'a'.repeat(40),
};

function serverDraft(): TriggerDraft {
  const editor = applyRepository(
    { ...newPublisherEditor('Chrono Maintainer'), selectedServiceId: 'service-exact' },
    REPOSITORY,
  );
  return {
    id: '11111111-1111-4111-8111-111111111111',
    selected_service_id: 'service-exact',
    version: 3,
    next_revision_number: 2,
    content: {
      schema_version: editor.content.schema_version,
      trigger_id: '11111111-1111-4111-8111-111111111111',
      revision_number: 2,
      publisher_subject_id: 'publisher',
      publisher_display_identity: editor.content.publisher_display_identity,
      upstream: {
        repository: REPOSITORY.repository,
        source_mode: editor.content.repository.source_mode,
        observed_source_sha: REPOSITORY.source_sha,
        contribution_base_ref: editor.content.repository.contribution_base_ref,
      },
      task: editor.content.task,
      session: editor.content.session,
      repository_access: editor.content.repository_access,
      setup: editor.content.setup,
      contribution: editor.content.contribution,
      commercial_policy: editor.content.commercial_policy,
      override_policy: editor.content.override_policy,
      public_projection: editor.content.public_projection,
    },
    created_at: '2026-08-27T00:00:00Z',
    updated_at: '2026-08-27T01:00:00Z',
  };
}

describe('GitHub repository URL parsing', () => {
  it('accepts only one canonical owner/name path', () => {
    expect(parseGithubRepositoryUrl(' https://github.com/ChronoAIProject/cma-trigger ')).toEqual({
      owner: 'ChronoAIProject',
      name: 'cma-trigger',
    });
    expect(parseGithubRepositoryUrl('https://github.com/owner/repository/')).toEqual({
      owner: 'owner',
      name: 'repository',
    });
  });

  it.each([
    'http://github.com/owner/repository',
    'https://github.example.test/owner/repository',
    'https://github.com/owner/repository/issues',
    'https://github.com/owner/repository?tab=readme',
    'https://github.com/owner/repository#readme',
    'https://user@github.com/owner/repository',
  ])('rejects a non-repository locator: %s', (value) => {
    expect(() => parseGithubRepositoryUrl(value)).toThrow();
  });
});

describe('publisher draft conversion', () => {
  it('retains the exact selected service and strips immutable fields from an update', () => {
    const draft = serverDraft();
    const editor = editorFromDraft(draft);

    expect(editor.selectedServiceId).toBe('service-exact');
    expect(editor.repositoryUrl).toBe('https://github.com/upstream-owner/project');
    expect(editor.content).toEqual(contentRequest(draft));
    expect(editorIsDirty(editor)).toBe(false);
    expect(editorIsDirty({ ...editor, selectedServiceId: 'service-other' })).toBe(true);
  });

  it('applies only canonical server repository identity', () => {
    const editor = applyRepository(newPublisherEditor('Maintainer'), REPOSITORY);
    expect(editor.repositoryUrl).toBe('https://github.com/upstream-owner/project');
    expect(editor.content.repository).toMatchObject({
      repository_id: 10,
      selected_ref: 'main',
      contribution_base_ref: 'main',
    });
  });
});

describe('publisher validation', () => {
  it('accepts the shipped MVP defaults after exact connection and repository verification', () => {
    const editor = applyRepository(
      { ...newPublisherEditor('Maintainer'), selectedServiceId: 'service-exact' },
      REPOSITORY,
    );
    expect(validatePublisherEditor(editor)).toEqual({});
  });

  it('finds unsafe templates, invalid required defaults, and inverted resources without mutation', () => {
    const base = applyRepository(
      { ...newPublisherEditor('Maintainer'), selectedServiceId: 'service-exact' },
      REPOSITORY,
    );
    const editor = {
      ...base,
      content: {
        ...base.content,
        task: {
          ...base.content.task,
          input_fields: [{ ...base.content.task.input_fields[0]!, default: undefined }],
        },
        session: {
          ...base.content.session,
          resources: {
            default: { cpu_millis: 2000, memory_mib: 1024 },
            ceiling: { cpu_millis: 1000, memory_mib: 2048 },
          },
        },
        contribution: {
          ...base.content.contribution,
          pull_request: {
            ...base.content.contribution.pull_request,
            body_template: '<!-- cma-trigger: forged -->',
          },
        },
      },
    };

    expect(validatePublisherEditor(editor)).toMatchObject({
      'task.input_fields.0.default': expect.any(String),
      resources: expect.any(String),
      prBody: expect.any(String),
    });
    expect(base.content.task.input_fields[0]?.default).toBe('the selected issue');
  });
});
