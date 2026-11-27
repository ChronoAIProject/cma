export function createAdapter(config) {
  async function call(method, body = {}) {
    const response = await fetch(`${config.control_origin}/${method}`, {
      method: 'POST',
      headers: {'Content-Type': 'application/json', Authorization: `Bearer ${config.control_token}`},
      body: JSON.stringify(body), signal: AbortSignal.timeout(method === 'observeScale' ? 260000 : 180000),
    });
    const value = await response.json();
    if (!response.ok) {
      const cause = new Error(`adapter ${method}: ${value.error || response.status}`);
      const preparedCode = /^(?:RuntimeError|ValueError):((?:prepared_|disposable_feature_fixture_)[a-z0-9_]{1,96})$/
        .exec(value.error ?? '')?.[1];
      if (method.startsWith('prepared') && preparedCode) throw new Error(preparedCode, {cause});
      throw cause;
    }
    return value;
  }
  return Object.fromEntries(['topology','runtimeFor','prepareTurn','ownerOf','killAm',
    'releaseTurn','oracle','restoreAm','captureFailure','titleEffects','titleIntent','skillEffects',
    'drainOwner','restartDrained','peerWriteEvidence','preparedReady','preparedEvidence','preparedRuntimeEvidence','observeScale',
    'observeNeverReady','liveOwnerLease','lifecycleSnapshot','organizationMembership',
    'configurationLog','configurationDispatch','publicationEvidence','retirementPurge','removeRuntimePod',
    'artifactPublicationAbort','artifactPublicationRace','preparedSchemaBarrier','preparedSchemaRace','preparedSchemaRefusals',
    'writerPublicationAbort','writerPublicationEvidence','runtimeLossPublicationAbort','runtimeLossEvidence','runtimeInventoryAuthority',
    'shareRedemptionPublicationAbort','accessProposalAbort','seedOversizeFile']
    .map(name=>[name,(body)=>call(name,body)]));
}
export default createAdapter;
