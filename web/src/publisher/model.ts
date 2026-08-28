import type {
  JsonValue,
  RepositorySelectionDraft,
  TriggerContentDraft,
  TriggerDraft,
  TriggerEditableField,
  TriggerInputFieldDefinition,
  TriggerRepositoryOperationClass,
  TriggerRepositorySelection,
  TriggerRequiredCapability,
} from '@chronoai/cma-sdk';

export const REQUIRED_CAPABILITIES: readonly TriggerRequiredCapability[] = [
  'conversation',
  'durable_conversation',
  'workspace_checkpoint',
  'restricted_service_gateway',
  'credential_refresh',
  'credential_revocation',
];

export const REPOSITORY_OPERATIONS: readonly TriggerRepositoryOperationClass[] = [
  'repository_read',
  'tree_read',
  'object_create',
  'launch_ref_create',
  'launch_ref_update',
  'status_read',
];

export const EDITABLE_FIELDS: readonly TriggerEditableField[] = [
  'task_input',
  'work_label',
  'resource_size',
  'timeout',
];

export interface GithubRepositoryLocator {
  readonly owner: string;
  readonly name: string;
}

export interface PublisherEditor {
  readonly server: TriggerDraft | null;
  readonly selectedServiceId: string;
  readonly repositoryUrl: string;
  readonly repository: TriggerRepositorySelection | null;
  readonly content: TriggerContentDraft;
}

export type PublisherValidation = Readonly<Record<string, string>>;

const encoder = new TextEncoder();

function bytes(value: string): number {
  return encoder.encode(value).byteLength;
}

function repositoryDraft(repository: TriggerRepositorySelection | null): RepositorySelectionDraft {
  return {
    repository_id: repository?.repository.repository_id ?? 0,
    selected_ref: repository?.repository.default_branch ?? 'main',
    contribution_base_ref: repository?.repository.default_branch ?? 'main',
    source_mode: 'pinned_commit',
  };
}

export function newPublisherEditor(displayIdentity: string): PublisherEditor {
  return {
    server: null,
    selectedServiceId: '',
    repositoryUrl: '',
    repository: null,
    content: {
      schema_version: 1,
      publisher_display_identity: displayIdentity.trim() || 'CMA publisher',
      repository: repositoryDraft(null),
      task: {
        public_summary: 'Fix one bounded issue',
        prompt_template: 'Work on {{topic}}',
        renderer_version: 1,
        input_fields: [
          {
            key: 'topic',
            kind: { text: { min_bytes: 1, max_bytes: 100 } },
            required: true,
            default: 'the selected issue',
            editable: true,
          },
        ],
      },
      session: {
        required_capabilities: REQUIRED_CAPABILITIES,
        permission_ceiling: 'autonomous',
        sandbox_profile: 'cma-trigger-v1',
        resources: {
          default: { cpu_millis: 500, memory_mib: 1024 },
          ceiling: { cpu_millis: 1000, memory_mib: 2048 },
        },
        timeout_default_seconds: 3600,
        timeout_ceiling_seconds: 7200,
      },
      repository_access: {
        credential_route: 'contributor_connection',
        required_scope_classes: ['public_repository'],
        allowed_operation_classes: ['repository_read'],
        workflow_change_policy: 'refuse',
      },
      setup: { executor_policy: 'disabled', steps: [] },
      contribution: {
        branch_prefix: 'cma/',
        commit_attribution: 'contributor_no_reply',
        pull_request: {
          title_template: 'CMA contribution',
          body_template: 'Created by CMA',
        },
      },
      commercial_policy: {
        billing_mode: 'zero_cost',
        policy_reference: 'public-zero-cost-v1',
      },
      override_policy: {
        editable_fields: ['task_input'],
        optional_checks: [],
      },
      public_projection: { version: 1 },
    },
  };
}

export function editorFromDraft(draft: TriggerDraft): PublisherEditor {
  const { content } = draft;
  const repository: TriggerRepositorySelection = {
    repository: content.upstream.repository,
    source_sha: content.upstream.observed_source_sha ?? '',
  };
  return {
    server: draft,
    selectedServiceId: draft.selected_service_id,
    repositoryUrl: canonicalRepositoryUrl(repository),
    repository,
    content: contentRequest(draft),
  };
}

