import assert from 'node:assert/strict';
import type { ContractCase } from './context.js';
import { browserOperations } from './navigation.js';

export const localCodes = new Map([
  ['admin-runtime-migration-malformed', 'invalid_runtime_migration'],
  ['retained-runtime-policy-invalid', 'runtime_canary_invalid'],
  ['retained-runtime-preview-invalid', 'runtime_canary_invalid'],
  ['retained-runtime-grant-invalid', 'runtime_canary_invalid'],
  ['retained-runtime-redeem-invalid', 'runtime_canary_invalid'],
  ['retained-runtime-abort-invalid', 'runtime_canary_invalid'],
  ['bound-launch-missing', 'command_subject_condition_invalid'],
  ['revocation-post-invalid_revision', 'invalid_share_command'],
  ['task-template-missing-field', 'task_template_field_required'],
  ['task-template-unknown-id', 'task_template_id_unknown'],
  ['task-template-unknown-version', 'task_template_version_unsupported'],
  ['task-template-unknown-field', 'task_template_field_unknown'],
  ['spend-observe-unknown-field', 'invalid_request'],
  ['spend-observe-unsupported-source', 'invalid_request'],
  ['spend-observe-adjustment-source', 'invalid_request'],
  ['spend-observe-fractional-units', 'invalid_request'],
  ['spend-observe-negative-units', 'invalid_request'],
  ['spend-observe-unsafe-units', 'invalid_request'],
  ['spend-observe-null-units', 'invalid_request'],
  ['spend-observe-missing-field', 'invalid_request'],
  ['spend-observe-malformed', 'invalid_request'],
  ['spend-observe-malformed-writes_closed', 'invalid_request'],
  ['spend-observe-malformed-absent', 'platform_spend_unavailable'],
  ['spend-observe-malformed-absent_forbidden', 'forbidden'],
  ['spend-observe-malformed-forbidden', 'forbidden'],
  ['spend-observe-malformed-anonymous', 'not_authenticated'],
  ['admin-admission-ceilings-invalid', 'invalid_request'],
  ['admin-pending-homes-invalid', 'invalid_request'],
  ['admin-admission-stop-invalid', 'invalid_request'],
  ['admin-admission-resume-invalid', 'invalid_request'],
  ['deliverables-owner-invalid-revision', 'invalid_deliverable_reference'],
  ['deliverables-owner-invalid-answer-list', 'invalid_deliverable_reference'],
  ['deliverables-owner-invalid-answer-read', 'invalid_deliverable_reference'],
  ['suggestions-invalid_query', 'invalid_instance_query'],
  ['feedback-reasons-invalid-comment', 'invalid_agent_request'],
  ['feedback-reasons-invalid-message', 'invalid_agent_request'],
  ['feedback-reasons-unknown-reason', 'invalid_json'],
  ['usage-sandboxes-zero-limit', 'invalid_query'],
  ['usage-admin-sandboxes-zero-limit', 'invalid_query'],

  ['preparation-reserve-missing-key', 'invalid_binding'],
  ['binding-profile-transfer-invalid', 'invalid_binding'],
  ['document-source-invalid', 'invalid_document_request'],
  ['filesystem-content-reject-invalid-max-bytes', 'invalid_file_request'],
  ['filesystem-write-reject-malformed-json', 'invalid_file_request'],
  ['admin-runtime-update-invalid-json', 'invalid_runtime_update'],
  ['interruption-v2-invalid-revision', 'invalid_control_observation'],
  ['interruption-v2-empty-text', 'invalid_interrupt_request'],
  ['interruption-v2-whitespace-text', 'invalid_interrupt_request'],
  ['interruption-v2-null-text', 'invalid_interrupt_request'],
  ['interruption-v2-unknown-field', 'invalid_interrupt_request'],
  ['attachment-invalid-name', 'invalid_attachment'],
  ['agent-skills-install-invalid-id', 'invalid_agent_request'],
  ['agent-skills-install-invalid-version', 'invalid_agent_request'],
  ['agent-skills-install-unknown-field', 'invalid_json'],
  ['settings-state-agent-invalid-input', 'invalid_agent_request'],
  ['preferences-state-agent-invalid-input', 'invalid_agent_request'],
  ['gateway-connection-list-invalid-query', 'invalid_query'],
  ['gateway-connection-delete-invalid-query', 'invalid_query'],
  ['queued-cancellation-v2-invalid-revision', 'invalid_queued_cancellation_request'],
  ['queued-cancellation-v2-unknown-field', 'invalid_queued_cancellation_request'],
  ['queued-input-v2-invalid-revision', 'invalid_queued_input_request'],
  ['queued-input-v2-unknown-field', 'invalid_queued_input_request'],
  ['queued-input-v2-empty-text', 'invalid_queued_input_request'],
  ['queued-input-v2-whitespace-text', 'invalid_queued_input_request'],
  ['queued-input-v2-null-text', 'invalid_queued_input_request'],
  ['agent-events-v2-snapshot-invalid', 'invalid_agent_cursor'],
  ['agent-events-v2-events-invalid', 'invalid_agent_cursor'],
  ['response-controls-v2-unknown-body', 'invalid_control_request'],
  ['response-controls-v2-invalid-observation', 'invalid_control_observation'],
  ['channel-inventory-invalid-active', 'invalid_instance_query'],
  ['channel-inventory-invalid-history', 'invalid_instance_query'],
  ['provider-associations-zero-limit', 'invalid_query'],
  ['provider-associations-large-limit', 'invalid_query'],
  ['channel-usage-zero-limit', 'invalid_query'],
  ['channel-usage-large-limit', 'invalid_query'],
  ['channel-usage-invalid-cursor', 'invalid_query'],
  ['artifact-invalid-name', 'invalid_artifact_request'],
  ['artifact-invalid-media_type', 'invalid_artifact_request'],
  ['artifact-invalid-content_base64', 'invalid_artifact_request'],
  ['artifact-zero-limit', 'invalid_artifact_request'],
  ['artifact-large-limit', 'invalid_artifact_request'],
  ['artifact-invalid-cursor', 'invalid_artifact_request'],
  ['shared-sandbox-inventory-mixed-scope', 'invalid_query'],
  ['shared-sandbox-inventory-invalid-shared', 'invalid_query'],
  ['sandbox-command-invalid-revision', 'invalid_sandbox_command'],
  ['creation-inventory-invalid-category', 'invalid_instance_query'],
  ['responses-v2-field-stream', 'stream_required'],
  ['responses-v2-field-previous_response_id', 'conversation_previous_response_conflict'],
  ['responses-v2-field-tools', 'responses_field_unsupported'],
  ['responses-v2-field-store', 'store_required'],
  ['responses-v2-field-metadata', 'invalid_metadata'],
  ['responses-v2-field-input', 'invalid_response_input'],
  ['responses-v2-field-conversation', 'invalid_conversation'],
  ['responses-v2-alias-mismatch', 'conversation_alias_mismatch'],
  ['responses-v2-items-limit', 'invalid_items_limit'],
  ['responses-v2-items-order', 'invalid_items_order'],
  ['sandbox-inventory-status-invalid', 'invalid_query'],
  ['authoring-skills-limit-zero', 'invalid_profile_authoring_query'],
  ['authoring-skills-limit-overflow', 'invalid_profile_authoring_query'],
  ['authoring-skills-limit-too-large', 'invalid_profile_authoring_query'],
  ['authoring-skills-limit-text', 'invalid_profile_authoring_query'],
  ['authoring-skills-query-too-long', 'invalid_profile_authoring_query'],
  ['authoring-skills-query-control', 'invalid_profile_authoring_query'],
  ['authoring-skills-query-byte-limit', 'invalid_profile_authoring_query'],
  ['authoring-skills-cursor-empty', 'invalid_profile_authoring_query'],
  ['authoring-skills-cursor-too-long', 'invalid_profile_authoring_query'],
  ['authoring-skills-cursor-control', 'invalid_profile_authoring_query'],
  ['authoring-skills-forbidden-before-query', 'forbidden'],
  ['authoring-skills-anonymous-before-query', 'not_authenticated'],
  ['authoring-resolve-invalid-body-type', 'invalid_json'],
  ['authoring-resolve-forbidden-before-body', 'forbidden'],
  ['authoring-resolve-anonymous-before-body', 'not_authenticated'],
  ['sandbox-invalid-query', 'invalid_query'],
  ['admin-invalid-query', 'invalid_admin_request'],
  ['admin-settings-invalid', 'invalid_request'],
  ['profile-invalid-content', 'profile_invalid'],
  ['responses-items-bad-limit', 'responses_field_unsupported'],
  ['responses-instructions-null', 'responses_field_unsupported'],
  ['responses-tools-null', 'responses_field_unsupported'],
  ['responses-stream-false', 'responses_field_unsupported'],
  ['responses-store-false', 'responses_field_unsupported'],
  ['responses-stateless', 'responses_field_unsupported'],
  ['responses-metadata-values', 'responses_field_unsupported'],
  ['responses-input-role', 'responses_field_unsupported'],
  ['responses-conversation-invalid-body', 'invalid_json'],
  ['responses-missing-content-type', 'invalid_json'],
  ['agui-missing-content-type', 'invalid_json'],
  ['triggers-badge-invalid-query', 'invalid-badge-query'],
  ['state-sandbox-create-invalid', 'profile_invalid'],
  ['state-sandbox-update-invalid', 'profile_invalid'],
  ['trigger-error-list-malformed-query', 'invalid_trigger_page'],
  ['launch-error-create-missing-start-choice', 'invalid_json'],
  ['launch-error-list-invalid-state', 'invalid_launch_cursor'],
  ['launch-error-list-malformed-limit', 'invalid_launch_cursor'],
  ['launch-error-events-malformed-after', 'invalid_launch_event_query'],
  ['launch-error-stream-malformed-follow', 'invalid_launch_event_query'],
  ['launch-error-stream-invalid-limit', 'invalid_launch_event_query'],
]);

