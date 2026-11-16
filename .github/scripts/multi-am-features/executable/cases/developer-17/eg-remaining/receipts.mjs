import { servingBinding, validServingReceipt } from '../../../serving.mjs';

// Matches the integrator's R8 evidence-scope.mjs contract. This adapter remains
// additive; it never publishes an EG process under an AM label.
export function egBinding(replica, topology) {
  const source = topology?.egs?.[replica];
  if (
    !/^[a-f0-9]{40}$/.test(source?.source ?? '') ||
    !/@sha256:[a-f0-9]{64}$/.test(source?.image ?? '')
  )
    throw Error('eg20_eg_source_image_required');
  const process = { ...servingBinding(replica, { ams: topology.egs }) };
  delete process.am;
  delete process.schema;
  return {
    ...process,
    schema: 'c275-eg-serving-v1',
    scope: 'eg',
    replica,
    source: source.source,
    image: source.image,
    ...(source.draining === undefined ? {} : { draining: source.draining }),
  };
}
export function validEgReceipt(receipt) {
  if (receipt?.scope !== 'eg' || !['a', 'b'].includes(receipt.replica) || receipt.am !== undefined)
    return false;
  const snapshots = [receipt.serving, receipt.serving_after];
  if (
    !snapshots.every(
      (value) =>
        value?.schema === 'c275-eg-serving-v1' &&
        value.scope === 'eg' &&
        value.am === undefined &&
        value.replica === receipt.replica &&
        /^[a-f0-9]{40}$/.test(value.source ?? '') &&
        /@sha256:[a-f0-9]{64}$/.test(value.image ?? ''),
    ) ||
    snapshots[0].source !== snapshots[1].source ||
    snapshots[0].image !== snapshots[1].image
  )
    return false;
  const convert = (value) => ({ ...value, schema: 'c275-serving-v1', am: value.replica });
  return validServingReceipt({
    ...receipt,
    am: receipt.replica,
    serving: convert(receipt.serving),
    serving_after: convert(receipt.serving_after),
  });
}
export async function retainBrowserEvidence(ctx, value, label) {
  // EG receipts enter the R8 scope gate independently of genuine AM traffic.
  for (const receipt of value.eg_requests)
    await ctx.observe('eg20_browser_eg_serving', async () => ({
      schema: 'multi-eg-request-v1',
      receipt,
    }));
  if (value.requests.length)
    await ctx.observe(label, async () => ({ ...value, schema: 'multi-am-browser-evidence-v1' }));
  return ctx.observe(label + '_manifest', async () => ({
    ...value,
    schema: 'eg20-browser-evidence-v1',
  }));
}
