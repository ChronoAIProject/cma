import {executeCase} from '../../executable/runner.mjs';
import {need} from './provenance.mjs';
import {installContext} from './context.mjs';
import {journey} from './index.mjs';

// Sole parent entry point. It does not create a fixture, browser or application.
// Parent release/trust inputs are distinct from all registrar return values.
export async function invokeJourney({binding, release, trust, lifecycleTrust, environment}) {
  const proofs = [], responses = [];
  const originalRecord = environment.record;
  const definition = {...journey, run: async ctx => {
    need(typeof originalRecord === 'function', 'private_evidence_sink_missing');
    installContext(ctx, {binding, release, trust, lifecycleTrust, environment,
      adapter: environment.adapter, proofs, responses});
    return journey.run(ctx);
  }};
  return executeCase(definition, {case_id: journey.id, family: 'journey',
    required_steps: [...journey.steps]}, {...environment, record: async event => {
    if (event.kind === 'owner_proof') proofs.push(structuredClone(event.proof));
    if (event.kind === 'response') responses.push(structuredClone(event));
    await originalRecord?.(event);
  }});
}