const protectedQueries = new Map([
  ['usage-admin-summary-unknown-query', ['unknown']],
  ['filesystem-tree-reject-unknown-query', ['owner_access']],
  ['filesystem-content-reject-unknown-query', ['owner_access']],
  ['filesystem-download-reject-unknown-query', ['owner_access']],
  ['filesystem-delete-reject-unknown-query', ['owner_access']],
  ['interruption-v2-query', ['unexpected']],
  ['queued-cancellation-v2-query-observe', ['unexpected']],
  ['queued-cancellation-v2-query-submit', ['unexpected']],
  ['queued-input-v2-query-list', ['unexpected']],
  ['queued-input-v2-query-submit', ['unexpected']],
  ['agent-events-v2-head-query', ['unknown']],
  ['agent-events-v2-snapshot-query', ['unknown']],
  ['agent-events-v2-events-query', ['unknown']],
  ['agent-events-v2-snapshot-duplicate', ['page_cursor']],
  ['agent-events-v2-events-duplicate', ['after_cursor']],
  ['response-controls-v2-observe-query', ['unexpected']],
  ['response-controls-v2-status-query', ['unexpected']],
  ['creation-inventory-invalid-creator', ['created_by']],
  ['responses-v2-retrieve-query', ['stream']],
  ['sandbox-inventory-invalid-owner', ['owner']],
  ['agui-invalid-replay-query', ['unexpected']],
  ['responses-items-bad-query', ['unknown']],
  ['responses-read-bad-query', ['unknown']],
  ['responses-read-stream-refused', ['stream']],
  ['triggers-embed-invalid-query', ['extra']],
  ['launch-error-stream-invalid-query', ['unknown']],
  ['org-inventory-invalid-authority-injection', ['owner']],
]);

