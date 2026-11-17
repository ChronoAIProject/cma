import { ProtocolError } from './errors.js';

export type Parser<Value> = (value: unknown) => Value;
export type Output<Parse> = Parse extends Parser<infer Value> ? Value : never;
type Shape = Readonly<Record<string, Parser<unknown>>>;
type ObjectOutput<Fields extends Shape> = {
  readonly [Key in keyof Fields as undefined extends Output<Fields[Key]> ? never : Key]: Output<
    Fields[Key]
  >;
} & {
  readonly [Key in keyof Fields as undefined extends Output<Fields[Key]> ? Key : never]?: Exclude<
    Output<Fields[Key]>,
    undefined
  >;
};

export function record(value: unknown): Record<string, unknown> {
  if (typeof value !== 'object' || value === null || Array.isArray(value))
    throw new ProtocolError('expected an object');
  return value as Record<string, unknown>;
}

export const text: Parser<string> = (value) => {
  if (typeof value !== 'string') throw new ProtocolError('expected a string');
  return value;
};

export const boolean: Parser<boolean> = (value) => {
  if (typeof value !== 'boolean') throw new ProtocolError('expected a boolean');
  return value;
};

export const integer: Parser<number> = (value) => {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0)
    throw new ProtocolError('expected a nonnegative safe integer');
  return value;
};

export const positiveInteger: Parser<number> = (value) => {
  const parsed = integer(value);
  if (parsed === 0) throw new ProtocolError('expected a positive safe integer');
  return parsed;
};

export function optional<Value>(parse: Parser<Value>): Parser<Value | undefined> {
  return (value) => (value === undefined ? undefined : parse(value));
}

export function nullable<Value>(parse: Parser<Value>): Parser<Value | null> {
  return (value) => (value === null ? null : parse(value));
}

export function array<Value>(parse: Parser<Value>): Parser<readonly Value[]> {
  return (value) => {
    if (!Array.isArray(value)) throw new ProtocolError('expected an array');
    return value.map((item: unknown) => parse(item));
  };
}

export function enumeration<const Values extends readonly string[]>(
  ...values: Values
): Parser<Values[number]> {
  return (value) => {
    if (typeof value !== 'string' || !values.includes(value))
      throw new ProtocolError(`expected one of ${values.join(', ')}`);
    return value;
  };
}

export function object<const Fields extends Shape>(fields: Fields): Parser<ObjectOutput<Fields>> {
  return (value) => {
    const source = record(value);
    const result: Record<string, unknown> = {};
    for (const [key, parse] of Object.entries(fields)) {
      try {
        const parsed = parse(source[key]);
        if (parsed !== undefined)
          Object.defineProperty(result, key, { value: parsed, enumerable: true });
      } catch (cause) {
        if (cause instanceof ProtocolError)
          throw new ProtocolError(`${key}: ${cause.message}`, { cause });
        throw cause;
      }
    }
    return result as ObjectOutput<Fields>;
  };
}

export function dictionary<Value>(parse: Parser<Value>): Parser<Readonly<Record<string, Value>>> {
  return (value) =>
    Object.fromEntries(Object.entries(record(value)).map(([key, item]) => [key, parse(item)]));
}

export function tagged<const Variants extends Shape>(
  variants: Variants,
  field = 'type',
): Parser<Output<Variants[keyof Variants]>> {
  return (value) => {
    const source = record(value);
    const tag = source[field];
    if (typeof tag !== 'string' || !Object.hasOwn(variants, tag))
      throw new ProtocolError(`unknown ${field} discriminator`);
    return variants[tag]!(value) as Output<Variants[keyof Variants]>;
  };
}

export function variant<const Name extends string, const Fields extends Shape>(
  name: Name,
  fields: Fields,
) {
  return object({ type: enumeration(name), ...fields });
}
