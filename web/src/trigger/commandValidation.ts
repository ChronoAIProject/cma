export type CommandValueParser<T> = (value: unknown) => T | null;

type ParsedCommand<T extends object> = {
  readonly [Key in keyof T]: CommandValueParser<T[Key]>;
};

export function commandObject<T extends object>(
  value: unknown,
  fields: ParsedCommand<T>,
): T | null {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return null;
  const record = value as Record<string, unknown>;
  const expected = Object.keys(fields);
  if (Object.keys(record).length !== expected.length) return null;
  const parsed: Record<string, unknown> = {};
  for (const key of expected) {
    if (!Object.prototype.hasOwnProperty.call(record, key)) return null;
    const parser = (fields as Record<string, CommandValueParser<unknown>>)[key];
    if (parser === undefined) return null;
    const item = parser(record[key]);
    if (item === null) return null;
    parsed[key] = item;
  }
  return parsed as T;
}

export function exactString(expected: string): CommandValueParser<string> {
  return (value) => (value === expected ? expected : null);
}

export function nonEmptyString(value: unknown): string | null {
  return typeof value === 'string' && value.length > 0 && value.length <= 2_048 ? value : null;
}

export function nonNegativeInteger(value: unknown): number | null {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 ? value : null;
}

export function commandBoolean(value: unknown): boolean | null {
  return typeof value === 'boolean' ? value : null;
}

export function oneOfStrings(values: readonly string[]): CommandValueParser<string> {
  const allowed = new Set(values);
  return (value) => (typeof value === 'string' && allowed.has(value) ? value : null);
}

export function oneOfLiterals<const T extends string>(values: readonly T[]): CommandValueParser<T> {
  const allowed = new Set<string>(values);
  return (value) => (typeof value === 'string' && allowed.has(value) ? (value as T) : null);
}
