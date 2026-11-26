import assert from 'node:assert/strict';
import test from 'node:test';
import {organizationShare} from './organization_share.mjs';

function fixture({keepAccess = false} = {}) {
  const agentId = 'fixture-agent', linkId = 'fixture-link', organization = 'fixture-organization';
  const memberships = [], visits = [];
  const audience = {kind: 'organization', organization_id: organization};
  const link = {agent_id: agentId, id: linkId, audience, accepted_count: 1, revision: 2};
  let member = false, accepted = false, revoked = false;
  const adapter = {ownerOf: async () => ({am: 'b', agent_id: agentId, epoch: 7, pod_uid: 'manager-b'}),
    organizationMembership: async input => {
      member = input.member;
      memberships.push(member);
      return {member};
    }};
  const user = {request: async (_am, method, path, options) => {
    if (method === 'POST') {
      assert.deepEqual(options.body.audience, audience);
      assert.equal(options.body.mode, 'view');
      assert.match(options.body.token, /^[a-f0-9]{64}$/);
      return {body: link};
    }
    if (method === 'GET') return {body: {items: [link]}};
    assert.equal(method, 'DELETE');
    assert.equal(path, `/api/v1/agents/${agentId}/share-links/${linkId}?expected_revision=2`);
    revoked = true;
  }};
  const outsider = {request: async (am, method, path, options) => {
    visits.push(am);
    if (path === '/api/v1/organizations') return {body: {items: member ? [{id: organization, role: 'member'}] : []}};
    if (method === 'POST') {
      if (!member) {
        assert.deepEqual(options.allowed, [403]);
        return {status: 403, body: {code: 'resource_forbidden'}};
      }
      accepted = true;
      return {status: 200, body: link};
    }
    assert(accepted);
    const response = member || keepAccess ? {status: 200, body: {capabilities: ['view_details']}} : {status: 404};
    if (options?.allowed && !options.allowed.includes(response.status)) throw Error('api_status_' + response.status);
    return response;
  }};
  return {context: {adapter, user, outsider, record: {agentId}}, memberships, visits, isRevoked: () => revoked};
}

test('organization admission uses current membership through each Manager and revokes the link', async () => {
  const {context, memberships, visits, isRevoked} = fixture();
  await organizationShare(context);
  assert.deepEqual(memberships, [false, true, false, false]);
  assert.deepEqual(new Set(visits), new Set(['a', 'b']));
  assert(isRevoked());
});

test('stale access after membership loss fails while still revoking the fixture link', async () => {
  const {context, memberships, isRevoked} = fixture({keepAccess: true});
  await assert.rejects(organizationShare(context), /api_status_200/);
  assert.equal(memberships.at(-1), false);
  assert(isRevoked());
});
