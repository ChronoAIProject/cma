import assert from 'node:assert/strict';
import type { AnswerDeliverableReading, DocumentAccessOptions } from '../../src/index.js';
import type { ContractCase, Dispatch } from './context.js';

const root = '/api/v1/agents/{id}';
const listing = `GET ${root}/answers/{answer_id}/deliverables`;
const reading = `${listing}/{delivery_id}`;
const download = `${reading}/download`;

function metadata(fixture: ContractCase, corpus: ContractCase[]): ContractCase {
  const path = fixture.request.path.replace(/\/download$/, '');
  const candidates = corpus.filter(
    (entry) =>
      entry.operation === reading &&
      entry.request.path === path &&
      entry.response.status === 200 &&
      ['ready', 'unchecked', 'version_changed'].includes(
        (entry.response.body as AnswerDeliverableReading).item.status,
      ),
  );
  const found =
    candidates.find((entry) => entry.scenario === fixture.scenario) ??
    candidates.find((entry) => entry.scenario === 'deliverable_owner');
  assert(found, `Missing real metadata companion for ${fixture.id}`);
  return found;
}

export function deliveryExchange(fixture: ContractCase, corpus: ContractCase[]): ContractCase[] {
  if (fixture.operation !== download) return [fixture];
  const before = metadata(fixture, corpus);
  return fixture.response.status < 400 ? [before, fixture, before] : [before, fixture];
}

export function deliverableDispatch(corpus: ContractCase[]): Record<string, Dispatch> {
  return {
    [`POST ${root}/deliverable-references`]: ({ client, param, body, options }) =>
      client.agents.describeDeliverable(param('id'), body, options),
    [listing]: ({ client, param, options }) =>
      client.agents.answerDeliverables(param('id'), param('answer_id'), options),
    [reading]: ({ client, param, query, options }) =>
      client.agents.readAnswerDeliverable(param('id'), param('answer_id'), param('delivery_id'), {
        ...(options as DocumentAccessOptions),
        verify: query.has('verify') ? query.get('verify') === 'true' : undefined,
      }),
    [download]: async ({ client, param, fixture, options }) => {
      const expected = metadata(fixture, corpus).response.body as AnswerDeliverableReading;
      const bytes = await client.agents.downloadAnswerDeliverable(
        param('id'),
        param('answer_id'),
        expected.item,
        options,
      );
      return new TextDecoder().decode(bytes);
    },
  };
}
