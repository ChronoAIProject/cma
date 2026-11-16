import {read, P} from './common.mjs';

export async function triggerAuditRecords(ctx, actor, am) {
  const rows = [], seen = new Set(), cursors = new Set(); let cursor;
  for (let pageNumber = 0; pageNumber < 20; pageNumber++) {
    const page = await read(ctx, P + '/activity?' + new URLSearchParams({actor, action: 'configure', limit: '50',
      ...(cursor ? {cursor} : {})}), am);
    ctx.assert(Array.isArray(page?.items) && page.items.length <= 50
      && (page.next === null || (typeof page.next === 'string' && page.next.trim())), 'admin_trigger_audit_page');
    for (const row of page.items) {
      ctx.assert(typeof row.id === 'string' && row.id.trim() && !seen.has(row.id)
        && Number.isFinite(Date.parse(row.at)) && row.detail && typeof row.detail === 'object', 'admin_trigger_audit_row');
      ctx.assert(row.actor === actor && row.action === 'configure', 'admin_trigger_audit_filter');
      seen.add(row.id); rows.push(row);
    }
    if (page.next === null) return rows;
    ctx.assert(!cursors.has(page.next), 'admin_trigger_audit_cursor');
    cursor = page.next; cursors.add(cursor);
  }
  throw Error('admin_trigger_audit_bound');
}

export async function triggerAuditJoin(ctx, f, command, action, priorIds) {
  const joined = [];
  for (const am of ['a', 'b']) {
    const rows = await triggerAuditRecords(ctx, f.adminSubject, am);
    const matches = rows.filter(row => row.actor === f.adminSubject && row.action === 'configure'
      && row.target?.kind === 'resource' && row.target.id === 'triggers' && row.outcome?.status === 'succeeded'
      && row.detail.revision === String(command.body.revision)
      && row.detail.status === (action === 'pause' ? 'paused' : 'running'));
    ctx.assert(matches.length === 1 && !priorIds.has(matches[0].id), 'admin_trigger_command_audit');
    joined.push({am, record: matches[0]});
  }
  ctx.assert(joined[0].record.id === joined[1].record.id && joined[0].record.at === joined[1].record.at,
    'admin_trigger_audit_peer_identity');
  return ctx.observe('admin_trigger_command_audit_join', async () => joined);
}
