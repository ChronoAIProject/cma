import { createHash } from 'node:crypto';
const hash = (value) => createHash('sha256').update(value).digest('hex');
const check = (value, code) => {
  if (!value) throw Error(code);
};
const proofFields = [
  'agent_sha256',
  'owner',
  'ingress',
  'kind',
  'method',
  'owner_epoch',
  'owner_instance_sha256',
  'ingress_instance_sha256',
  'request_id',
  'verdict',
];
const requestFields = [
  'request_id',
  'am',
  'serving_am',
  'method',
  'status',
  'elapsed_ms',
  'agent_id_sha256',
  'owner',
  'owner_epoch',
  'owner_stable',
];
const servingFields = [
  'schema',
  'am',
  'origin',
  'pod_uid',
  'instance_id',
  'container_id',
  'ready',
  'observed_at',
];
const attributionFields = ['verdict', 'basis'];
const select = (value, fields) => Object.fromEntries(fields.map((field) => [field, value?.[field]]));
const projectServing = (value) => (value ? select(value, servingFields) : undefined);
const projectAttribution = (value) => (value ? select(value, attributionFields) : undefined);

// Preserve the shared runner's original correlation proof without retaining
// headers, bodies, URLs or future adapter extensions that may contain secrets.
export function projectReceipt(receipt) {
  return {
    ...select(receipt, [
      'schema',
      'real_ui',
      'compiled_source',
      'compiled_manifest',
      'authentication',
      'secrets_redacted',
    ]),
    requests: Array.isArray(receipt?.requests)
      ? receipt.requests.map((r) => ({
          ...select(r, requestFields),
          ...(r.serving ? { serving: projectServing(r.serving) } : {}),
          ...(r.serving_after ? { serving_after: projectServing(r.serving_after) } : {}),
          ...(r.started_at ? { started_at: r.started_at } : {}),
          ...(r.ended_at ? { ended_at: r.ended_at } : {}),
          ...(r.attribution ? { attribution: projectAttribution(r.attribution) } : {}),
          ...(r.owner_proof ? { owner_proof: select(r.owner_proof, proofFields) } : {}),
        }))
      : [],
  };
}

export function validateReceipt(receipt, expectedSource, agentId, assert = check) {
  assert(
    receipt?.schema === 'multi-am-browser-evidence-v1' &&
      receipt.real_ui === true &&
      /^[a-f0-9]{40}$/.test(expectedSource ?? '') &&
      receipt.compiled_source === expectedSource &&
      /^sha256:[a-f0-9]{64}$/.test(receipt.compiled_manifest ?? ''),
    'compiled_real_ui_required',
  );
  assert(
    Array.isArray(receipt.requests) &&
      receipt.requests.length > 0 &&
      receipt.requests.every(
        (r) =>
          typeof r.request_id === 'string' &&
          r.request_id.trim() &&
          ['a', 'b'].includes(r.am) &&
          r.serving_am === r.am &&
          Number.isFinite(r.elapsed_ms) &&
          r.elapsed_ms >= 0 &&
          ['GET', 'HEAD', 'POST', 'PUT', 'PATCH', 'DELETE', 'OPTIONS'].includes(r.method) &&
          Number.isInteger(r.status) &&
          r.status >= 100 &&
          r.status < 600,
      ),
    'browser_request_attribution',
  );
  const bound = receipt.requests.filter((r) => r.agent_id_sha256);
  if (agentId)
    assert(
      bound.some((r) => r.agent_id_sha256 === hash(agentId)),
      'browser_bound_agent_evidence',
    );
  assert(
    bound.every((r) => {
      const p = r.owner_proof;
      return (
        p?.verdict === 'PASS' &&
        p.agent_sha256 === r.agent_id_sha256 &&
        p.request_id === r.request_id &&
        p.ingress === r.am &&
        p.owner === r.owner &&
        ['a', 'b'].includes(p.owner) &&
        p.owner_epoch === r.owner_epoch &&
        Number.isSafeInteger(p.owner_epoch) &&
        p.owner_epoch > 0 &&
        r.owner_stable === true &&
        (p.owner !== p.ingress ||
          (p.kind === 'peer_read' && p.method === r.method && ['GET', 'HEAD'].includes(r.method)))
      );
    }),
    'browser_current_owner_proof_required',
  );
  assert(receipt.secrets_redacted === true, 'browser_evidence_redaction');
}

export function validatePeerReads(receipt, switches = [], assert = check) {
  for (const transition of switches) {
    const write = receipt.requests.find((r) => r.request_id === transition.write);
    assert(
      write &&
        write.am !== transition.am &&
        receipt.requests
          .slice(transition.after)
          .some(
            (r) =>
              r.am === transition.am && r.serving_am === transition.am && ['GET', 'HEAD'].includes(r.method),
          ),
      'browser_peer_read_observed',
    );
  }
}
