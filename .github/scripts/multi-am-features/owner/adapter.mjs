export function createAdapter(config) {
  async function call(method, body = {}) {
    const response = await fetch(`${config.control_origin}/${method}`, {
      method: 'POST',
      headers: {'Content-Type': 'application/json', Authorization: `Bearer ${config.control_token}`},
      body: JSON.stringify(body), signal: AbortSignal.timeout(method === 'observeScale' ? 260000 : 180000),
    });
    const value = await response.json();
    if (!response.ok) throw new Error(`adapter ${method}: ${value.error || response.status}`);
    return value;
  }
  return Object.fromEntries(['topology','runtimeFor','prepareTurn','ownerOf','killAm',
    'releaseTurn','oracle','restoreAm','captureFailure','titleEffects','titleIntent','skillEffects',
    'drainOwner','restartDrained','peerWriteEvidence','preparedReady','preparedEvidence','observeScale',
    'observeNeverReady','liveOwnerLease','lifecycleSnapshot','organizationMembership',
    'configurationLog','configurationDispatch','publicationEvidence','retirementPurge','removeRuntimePod',
    'artifactPublicationAbort','preparedPublicationAbort','preparedSchemaBarrier','preparedSchemaRace','preparedSchemaRefusals',
    'writerPublicationAbort','writerPublicationEvidence','runtimeLossEvidence']
    .map(name=>[name,(body)=>call(name,body)]));
}
export default createAdapter;
