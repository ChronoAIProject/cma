import { ProtocolError } from '../errors.js';
import type { AguiState } from '../wire.js';
import type { JsonPatch } from './events.js';

type JsonPrimitive = boolean | null | number | string;
type JsonValue = JsonPrimitive | JsonObject | JsonValue[];

interface JsonObject {
  [key: string]: JsonValue;
}

interface Pointer {
  readonly raw: string;
  readonly tokens: readonly string[];
}

type PatchOperation =
  | { readonly op: 'add'; readonly path: Pointer; readonly value: JsonValue }
  | { readonly op: 'remove'; readonly path: Pointer }
  | { readonly op: 'replace'; readonly path: Pointer; readonly value: JsonValue }
  | { readonly op: 'move'; readonly path: Pointer; readonly from: Pointer }
  | { readonly op: 'copy'; readonly path: Pointer; readonly from: Pointer }
  | { readonly op: 'test'; readonly path: Pointer; readonly value: JsonValue };

const RFC_6902_OPERATIONS = new Set(['add', 'remove', 'replace', 'move', 'copy', 'test']);
const ARRAY_INDEX = /^(?:0|[1-9][0-9]*)$/u;
const hasOwn = (value: object, key: PropertyKey): boolean =>
  Object.prototype.hasOwnProperty.call(value, key);

/**
 * Apply one AG-UI `STATE_DELTA` to its public state document.
 *
 * The operation list is validated as RFC 6902, the caller's document is never
 * mutated, and pointers never traverse JavaScript's prototype chain. Any
 * malformed operation is a protocol failure rather than a patch the client
 * partly applies or silently ignores.
 */
export function applyStateDelta(state: AguiState, patches: readonly JsonPatch[]): AguiState {
  try {
    let document = cloneJson(state, 'AG-UI state');
    const operations = cloneJson(patches, 'AG-UI state patch list');
    if (!Array.isArray(operations)) {
      throw new ProtocolError('AG-UI state patch list was not an array');
    }

    for (const [index, raw] of operations.entries()) {
      document = applyOperation(document, operationFrom(raw, index), index);
    }

    if (!isJsonObject(document)) {
      throw new ProtocolError('AG-UI state delta did not produce a state object');
    }
    return document as unknown as AguiState;
  } catch (cause) {
    if (cause instanceof ProtocolError) {
      throw cause;
    }
    const code = cause instanceof Error && cause.name !== 'Error' ? ` (${cause.name})` : '';
    throw new ProtocolError(`AG-UI state delta was not a valid RFC 6902 patch${code}`, { cause });
  }
}

function operationFrom(raw: JsonValue, index: number): PatchOperation {
  if (!isJsonObject(raw)) {
    throw new ProtocolError(`AG-UI state patch ${String(index)} was not an object`);
  }

  const op = raw.op;
  if (typeof op !== 'string' || !RFC_6902_OPERATIONS.has(op)) {
    throw new ProtocolError(
      `AG-UI state patch ${String(index)} used unsupported operation '${String(op)}'`,
    );
  }
  const path = pointerFrom(raw.path, index, 'path');

  switch (op) {
    case 'add':
    case 'replace':
    case 'test': {
      if (!hasOwn(raw, 'value')) {
        throw new ProtocolError(`AG-UI state patch ${String(index)} ${op} operation omitted value`);
      }
      return { op, path, value: raw.value as JsonValue };
    }
    case 'move':
    case 'copy': {
      const from = pointerFrom(raw.from, index, 'from');
      if (op === 'move' && isProperPrefix(from.tokens, path.tokens)) {
        throw new ProtocolError(
          `AG-UI state patch ${String(index)} cannot move a value into its own descendant`,
        );
      }
      return { op, path, from };
    }
    case 'remove':
      return { op, path };
    default:
      throw new ProtocolError(`AG-UI state patch ${String(index)} was not an RFC 6902 operation`);
  }
}

function pointerFrom(raw: JsonValue | undefined, index: number, member: 'from' | 'path'): Pointer {
  if (typeof raw !== 'string' || (raw !== '' && !raw.startsWith('/'))) {
    throw new ProtocolError(
      `AG-UI state patch ${String(index)} ${member} was not an RFC 6901 JSON Pointer`,
    );
  }

  const tokens =
    raw === ''
      ? []
      : raw
          .slice(1)
          .split('/')
          .map((token) => decodeToken(token, index));
  if (tokens.includes('__proto__')) {
    throw new ProtocolError(
      `AG-UI state patch ${String(index)} ${member} attempted prototype access`,
    );
  }
  return { raw, tokens };
}

