import { ProtocolError } from '../errors.js';
import { enumeration, object, record, tagged, text, type Output, type Parser } from '../schema.js';

function strict<const Value>(parse: Parser<Value>): Parser<Value> {
  return (value) => {
    const parsed = parse(value);
    const source = record(value);
    if (Object.keys(source).some((key) => !Object.hasOwn(record(parsed), key)))
      throw new ProtocolError('Unknown task template field');
    return parsed;
  };
}
const version = (value: unknown): 1 | 2 => {
  if (value !== 1 && value !== 2) throw new ProtocolError('Unsupported task template version');
  return value;
};
export const taskTemplate = tagged(
  {
    research: strict(
      object({ id: enumeration('research'), version, fields: strict(object({ topic: text })) }),
    ),
    spreadsheet: strict(
      object({ id: enumeration('spreadsheet'), version, fields: strict(object({ link: text })) }),
    ),
    document: strict(
      object({
        id: enumeration('document'),
        version,
        fields: strict(object({ type: text, subject: text, audience: text })),
      }),
    ),
    website: strict(
      object({ id: enumeration('website'), version, fields: strict(object({ project: text })) }),
    ),
    code: strict(
      object({
        id: enumeration('code'),
        version,
        fields: strict(object({ language: text, behavior: text })),
      }),
    ),
  },
  'id',
);
export type TaskTemplate = Output<typeof taskTemplate>;
