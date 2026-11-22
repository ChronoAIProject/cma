import { enumeration, object, positiveInteger, tagged, text, type Output } from '../schema.js';

export const deliverableResource = tagged(
  {
    agent_artifact: object({ kind: enumeration('agent_artifact'), id: text }),
    document: object({ kind: enumeration('document'), id: text, revision: positiveInteger }),
    live_file: object({ kind: enumeration('live_file'), path: text }),
  },
  'kind',
);

export type DeliverableResource = Output<typeof deliverableResource>;
