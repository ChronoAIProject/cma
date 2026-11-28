import {randomBytes, randomUUID} from 'node:crypto';
import {assertOwner, check, id, other} from '../cma/core.mjs';
import {withRestoration} from '../failure.mjs';

const organization = 'fixture-organization';

export async function organizationShare({adapter, user, outsider, record, recordFailure}) {
  const {agentId} = record;
  const owner = assertOwner(await adapter.ownerOf({agentId}), agentId);
  const ingress = other(owner.am);
  const managers = [ingress, owner.am];
  const links = `/api/v1/agents/${encodeURIComponent(agentId)}/share-links`;
  const access = `/api/v1/resource-access/agent/${encodeURIComponent(agentId)}`;
  const setMembership = async member => {
    const confirmed = await adapter.organizationMembership({member});
    check(confirmed.member === member, 'organization_fixture_membership_unconfirmed');
  };
  await setMembership(false);
  const options = {body: {audience: {kind: 'organization', organization_id: organization}, mode: 'view',
    token: randomBytes(32).toString('hex')}, key: randomUUID()};
  const {body: link} = await user.request(ingress, 'POST', links, options);
  check(id(link?.id) && link.agent_id === agentId && link.audience?.organization_id === organization,
    'organization_share_identity_missing');
  const redeem = `/api/v1/agent-share-links/${encodeURIComponent(link.id)}/redeem`;
  const assertMembership = async (am, member) => {
    const {body} = await outsider.request(am, 'GET', '/api/v1/organizations');
    check(Array.isArray(body?.items) && body.items.length === Number(member), 'organization_membership_count_mismatch');
    if (member) check(body.items[0]?.id === organization && body.items[0]?.role === 'member',
      'organization_membership_identity_mismatch');
  };
  const refuse = async am => {
    const {body} = await outsider.request(am, 'POST', redeem, {body: {token: options.body.token}, allowed: [403]});
    check(body?.code === 'resource_forbidden', 'organization_share_refusal_untyped');
  };
  await withRestoration(async () => {
    for (const am of managers) {
      await assertMembership(am, false);
      await refuse(am);
    }
    await setMembership(true);
    let revision;
    for (const am of managers) {
      await assertMembership(am, true);
      const {body: receipt} = await outsider.request(am, 'POST', redeem, {body: {token: options.body.token}});
      check(receipt?.id === link.id && receipt.accepted_count === 1 && Number.isSafeInteger(receipt.revision) &&
        (revision === undefined || revision === receipt.revision), 'organization_share_redemption_duplicated');
      revision = receipt.revision;
      const {body: rights} = await outsider.request(am, 'GET', access);
      check(Array.isArray(rights?.capabilities) && rights.capabilities.includes('view_details') &&
        !rights.capabilities.includes('operate_agent') && !rights.capabilities.includes('manage_sharing'),
      'organization_share_capabilities_changed');
    }
    await setMembership(false);
    for (const am of managers) {
      await assertMembership(am, false);
      await outsider.request(am, 'GET', access, {allowed: [404]});
      await refuse(am);
    }
  }, async () => {
    try { await setMembership(false); }
    finally {
      const {body: listing} = await user.request(owner.am, 'GET', links);
      const current = listing?.items?.find(row => row.id === link.id);
      check(Number.isSafeInteger(current?.revision), 'organization_share_cleanup_revision_missing');
      await user.request(ingress, 'DELETE', `${links}/${link.id}?expected_revision=${current.revision}`,
        {key: randomUUID()});
    }
  }, recordFailure);
}
