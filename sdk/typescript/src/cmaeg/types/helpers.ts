import { object, record, type Output, type Parser } from '../../schema.js';

/** Provider extension and JSON data values are intentionally unconstrained. */
export const anything: Parser<unknown> = (value) => value;

/** CloudEvents retain extension attributes alongside their required envelope. */
export function extensible<const Fields extends Readonly<Record<string, Parser<unknown>>>>(
  fields: Fields,
): Parser<Output<ReturnType<typeof object<Fields>>> & Readonly<Record<string, unknown>>> {
  const known = object(fields);
  return (value) => ({ ...record(value), ...known(value) });
}
