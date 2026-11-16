import {randomUUID} from 'node:crypto';
import {requireResourceInputs, resources} from '../developer-17/setup-resources.mjs';
import {identity} from '../developer-17/setup-browser.mjs';
import {required, check, other} from './common.mjs';
import {agentRequest, access, digest, markerCount, transcript} from './access-common.mjs';
import {poll} from './enforcement.mjs';

// This recipe creates data through the existing public hierarchy helper. The
// sole parent supplies real roles, runtime/provider, custody and native finality.
export function createCrossOwnerReadFactory(bindings) {
  return async (ctx, {config}) => {
    const resourceConfig = requireResourceInputs(bindings);
    const subjects = ['user', 'creator', 'read_operator', 'outsider']
      .map(role => config.actors?.[role]?.subject);
    required(subjects.every(subject => typeof subject === 'string' && subject.trim())
      && subjects[0] === subjects[1] && new Set(subjects.slice(1)).size === 3,
    'admin_cross_read_exact_roles_required');
    // user/creator is an explicit, checked same-subject binding. Missing roles
    // never borrow another credential, and the operator remains a distinct user.
    required(identity(bindings, 'user').subject === subjects[0], 'admin_cross_read_resource_owner');
    const owned = await resources(ctx, bindings);
    const hierarchy = await owned.hierarchy();
    const f = {agentId: hierarchy.agentId, ownerSubject: subjects[0], operatorSubject: subjects[2],
      cleanup: owned.cleanup};
    const operator = await access(ctx, f, 'read_operator');
    required(operator.owner?.kind === 'person' && operator.owner.is_current_person === false
      && Array.isArray(operator.capabilities)
      && ['view_details', 'view_history', 'view_files'].every(c => operator.capabilities.includes(c))
      && ['interact', 'operate_agent', 'resolve_approval', 'manage_sharing']
        .every(c => !operator.capabilities.includes(c)), 'admin_cross_read_operator_authority');
    await agentRequest(ctx, f, 'outsider', 'GET', '', undefined, {allowed: [403, 404]});

    const marker = 'admin01-transcript-' + randomUUID();
    const text = 'Reply with exactly this text and nothing else: ' + marker;
    const inputKey = randomUUID();
    await ctx.observe('admin_cross_read_input_intent', async () => ({
      agent_id: f.agentId, actor: 'creator', key: inputKey, body: {text},
    }));
    const accepted = await agentRequest(ctx, f, 'creator', 'POST', '/input', {text},
      {key: inputKey, allowed: [200, 202]});
    check(typeof accepted.body?.queued === 'boolean', 'admin_cross_read_input_receipt');
    await poll(() => transcript(ctx, f, 'creator'), value => {
      check(value.agent_id === f.agentId && value.next == null && Array.isArray(value.turns),
        'admin_cross_read_transcript_identity');
      const matches = value.turns.filter(turn => turn.owner_message === text);
      check(matches.length <= 1, 'admin_cross_read_duplicate_turn');
      const turn = matches[0];
      check(!['failed', 'interrupted', 'cancelled'].includes(turn?.outcome?.type),
        'admin_cross_read_marker_turn_failed');
      if (turn?.outcome?.type !== 'completed') return false;
      check(markerCount(value, marker) === 1, 'admin_cross_read_marker_not_exact');
      return true;
    }, 'admin_cross_read_marker_deadline', resourceConfig.timeout_ms ?? 120000);

    const filePath = 'admin01-read-' + randomUUID() + '.txt';
    const content = 'Private local ADMIN read fixture ' + randomUUID() + '\n';
    const before = (await agentRequest(ctx, f, 'creator', 'GET', '/files/tree')).body;
    check(Array.isArray(before?.entries) && !before.entries.some(entry => entry.path === filePath),
      'admin_cross_read_file_collision');
    const fileKey = randomUUID(), body = {path: filePath, content_base64: Buffer.from(content).toString('base64')};
    await ctx.observe('admin_cross_read_file_intent', async () => ({
      agent_id: f.agentId, actor: 'creator', key: fileKey, body,
    }));
    // The whole disposable Sandbox is already in parent custody. Unknown file
    // or input acknowledgements fail setup; root retirement still owns cleanup.
    const written = await agentRequest(ctx, f, 'creator', 'POST', '/files', body, {key: fileKey});
    check(written.body?.path === filePath && written.body.deleted !== true,
      'admin_cross_read_file_receipt');
    const peer = other(written.timing.am);
    const bytes = (await agentRequest(ctx, f, 'creator', 'GET', '/files/content?'
      + new URLSearchParams({path: filePath}), undefined, {am: peer})).body;
    check(bytes.path === filePath && bytes.sha256 === digest(content)
      && Buffer.from(bytes.content_base64, 'base64').toString() === content,
    'admin_cross_read_file_readback');
    const deniedMarker = 'admin01-denied-' + randomUUID();
    return {...f, content, filePath, transcriptMarker: marker,
      deniedMarker, deniedInput: {text: 'Reply exactly ' + deniedMarker}, deniedKey: randomUUID()};
  };
}