export function expectation(
  fixture: ContractCase,
): 'wire' | 'preflight-validation' | 'serializer-protection' | 'browser-only' | 'raw-wire-only' {
  if (fixture.id === 'desktop-capability-owner-400') {
    assert.equal(fixture.operation, 'GET /api/v1/desktop-capability');
    assert.equal(problemBody(fixture).code, 'invalid_instance_query');
    assert(
      new URL(fixture.request.path, 'https://contract.example').searchParams.has('unexpected'),
    );
    return 'raw-wire-only';
  }
  if (
    [
      'binding-media-missing-length',
      'binding-media-missing-type',
      'binding-media-encoding',
      'binding-media-too-large',
      'binding-media-incomplete-body',
    ].includes(fixture.id)
  ) {
    const headers = fixture.request.headers;
    const invalid =
      fixture.id === 'binding-media-missing-length'
        ? headers['content-length'] === undefined
        : fixture.id === 'binding-media-missing-type'
          ? headers['content-type'] === undefined
          : fixture.id === 'binding-media-encoding'
            ? headers['content-encoding'] === 'gzip'
            : fixture.id === 'binding-media-too-large'
              ? Number(headers['content-length']) > 20 * 1024 * 1024
              : Number(headers['content-length']) !== Buffer.byteLength(fixture.request.rawBody!);
    assert(
      invalid,
      'typed Blob upload always derives valid content length/type and sends unencoded bytes',
    );
    return 'raw-wire-only';
  }
  if (
    fixture.request.method === 'POST' &&
    fixture.operation.endsWith('/media/{attachment_index}/attachment')
  )
    return 'wire';
  if (fixture.id === 'filesystem-delete-reject-invalid-recursive') {
    assert.equal(problemBody(fixture).code, 'invalid_file_request');
    assert.equal(
      new URL(fixture.request.path, 'https://contract.example').searchParams.get('recursive'),
      'maybe',
    );
    return 'raw-wire-only';
  }
  if (browserOperations.has(fixture.operation)) return 'browser-only';
  const body = problemBody(fixture);
  if (fixture.id === 'agent-access-deny-invalid-json') {
    assert.equal(fixture.operation, 'POST /api/v1/agents/{id}/access-requests/{request_id}/deny');
    assert.equal(body.code, 'invalid_json');
    assert.throws(() => JSON.parse(fixture.request.rawBody!));
    // Denial exposes no body argument and always serializes the empty object.
    return 'raw-wire-only';
  }
  if (['authoring-skills-unknown-query', 'authoring-skills-duplicate-query'].includes(fixture.id)) {
    assert.equal(fixture.operation, 'GET /api/v1/profile-authoring/skills');
    assert.equal(body.code, 'invalid_profile_authoring_query');
    const query = new URL(fixture.request.path, 'https://contract.example').searchParams;
    if (fixture.id === 'authoring-skills-unknown-query') assert(query.has('unknown'));
    else assert.equal(query.getAll('q').length, 2);
    return 'raw-wire-only';
  }
  if (
    ['authoring-resolve-missing-content-type', 'authoring-resolve-wrong-content-type'].includes(
      fixture.id,
    )
  ) {
    assert.equal(fixture.operation, 'POST /api/v1/profile-authoring/repositories/resolve');
    assert.equal(body.code, 'invalid_json');
    assert.equal(fixture.response.status, 415);
    assert.equal(
      fixture.request.headers['content-type'],
      fixture.id === 'authoring-resolve-missing-content-type' ? undefined : 'text/plain',
    );
    return 'raw-wire-only';
  }
  if (
    fixture.request.rawBody !== undefined &&
    fixture.operation !== 'POST /api/v1/agents/{id}/attachments'
  ) {
    if (fixture.request.rawBodyRepeat !== undefined && fixture.response.status === 413) {
      assert.equal(
        body.code,
        [
          'authoring-resolve-body-too-large',
          'sandbox-service-access-oversize',
          'agent-skills-install-oversized-body',
        ].includes(fixture.id)
          ? 'invalid_json'
          : 'request_too_large',
      );
      assert(Buffer.byteLength(fixture.request.rawBody, 'utf8') > 2_097_152);
      return 'raw-wire-only';
    }
    assert.throws(() => JSON.parse(fixture.request.rawBody!));
    assert.equal(
      body.code,
      localCodes.get(fixture.id) ??
        (fixture.operation.includes('/api/v1/admin/') ? 'malformed_json' : 'invalid_json'),
    );
    return 'preflight-validation';
  }
  if (fixture.id === 'trigger-state-version-overflow') {
    assert.equal(fixture.operation, 'PUT /api/v1/triggers/{id}');
    assert.equal(body.code, 'trigger_version_overflow');
    const version = (fixture.request.body as { draft_version: unknown }).draft_version;
    // The Rust fixture's u64 cannot cross JavaScript's number boundary exactly.
    assert(typeof version === 'number' && Number.isInteger(version));
    assert(version > Number.MAX_SAFE_INTEGER, 'Expected an unsafe integer fixture');
    return 'preflight-validation';
  }
  if (localCodes.has(fixture.id)) {
    assert.equal(body.code, localCodes.get(fixture.id));
    return 'preflight-validation';
  }
  if (fixture.id.endsWith('-missing-key') || fixture.id.endsWith('-invalid-key')) {
    assert(['missing_idempotency_key', 'invalid_idempotency_key'].includes(String(body.code)));
    assert(!/^[\x21-\x7e]{1,255}$/.test(fixture.request.headers['idempotency-key'] ?? ''));
    return 'preflight-validation';
  }
  if (
    /-invalid-body(?:-reused-key)?$/.test(fixture.id) &&
    (Array.isArray(fixture.request.body) || typeof fixture.request.body === 'string')
  ) {
    assert(
      [
        'invalid_request',
        'invalid_json',
        'agui_invalid_run',
        'responses_field_unsupported',
        'idempotency_key_reused',
      ].includes(String(body.code)),
    );
    return 'preflight-validation';
  }
  if (fixture.id.startsWith('admin-') && fixture.id.endsWith('-invalid-page')) {
    assert.equal(body.code, 'invalid_admin_request');
    assert.equal(
      new URL(fixture.request.path, 'https://contract.example').searchParams.get('limit'),
      '0',
    );
    return 'preflight-validation';
  }
  if (fixture.id.endsWith('-missing-media-type') && fixture.request.body !== undefined) {
    assert(['unsupported_media_type', 'json_content_type_required'].includes(String(body.code)));
    assert(!Object.hasOwn(fixture.request.headers, 'content-type'));
    return 'serializer-protection';
  }
  if (protectedQueries.has(fixture.id)) {
    const url = new URL(fixture.request.path, 'https://contract.example');
    if (fixture.id.startsWith('agent-events-v2-') && fixture.id.endsWith('-duplicate')) {
      const key = protectedQueries.get(fixture.id)![0]!;
      assert.equal(url.searchParams.getAll(key).length, 2);
    }
    for (const key of protectedQueries.get(fixture.id)!) assert(url.searchParams.has(key));
    assert(fixture.response.status >= 400);
    return 'serializer-protection';
  }
  if (fixture.operation === 'GET /auth/signup-options' && fixture.request.headers.cookie)
    return 'serializer-protection';
  if (fixture.id === 'responses-conversation-content-type') {
    assert.equal(fixture.request.body, undefined);
    assert.equal(body.code, 'invalid_json');
    return 'serializer-protection';
  }
  return 'wire';
}