export function contentRequest(draft: TriggerDraft): TriggerContentDraft {
  const { content } = draft;
  return {
    schema_version: content.schema_version,
    publisher_display_identity: content.publisher_display_identity,
    repository: {
      repository_id: content.upstream.repository.repository_id,
      selected_ref: content.upstream.repository.selected_ref,
      contribution_base_ref: content.upstream.contribution_base_ref,
      source_mode: content.upstream.source_mode,
    },
    task: content.task,
    session: content.session,
    repository_access: content.repository_access,
    setup: content.setup,
    contribution: content.contribution,
    commercial_policy: content.commercial_policy,
    override_policy: content.override_policy,
    public_projection: content.public_projection,
  };
}

export function canonicalRepositoryUrl(selection: TriggerRepositorySelection): string {
  const { canonical_owner: owner, canonical_name: name } = selection.repository;
  return `https://github.com/${owner}/${name}`;
}

export function parseGithubRepositoryUrl(value: string): GithubRepositoryLocator {
  let url: URL;
  try {
    url = new URL(value.trim());
  } catch (cause) {
    throw new Error('Enter a complete GitHub repository URL.', { cause });
  }
  const parts = url.pathname.split('/').filter(Boolean);
  if (
    url.protocol !== 'https:' ||
    url.hostname !== 'github.com' ||
    url.port !== '' ||
    url.username !== '' ||
    url.password !== '' ||
    url.search !== '' ||
    url.hash !== '' ||
    parts.length !== 2
  ) {
    throw new Error('Use https://github.com/owner/repository with no extra path or query.');
  }
  const owner = parts[0] ?? '';
  const name = parts[1] ?? '';
  const segment = /^[A-Za-z0-9_.-]{1,100}$/;
  if (!segment.test(owner) || !segment.test(name) || owner === '.' || name === '.') {
    throw new Error('The GitHub owner or repository name is not valid.');
  }
  return { owner, name };
}

export function applyRepository(
  editor: PublisherEditor,
  selection: TriggerRepositorySelection,
): PublisherEditor {
  const branch = selection.repository.default_branch;
  return {
    ...editor,
    repositoryUrl: canonicalRepositoryUrl(selection),
    repository: selection,
    content: {
      ...editor.content,
      repository: {
        ...editor.content.repository,
        repository_id: selection.repository.repository_id,
        selected_ref: branch,
        contribution_base_ref: branch,
      },
    },
  };
}

export function fieldKindName(field: TriggerInputFieldDefinition): string {
  if (field.kind === 'boolean') return 'boolean';
  if ('text' in field.kind) return 'text';
  if ('integer' in field.kind) return 'integer';
  return 'choice';
}

export function defaultForKind(kind: string): JsonValue {
  if (kind === 'integer') return 0;
  if (kind === 'boolean') return false;
  return '';
}

function validateInputField(
  field: TriggerInputFieldDefinition,
  index: number,
  errors: Record<string, string>,
): void {
  const path = `task.input_fields.${String(index)}`;
  if (!/^[A-Za-z0-9_]{1,64}$/.test(field.key)) {
    errors[`${path}.key`] = 'Use 1-64 ASCII letters, numbers, or underscores.';
  }
  if (field.required && field.default === undefined) {
    errors[`${path}.default`] = 'A required input needs a default value.';
  }
  if (field.kind === 'boolean') {
    if (field.default !== undefined && typeof field.default !== 'boolean') {
      errors[`${path}.default`] = 'Use a boolean default.';
    }
    return;
  }
  if ('text' in field.kind) {
    const { min_bytes: min, max_bytes: max } = field.kind.text;
    if (!Number.isInteger(min) || !Number.isInteger(max) || min < 0 || min > max || max > 8192) {
      errors[`${path}.bounds`] = 'Text bounds must be ordered between 0 and 8,192 bytes.';
    }
    if (
      field.default !== undefined &&
      (typeof field.default !== 'string' ||
        bytes(field.default) < min ||
        bytes(field.default) > max)
    ) {
      errors[`${path}.default`] = 'The default must fit the declared text bounds.';
    }
    return;
  }
  if ('integer' in field.kind) {
    const { minimum, maximum } = field.kind.integer;
    if (!Number.isSafeInteger(minimum) || !Number.isSafeInteger(maximum) || minimum > maximum) {
      errors[`${path}.bounds`] = 'Integer bounds must be safe whole numbers in ascending order.';
    }
    if (
      field.default !== undefined &&
      (typeof field.default !== 'number' ||
        !Number.isSafeInteger(field.default) ||
        field.default < minimum ||
        field.default > maximum)
    ) {
      errors[`${path}.default`] = 'The default must be an integer inside the declared bounds.';
    }
    return;
  }
  const values = field.kind.choice.values;
  if (
    values.length === 0 ||
    values.length > 64 ||
    new Set(values).size !== values.length ||
    values.some((value) => value === '' || bytes(value) > 256)
  ) {
    errors[`${path}.choices`] = 'Declare 1-64 unique, non-empty choices of at most 256 bytes.';
  }
  if (
    field.default !== undefined &&
    (typeof field.default !== 'string' || !values.includes(field.default))
  ) {
    errors[`${path}.default`] = 'The default must be one of the declared choices.';
  }
}

