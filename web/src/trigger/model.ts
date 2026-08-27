import type {
  JsonObject,
  JsonValue,
  Launch,
  LaunchStepStatus,
  Mergeability,
  PromptDelivery,
  PromptDeliveryState,
} from '@chronoai/cma-sdk';

export function jsonObject(value: JsonValue | undefined): JsonObject | undefined {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? (value as JsonObject)
    : undefined;
}

export function jsonText(value: JsonValue | undefined): string | undefined {
  return typeof value === 'string' && value !== '' ? value : undefined;
}

export function jsonNumber(value: JsonValue | undefined): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined;
}

export function jsonBoolean(value: JsonValue | undefined): boolean | undefined {
  return typeof value === 'boolean' ? value : undefined;
}

export function jsonStrings(value: JsonValue | undefined): readonly string[] {
  return Array.isArray(value)
    ? value.filter((entry): entry is string => typeof entry === 'string')
    : [];
}

export interface TaskField {
  readonly key: string;
  readonly required: boolean;
  readonly editable: boolean;
  readonly defaultValue?: JsonValue;
  readonly kind:
    | { readonly type: 'text'; readonly minimum: number; readonly maximum: number }
    | { readonly type: 'integer'; readonly minimum: number; readonly maximum: number }
    | { readonly type: 'boolean' }
    | { readonly type: 'choice'; readonly values: readonly string[] };
}

export function taskFields(launch: Launch): readonly TaskField[] {
  const fields: TaskField[] = [];
  for (const raw of launch.revision_content.task.input_fields) {
    if (raw.kind === 'boolean') {
      fields.push({
        key: raw.key,
        required: raw.required,
        editable: raw.editable,
        defaultValue: raw.default,
        kind: { type: 'boolean' },
      });
      continue;
    }
    if ('text' in raw.kind) {
      fields.push({
        key: raw.key,
        required: raw.required,
        editable: raw.editable,
        defaultValue: raw.default,
        kind: {
          type: 'text',
          minimum: raw.kind.text.min_bytes,
          maximum: raw.kind.text.max_bytes,
        },
      });
      continue;
    }
    if ('integer' in raw.kind) {
      fields.push({
        key: raw.key,
        required: raw.required,
        editable: raw.editable,
        defaultValue: raw.default,
        kind: {
          type: 'integer',
          minimum: raw.kind.integer.minimum,
          maximum: raw.kind.integer.maximum,
        },
      });
      continue;
    }
    fields.push({
      key: raw.key,
      required: raw.required,
      editable: raw.editable,
      defaultValue: raw.default,
      kind: { type: 'choice', values: raw.kind.choice.values },
    });
  }
  return fields;
}

export function editableFields(launch: Launch): ReadonlySet<string> {
  return new Set(jsonStrings(launch.revision_content.override_policy.editable_fields));
}

export function optionalCheckChoices(launch: Launch): readonly string[] {
  return jsonStrings(launch.revision_content.override_policy.optional_checks);
}

export function upstreamRepository(launch: Launch) {
  return launch.revision_content.upstream.repository;
}

export function sessionId(launch: Launch): string | undefined {
  return launch.session === null
    ? undefined
    : jsonText(launch.session.session_id ?? launch.session.id);
}

export function workspaceCheckpoint(launch: Launch): string | undefined {
  if (launch.workspace === null) return undefined;
  const checkpoint = jsonObject(launch.workspace.latest_checkpoint);
  return checkpoint === undefined ? undefined : jsonText(checkpoint.id);
}

export function reachState(launch: Launch): string | undefined {
  return launch.reach === null
    ? undefined
    : jsonText(launch.reach.revocation_state ?? launch.reach.state);
}

export function deliveryState(launch: Launch): PromptDeliveryState | undefined {
  return launch.delivery?.state;
}

export function recoverablePromptDelivery(launch: Launch): PromptDelivery | undefined {
  const delivery = launch.delivery;
  return launch.state === 'failed_retryable' &&
    launch.blocked_reason === 'recovery_required' &&
    delivery?.state === 'recovery_required' &&
    delivery.payload_hash !== ''
    ? delivery
    : undefined;
}

const STEP_LABELS: Readonly<Record<string, string>> = {
  review: 'Review configuration',
  authentication: 'Connect GitHub',
  preflight: 'Verify repository access',
  fork: 'Fork repository',
  branch: 'Create Launch branch',
  workspace: 'Import workspace',
  sandbox: 'Allocate sandbox',
  session: 'Create CMA session',
  reach: 'Provision GitHub reach',
  initial_prompt: 'Send initial task',
  checkpoint: 'Checkpoint workspace',
  diff: 'Review contribution diff',
  publish_branch: 'Publish Launch branch',
  pull_request: 'Open or update pull request',
  cleanup: 'Release Launch capabilities',
};

export function stepLabel(key: string): string {
  return STEP_LABELS[key] ?? key.replaceAll('_', ' ');
}

export function stateLabel(value: string): string {
  return value.replaceAll('_', ' ');
}

export function statusLabel(status: LaunchStepStatus): string {
  return status;
}

export function mergeabilityLabel(value: Mergeability): 'Computing' | 'Mergeable' | 'Conflicting' {
  switch (value) {
    case 'computing':
      return 'Computing';
    case 'mergeable':
      return 'Mergeable';
    case 'conflicting':
      return 'Conflicting';
    default: {
      const exhaustive: never = value;
      return exhaustive;
    }
  }
}

export function shortRevision(value: string): string {
  return value.length <= 12 ? value : value.slice(0, 12);
}

export function timestamp(value: string | null | undefined): string {
  if (value === null || value === undefined || value === '') return 'Not yet';
  const parsed = new Date(value);
  return Number.isNaN(parsed.getTime())
    ? value
    : new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' }).format(
        parsed,
      );
}

export function isTerminalLaunch(launch: Launch): boolean {
  return (
    launch.state === 'completed' ||
    launch.state === 'cancelled' ||
    launch.state === 'failed_terminal'
  );
}