function decodeToken(token: string, index: number): string {
  for (let offset = token.indexOf('~'); offset !== -1; offset = token.indexOf('~', offset + 2)) {
    const escaped = token[offset + 1];
    if (escaped !== '0' && escaped !== '1') {
      throw new ProtocolError(`AG-UI state patch ${String(index)} used an invalid RFC 6901 escape`);
    }
  }
  return token.replaceAll('~1', '/').replaceAll('~0', '~');
}

function isProperPrefix(prefix: readonly string[], value: readonly string[]): boolean {
  return prefix.length < value.length && prefix.every((token, index) => token === value[index]);
}

function applyOperation(document: JsonValue, operation: PatchOperation, index: number): JsonValue {
  switch (operation.op) {
    case 'add':
      return addAt(document, operation.path, operation.value, index);
    case 'remove':
      return removeAt(document, operation.path, index).document;
    case 'replace':
      return replaceAt(document, operation.path, operation.value, index);
    case 'copy': {
      const value = cloneJson(valueAt(document, operation.from, index), 'AG-UI copy source');
      return addAt(document, operation.path, value, index);
    }
    case 'move': {
      if (pointersEqual(operation.from, operation.path)) {
        valueAt(document, operation.from, index);
        return document;
      }
      const removed = removeAt(document, operation.from, index);
      return addAt(removed.document, operation.path, removed.value, index);
    }
    case 'test':
      if (!jsonEqual(valueAt(document, operation.path, index), operation.value)) {
        throw new ProtocolError(`AG-UI state patch ${String(index)} test operation failed`);
      }
      return document;
    default:
      throw new ProtocolError(`AG-UI state patch ${String(index)} was not an RFC 6902 operation`);
  }
}

function pointersEqual(left: Pointer, right: Pointer): boolean {
  return (
    left.tokens.length === right.tokens.length &&
    left.tokens.every((token, index) => token === right.tokens[index])
  );
}

function addAt(document: JsonValue, pointer: Pointer, value: JsonValue, index: number): JsonValue {
  if (pointer.tokens.length === 0) {
    return value;
  }
  const { parent, token } = parentAt(document, pointer, index);
  if (Array.isArray(parent)) {
    const arrayIndex = token === '-' ? parent.length : indexForAdd(parent, token, pointer, index);
    parent.splice(arrayIndex, 0, value);
  } else {
    defineJsonMember(parent, token, value);
  }
  return document;
}

function replaceAt(
  document: JsonValue,
  pointer: Pointer,
  value: JsonValue,
  index: number,
): JsonValue {
  if (pointer.tokens.length === 0) {
    return value;
  }
  const { parent, token } = parentAt(document, pointer, index);
  if (Array.isArray(parent)) {
    parent[indexForExisting(parent, token, pointer, index)] = value;
  } else {
    requireOwn(parent, token, pointer, index);
    defineJsonMember(parent, token, value);
  }
  return document;
}

function removeAt(
  document: JsonValue,
  pointer: Pointer,
  index: number,
): { readonly document: JsonValue; readonly value: JsonValue } {
  if (pointer.tokens.length === 0) {
    return { document: null, value: document };
  }
  const { parent, token } = parentAt(document, pointer, index);
  if (Array.isArray(parent)) {
    const arrayIndex = indexForExisting(parent, token, pointer, index);
    const value = parent[arrayIndex] as JsonValue;
    parent.splice(arrayIndex, 1);
    return { document, value };
  }

  requireOwn(parent, token, pointer, index);
  const value = parent[token] as JsonValue;
  delete parent[token];
  return { document, value };
}

function parentAt(
  document: JsonValue,
  pointer: Pointer,
  index: number,
): { readonly parent: JsonObject | JsonValue[]; readonly token: string } {
  const token = pointer.tokens.at(-1) as string;
  const parentTokens = pointer.tokens.slice(0, -1);
  const parent = valueAt(document, { raw: pointer.raw, tokens: parentTokens }, index);
  if (!isContainer(parent)) {
    throw unresolved(pointer, index);
  }
  return { parent, token };
}

function valueAt(document: JsonValue, pointer: Pointer, index: number): JsonValue {
  let value = document;
  for (const token of pointer.tokens) {
    if (Array.isArray(value)) {
      value = value[indexForExisting(value, token, pointer, index)] as JsonValue;
    } else if (isJsonObject(value)) {
      requireOwn(value, token, pointer, index);
      value = value[token] as JsonValue;
    } else {
      throw unresolved(pointer, index);
    }
  }
  return value;
}

function requireOwn(object: JsonObject, token: string, pointer: Pointer, index: number): void {
  if (!hasOwn(object, token)) {
    throw unresolved(pointer, index);
  }
}

function indexForAdd(array: JsonValue[], token: string, pointer: Pointer, index: number): number {
  const arrayIndex = parseArrayIndex(token, pointer, index);
  if (arrayIndex > array.length) {
    throw unresolved(pointer, index);
  }
  return arrayIndex;
}

