import {assertOwner, check, other} from '../cma/core.mjs';
import {withRestoration} from '../failure.mjs';

export async function retryOwnerPublication({api, adapter, agentId, owner, operation, request, recordFailure}) {
  assertOwner(owner, agentId);
  const ingress = other(owner.am);
  const evidence = () => adapter.writerPublicationEvidence({agentId, operation});
  const before = await evidence();
  let armed, disarmed, result;
  await withRestoration(async () => {
    armed = await adapter.writerPublicationAbort({arm: true});
    result = await api.router.dispatch(agentId, am => {
      check(am === ingress, 'writer_publication_owner_changed');
      return request(am);
    });
  }, async () => {
    disarmed = await adapter.writerPublicationAbort({arm: false});
  }, recordFailure);
  check(Number.isSafeInteger(armed?.count) && disarmed?.count === armed.count + 1,
    'writer_publication_abort_not_consumed_once');
  const after = await evidence();
  check(Number.isSafeInteger(before[owner.am]) && Number.isSafeInteger(before[ingress])
    && after[owner.am] === before[owner.am] + 1 && after[ingress] === before[ingress],
    'writer_publication_abort_not_attributed_to_owner');
  return result.body;
}