export function expectedRequestUrl(fixture: ContractCase): URL {
  const url = new URL(fixture.request.path, 'https://contract.example');
  if (fixture.operation.includes('/answers/{answer_id}/deliverables')) {
    url.pathname = url.pathname.replace('/answers/event:', '/answers/event%3A');
  }
  for (const key of protectedQueries.get(fixture.id) ?? []) url.searchParams.delete(key);
  if (fixture.operation === 'GET /api/v1/agui/events') {
    if (!url.searchParams.has('starting_after')) url.searchParams.set('starting_after', '0');
    if (!url.searchParams.has('follow')) url.searchParams.set('follow', 'true');
  }
  if (fixture.operation === 'GET /api/v1/agui/launches/{id}/events') {
    if (!url.searchParams.has('starting_after')) url.searchParams.set('starting_after', '0');
  }
  return url;
}

export function expectedProjection(fixture: ContractCase): unknown {
  if (fixture.id.startsWith('revocation-success-')) {
    const body = fixture.response.body as Record<string, unknown> & {
      revocation: { operation_id: string };
    };
    return { ...body, revocation: { operation: body.revocation.operation_id } };
  }
  if (fixture.operation === 'GET /api/v1/triggers/public/{public_id}/badge.svg') {
    return {
      status: fixture.response.status,
      etag: fixture.response.headers.etag,
      ...(fixture.response.status === 200 ? { svg: fixture.response.body } : {}),
    };
  }
  if (fixture.response.bodyKind === 'empty') return undefined;
  if (fixture.operation === 'GET /auth/session' && fixture.response.status === 401) {
    // The wire also advertises silent sign-in for the web client's own
    // browser flow; the SDK's signed-out projection deliberately omits it.
    const { legal_terms, ...body } = fixture.response.body as Record<string, unknown>;
    assert.deepStrictEqual(body, {
      authenticated: false,
      silent_sign_in:
        fixture.scenario !== 'auth_anonymous_disabled' &&
        !['auth-silent-suppressed', 'auth-silent-later-logout'].includes(fixture.id),
    });
    return { authenticated: false, ...(legal_terms === undefined ? {} : { legal_terms }) };
  }
  if (fixture.response.bodyKind === 'sse')
    return (fixture.response.body as { id?: string; data: Record<string, unknown> }[]).map(
      (frame) => ({
        ...frame.data,
        ...(fixture.operation.startsWith('GET /api/v1/agui/events') ||
        fixture.operation.endsWith('/agui/run')
          ? frame.id === undefined
            ? {}
            : { cursor: Number(frame.id) }
          : {}),
      }),
    );
  if (['GET /api/v1/triggers', 'GET /api/v1/launches'].includes(fixture.operation)) {
    const { next_cursor, ...body } = fixture.response.body as Record<string, unknown>;
    return { ...body, ...(next_cursor === undefined ? {} : { next: next_cursor }) };
  }
  if (
    ['GET /api/v1/admin/sandboxes/{id}', 'POST /api/v1/admin/sandboxes/{id}/retire'].includes(
      fixture.operation,
    )
  ) {
    const body = fixture.response.body as Record<string, unknown>;
    const detail = fixture.operation.startsWith('GET');
    const sandbox = (detail ? body.sandbox : body) as Record<string, unknown>;
    // The established SDK view intentionally excludes internal credential/checkpoint/worker records.
    const projected = Object.fromEntries(
      [
        'id',
        'owner',
        'name',
        'status',
        'limits',
        'failure',
        'last_activity_at',
        'created_at',
        'updated_at',
        'retired_at',
        'revision',
      ]
        .filter((key) => Object.hasOwn(sandbox, key))
        .map((key) => [key, sandbox[key]]),
    );
    const profile = sandbox.profile as Record<string, unknown>;
    projected.profile = { id: profile.id, revision: profile.revision, digest: profile.digest };
    if (sandbox.runtime)
      projected.runtime = {
        runtime_version: (sandbox.runtime as { runtime_version: string }).runtime_version,
      };
    return detail ? { ...body, sandbox: projected } : projected;
  }
  return fixture.response.body;
}

/** Both public error envelopes retain their exact machine code and extensions. */
export function problemBody(fixture: ContractCase): Record<string, unknown> {
  const body = fixture.response.body as Record<string, unknown>;
  if (fixture.response.status >= 400 && typeof body?.error === 'string')
    return { code: body.error };
  return fixture.response.status >= 400 && body?.error !== undefined
    ? (body.error as Record<string, unknown>)
    : body;
}
export function problemExtensions(fixture: ContractCase): Record<string, unknown> {
  const body = problemBody(fixture);
  if (typeof (fixture.response.body as Record<string, unknown>)?.error === 'string') return {};
  if ((fixture.response.body as Record<string, unknown>).error !== undefined)
    return { type: body.type, param: body.param };
  return Object.fromEntries(
    Object.entries(body).filter(
      ([key]) => !['type', 'title', 'status', 'detail', 'instance', 'code'].includes(key),
    ),
  );
}