function indexForExisting(
  array: JsonValue[],
  token: string,
  pointer: Pointer,
  index: number,
): number {
  const arrayIndex = parseArrayIndex(token, pointer, index);
  if (arrayIndex >= array.length || !hasOwn(array, arrayIndex)) {
    throw unresolved(pointer, index);
  }
  return arrayIndex;
}

function parseArrayIndex(token: string, pointer: Pointer, index: number): number {
  if (!ARRAY_INDEX.test(token)) {
    throw unresolved(pointer, index);
  }
  const value = Number(token);
  if (!Number.isSafeInteger(value)) {
    throw unresolved(pointer, index);
  }
  return value;
}

function unresolved(pointer: Pointer, index: number): ProtocolError {
  return new ProtocolError(
    `AG-UI state patch ${String(index)} could not resolve pointer '${pointer.raw}'`,
  );
}

function jsonEqual(left: JsonValue, right: JsonValue): boolean {
  if (left === right) {
    return true;
  }
  if (Array.isArray(left) || Array.isArray(right)) {
    return (
      Array.isArray(left) &&
      Array.isArray(right) &&
      left.length === right.length &&
      left.every((value, index) => jsonEqual(value, right[index] as JsonValue))
    );
  }
  if (isJsonObject(left) && isJsonObject(right)) {
    const keys = Object.keys(left);
    return (
      keys.length === Object.keys(right).length &&
      keys.every(
        (key) => hasOwn(right, key) && jsonEqual(left[key] as JsonValue, right[key] as JsonValue),
      )
    );
  }
  return false;
}

function cloneJson(value: unknown, name: string, ancestors = new WeakSet<object>()): JsonValue {
  if (
    value === null ||
    typeof value === 'boolean' ||
    typeof value === 'string' ||
    (typeof value === 'number' && Number.isFinite(value))
  ) {
    return value;
  }
  if (typeof value !== 'object') {
    throw new ProtocolError(`${name} contained a value that JSON cannot represent`);
  }
  if (ancestors.has(value)) {
    throw new ProtocolError(`${name} contained a cycle`);
  }

  ancestors.add(value);
  try {
    if (Array.isArray(value)) {
      return cloneJsonArray(value, name, ancestors);
    }
    return cloneJsonObject(value, name, ancestors);
  } finally {
    ancestors.delete(value);
  }
}

function cloneJsonArray(value: unknown[], name: string, ancestors: WeakSet<object>): JsonValue[] {
  if (
    Object.getPrototypeOf(value) !== Array.prototype ||
    Object.getOwnPropertySymbols(value).length
  ) {
    throw new ProtocolError(`${name} contained a non-JSON array`);
  }
  const names = Object.getOwnPropertyNames(value);
  if (names.length !== value.length + 1 || !hasOwn(value, 'length')) {
    throw new ProtocolError(`${name} contained a sparse array or an array property`);
  }

  const clone: JsonValue[] = [];
  for (let index = 0; index < value.length; index += 1) {
    const descriptor = Object.getOwnPropertyDescriptor(value, String(index));
    if (!descriptor?.enumerable || !hasOwn(descriptor, 'value')) {
      throw new ProtocolError(`${name} contained a sparse array or an accessor`);
    }
    clone.push(cloneJson(descriptor.value, name, ancestors));
  }
  return clone;
}

function cloneJsonObject(value: object, name: string, ancestors: WeakSet<object>): JsonObject {
  const prototype = Object.getPrototypeOf(value);
  if (
    (prototype !== Object.prototype && prototype !== null) ||
    Object.getOwnPropertySymbols(value).length
  ) {
    throw new ProtocolError(`${name} contained a non-JSON object`);
  }
  const names = Object.getOwnPropertyNames(value);
  if (names.length !== Object.keys(value).length) {
    throw new ProtocolError(`${name} contained a non-enumerable property`);
  }

  const clone: JsonObject = {};
  for (const key of names) {
    const descriptor = Object.getOwnPropertyDescriptor(value, key);
    if (!descriptor?.enumerable || !hasOwn(descriptor, 'value')) {
      throw new ProtocolError(`${name} contained an accessor`);
    }
    defineJsonMember(clone, key, cloneJson(descriptor.value, name, ancestors));
  }
  return clone;
}

function defineJsonMember(object: JsonObject, key: string, value: JsonValue): void {
  Object.defineProperty(object, key, {
    configurable: true,
    enumerable: true,
    value,
    writable: true,
  });
}

function isJsonObject(value: JsonValue): value is JsonObject {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isContainer(value: JsonValue): value is JsonObject | JsonValue[] {
  return Array.isArray(value) || isJsonObject(value);
}