function positiveInteger(value: number): boolean {
  return Number.isSafeInteger(value) && value > 0;
}

export function validatePublisherEditor(editor: PublisherEditor): PublisherValidation {
  const errors: Record<string, string> = {};
  const { content } = editor;
  if (editor.selectedServiceId === '') errors.connection = 'Select an exact GitHub connection.';
  if (editor.repository === null || content.repository.repository_id === 0) {
    errors.repository = 'Verify a public, forkable GitHub repository.';
  } else if (
    editor.repository.repository.visibility !== 'public' ||
    !editor.repository.repository.allow_forking
  ) {
    errors.repository = 'The MVP requires a public repository that permits forking.';
  }
  if (content.publisher_display_identity.trim() === '') {
    errors.publisher = 'Publisher display identity is required.';
  }
  if (content.task.public_summary.trim() === '' || bytes(content.task.public_summary) > 2_000) {
    errors.summary = 'Use a non-empty public summary of at most 2,000 bytes.';
  }
  if (
    content.task.prompt_template.trim() === '' ||
    bytes(content.task.prompt_template) > 32 * 1024
  ) {
    errors.prompt = 'Use a non-empty prompt template of at most 32 KiB.';
  }
  if (content.task.input_fields.length > 32) {
    errors.inputs = 'At most 32 task inputs may be declared.';
  }
  const keys = new Set<string>();
  content.task.input_fields.forEach((field, index) => {
    validateInputField(field, index, errors);
    if (keys.has(field.key)) errors.inputs = `${field.key} is declared more than once.`;
    keys.add(field.key);
  });

  const resource = content.session.resources;
  if (
    !positiveInteger(resource.default.cpu_millis) ||
    !positiveInteger(resource.default.memory_mib) ||
    !positiveInteger(resource.ceiling.cpu_millis) ||
    !positiveInteger(resource.ceiling.memory_mib) ||
    resource.default.cpu_millis > resource.ceiling.cpu_millis ||
    resource.default.memory_mib > resource.ceiling.memory_mib
  ) {
    errors.resources = 'Resource defaults must be positive and fit within their ceilings.';
  }
  if (
    !positiveInteger(content.session.timeout_default_seconds) ||
    !positiveInteger(content.session.timeout_ceiling_seconds) ||
    content.session.timeout_default_seconds > content.session.timeout_ceiling_seconds ||
    content.session.timeout_ceiling_seconds > 86_400
  ) {
    errors.timeout = 'Timeouts must be ordered positive seconds with a one-day ceiling.';
  }
  if (
    content.session.sandbox_profile.trim() === '' ||
    bytes(content.session.sandbox_profile) > 128
  ) {
    errors.sandbox = 'Name a sandbox profile using at most 128 bytes.';
  }
  if (content.repository.selected_ref.trim() === '') errors.selectedRef = 'Select a source branch.';
  if (content.repository.contribution_base_ref.trim() === '') {
    errors.baseRef = 'Select a contribution base branch.';
  }
  if (content.repository_access.allowed_operation_classes.length === 0) {
    errors.operations = 'Allow at least repository read access.';
  }
  if (
    content.contribution.pull_request.title_template.trim() === '' ||
    bytes(content.contribution.pull_request.title_template) > 32 * 1024
  ) {
    errors.prTitle = 'Use a non-empty pull-request title of at most 32 KiB.';
  }
  const body = content.contribution.pull_request.body_template;
  if (body.trim() === '' || bytes(body) > 32 * 1024 || body.includes('<!-- cma-trigger:')) {
    errors.prBody = 'Use a non-empty body under 32 KiB without the reserved CMA marker.';
  }
  return errors;
}

export function firstValidationMessage(validation: PublisherValidation): string | undefined {
  return Object.values(validation)[0];
}

export function editorIsDirty(editor: PublisherEditor): boolean {
  if (editor.server === null) return true;
  return (
    editor.selectedServiceId !== editor.server.selected_service_id ||
    JSON.stringify(editor.content) !== JSON.stringify(contentRequest(editor.server))
  );
}
