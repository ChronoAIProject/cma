import {define, acquire, invoke, egRequest, peer, segment, same, digest} from './helpers.mjs';

const appPath = app => `/v1/github/apps/${segment(app.app_ref)}`;
const noSecrets = (ctx, value) => ctx.assert(!Object.keys(value).some(key =>
  ['webhook_secret', 'private_key', 'token', 'authorization'].includes(key)), 'github_no_secret_fields');

export const githubCases = [
  define(74, 'eg', ['eg'], async ctx => {
    let lease, created, updated, verified, rotated, retired;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'github_app_lifecycle');
      const body = {app_id: lease.expected.appId, display_name: 'Synthetic EG integration'};
      created = await egRequest(ctx, lease, {method: 'POST', path: '/v1/github/apps', body,
        key: lease.keys.create, allowed: [200, 201]});
      await invoke(lease, 'retainApp', {appRef: created.app_ref});
      ctx.assert(created.app_id === body.app_id && created.display_name === body.display_name
        && created.version > 0, 'github_created_exact_app');
      const replay = await egRequest(ctx, lease, {method: 'POST', path: '/v1/github/apps', body,
        key: lease.keys.create, allowed: [200, 201], replica: peer(ctx.am)});
      ctx.assert(replay.app_ref === created.app_ref && replay.version === created.version,
        'github_create_same_key_same_record');
      updated = await egRequest(ctx, lease, {method: 'PATCH', path: appPath(created),
        body: {display_name: 'Renamed synthetic EG integration', expected_version: created.version}});
      ctx.assert(updated.version > created.version && updated.display_name !== created.display_name,
        'github_changed_name_revision');
      const stale = await egRequest(ctx, lease, {method: 'PATCH', path: appPath(created), replica: peer(ctx.am),
        body: {display_name: 'must not overwrite', expected_version: created.version}, allowed: [409]});
      ctx.assert(stale.code || stale.type, 'github_stale_update_typed_conflict');
      verified = await egRequest(ctx, lease, {method: 'POST', path: appPath(created) + '/verification',
        body: {expected_version: updated.version, evidence_ref: lease.expected.verificationEvidenceRef}});
      ctx.assert(verified.verification?.evidence_ref === lease.expected.verificationEvidenceRef
        && verified.verification.verified_at > 0 && verified.status === lease.expected.verifiedStatus,
      'github_verification_explicit');
      const denied = await egRequest(ctx, lease, {path: appPath(created), actor: 'outsider', allowed: [403, 404]});
      ctx.assert(!denied.app_ref, 'github_owner_boundary');
    });
    await ctx.step('step_2', async () => {
      rotated = await egRequest(ctx, lease, {method: 'PUT', path: appPath(created) + '/webhook-secret',
        body: {expected_version: verified.version, overlap_seconds: 0}, credentialSlot: 'new_webhook_secret'});
      ctx.assert(rotated.webhook_credential?.present === true
        && rotated.webhook_credential.version > (verified.webhook_credential?.version ?? 0),
      'github_secret_rotation_revision');
      const signatures = await invoke(lease, 'probeWebhookCredentials', {appRef: created.app_ref});
      ctx.assert(signatures.newAccepted === 1 && signatures.oldAccepted === 0,
        'github_new_secret_accepted_old_refused');
      retired = await egRequest(ctx, lease, {method: 'POST', path: appPath(created) + '/retirement',
        body: {expected_version: rotated.version}});
      ctx.assert(retired.status === 'retired' && retired.retired_at > 0
        && retired.app_ref === created.app_ref, 'github_retirement_durable');
    });
    await ctx.step('expected_result', async () => {
      for (const value of [created, updated, verified, rotated, retired]) noSecrets(ctx, value);
      const read = await egRequest(ctx, lease, {path: appPath(created), replica: peer(ctx.am)});
      ctx.assert(read.status === 'retired' && read.version === retired.version, 'github_retired_peer_readback');
      const unrelated = await egRequest(ctx, lease, {path: `/v1/github/apps/${segment(lease.expected.unrelatedAppRef)}`});
      ctx.assert(digest(unrelated) === lease.expected.unrelatedAppDigest, 'github_retirement_preserves_other_app');
      await ctx.observe('github_lifecycle_revisions', async () => [created, updated, verified, rotated, retired]
        .map(app => ({version: app.version, status: app.status})));
    });
  }),
  define(75, 'eg', ['eg'], async ctx => {
    let lease, binding, verified;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'github_installation');
      binding = await egRequest(ctx, lease, {method: 'POST', path: '/v1/github/installation-bindings',
        body: {app_ref: lease.expected.appRef, installation_id: lease.expected.installationId,
          repository_ids: lease.expected.repositoryIds}, key: lease.keys.binding, allowed: [200, 201]});
      ctx.assert(binding.app_ref === lease.expected.appRef && binding.installation_id === lease.expected.installationId
        && binding.account_id === lease.expected.accountId, 'github_binding_exact_installation_owner');
      ctx.assert(same([...binding.repository_ids].sort(), [...lease.expected.repositoryIds].sort()),
        'github_binding_repository_scope');
      await invoke(lease, 'retainBinding', {bindingId: binding.binding_id});
    });
    await ctx.step('step_2', async () => {
      const path = `/v1/github/installation-bindings/${segment(binding.binding_id)}`;
      const read = await egRequest(ctx, lease, {path, replica: peer(ctx.am)});
      ctx.assert(read.binding_id === binding.binding_id && read.installation_id === binding.installation_id
        && read.authority_revision === binding.authority_revision, 'github_binding_peer_identity');
      verified = await egRequest(ctx, lease, {method: 'POST', path: path + '/verification',
        body: {expected_version: read.version, evidence_ref: lease.expected.verificationEvidenceRef,
          repository_ids: lease.expected.repositoryIds}});
      ctx.assert(verified.verification_ref === lease.expected.verificationEvidenceRef
        && verified.verified_until > lease.expected.nowSeconds && verified.status === lease.expected.verifiedStatus,
      'github_installation_verified_current');
      const denied = await egRequest(ctx, lease, {method: 'POST', path: '/v1/github/installation-bindings',
        body: {app_ref: lease.expected.appRef, installation_id: lease.expected.deniedInstallationId},
        key: lease.keys.denied, allowed: [403]});
      ctx.assert(!denied.binding_id, 'github_denied_installation_not_bound');
    });
    await ctx.step('expected_result', async () => {
      const bindings = await invoke(lease, 'installationBindings');
      ctx.assert(bindings.filter(b => b.installationId === lease.expected.installationId).length === 1
        && !bindings.some(b => b.installationId === lease.expected.deniedInstallationId), 'github_no_unauthorized_binding');
      noSecrets(ctx, verified); await ctx.observe('github_binding_receipt', async () => ({
        binding_sha256: digest(binding.binding_id), authority_revision: verified.authority_revision,
        status: verified.status, repository_count: verified.repository_ids.length}));
    });
  }),
  define(76, 'eg', ['eg'], async ctx => {
    let lease, accepted, receipt;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'github_webhook');
      accepted = await invoke(lease, 'githubWebhook', {replica: ctx.am, signature: 'valid'});
      ctx.assert(accepted.deliveryId === lease.expected.deliveryId && accepted.receiptId,
        'github_signed_delivery_retained');
      const invalid = await invoke(lease, 'githubWebhook', {replica: peer(ctx.am), signature: 'invalid'});
      ctx.assert(invalid.status === 401 || invalid.status === 403, 'github_bad_signature_refused');
      ctx.assert(!invalid.receiptId && !invalid.taskId, 'github_bad_signature_no_work');
    });
    await ctx.step('step_2', async () => {
      const lookup = await egRequest(ctx, lease, {path: `/v1/github/apps/${segment(lease.expected.appRef)}`
        + `/deliveries/${segment(accepted.deliveryId)}`, replica: peer(ctx.am)});
      ctx.assert(lookup.receipt_id === accepted.receiptId && lookup.delivery_id === accepted.deliveryId,
        'github_delivery_exact_receipt');
      receipt = await egRequest(ctx, lease, {path: `/v1/github/deliveries/${segment(accepted.receiptId)}`});
      ctx.assert(receipt.app_ref === lease.expected.appRef && receipt.body_sha256 === lease.expected.bodyDigest
        && receipt.installation_id === lease.expected.installationId, 'github_receipt_auth_scope');
      ctx.assert(receipt.task_ids.length > 0, 'github_task_created');
      for (const id of receipt.task_ids) {
        const task = await egRequest(ctx, lease, {path: `/v1/github/tasks/${segment(id)}`, replica: peer(ctx.am)});
        ctx.assert(task.task_id === id && task.channel_id === lease.channelId && task.event_id,
          'github_task_exact_work');
        const completed = await invoke(lease, 'waitForTask', {taskId: id, timeoutMs: 90000});
        ctx.assert(completed.taskId === id && completed.eventId === task.event_id
          && completed.responseId && completed.turnCount === 1 && completed.terminal === true,
        'github_task_actual_completion');
      }
      const replay = await invoke(lease, 'githubWebhook', {replica: peer(ctx.am), signature: 'valid', replay: true});
      ctx.assert(replay.deliveryId === accepted.deliveryId && replay.receiptId === accepted.receiptId,
        'github_webhook_deduplicated_receipt');
    });
    await ctx.step('expected_result', async () => {
      const final = await invoke(lease, 'drain', {timeoutMs: 90000});
      ctx.assert(final.turns === receipt.task_ids.length && final.pending === 0 && final.unknown === 0,
        'github_no_duplicate_task_execution');
      const foreign = await egRequest(ctx, lease, {path: `/v1/github/deliveries/${segment(accepted.receiptId)}`,
        actor: 'outsider', allowed: [403, 404]});
      ctx.assert(!foreign.receipt_id && !foreign.task_ids, 'github_receipt_owner_isolation');
      await ctx.observe('github_delivery_accounting', async () => ({receipt_sha256: digest(receipt.receipt_id),
        tasks: receipt.task_ids.length, turns: final.turns, pending: final.pending, unknown: final.unknown}));
    });
  }),
];
