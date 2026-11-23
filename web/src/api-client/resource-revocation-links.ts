/** Select one status target without interpreting or following any URI. */
export function revocationStatusTarget(header: string | undefined): string | undefined {
  if (
    !header ||
    header.length > MAX_REVOCATION_LINK_LENGTH ||
    [...header].some((char) => (char < ' ' && char !== '\t') || char === '\u007f')
  )
    return undefined;

  const link = /[ \t]*<([^<>"\s]+)>[ \t]*/y;
  const parameter =
    /;[ \t]*([!#$%&'*+.^_`|~\da-z-]+)[ \t]*(?:=[ \t]*(?:"((?:[^"\\]|\\.)*)"|([!#$%&'*+.^_`|~\da-z-]+))[ \t]*)?/iy;
  let offset = 0;
  let count = 0;
  let target: string | undefined;
  while (offset < header.length) {
    if (++count > 32) return undefined;
    link.lastIndex = offset;
    const match = link.exec(header);
    if (!match) return undefined;
    offset = link.lastIndex;
    const parameters = new Map<string, string | undefined>();
    while (header[offset] === ';') {
      parameter.lastIndex = offset;
      const part = parameter.exec(header);
      if (!part) return undefined;
      const name = part[1]!.toLowerCase();
      if (parameters.has(name)) return undefined;
      parameters.set(name, part[2]?.replace(/\\(.)/g, '$1') ?? part[3]);
      offset = parameter.lastIndex;
    }
    const relations = parameters
      .get('rel')
      ?.toLowerCase()
      .split(/[ \t]+/)
      .filter(Boolean);
    if (!relations?.[0]) return undefined;
    if (relations.includes('status')) {
      // An anchor changes the link context; duplicate status targets are ambiguous.
      if (target !== undefined || parameters.has('anchor')) return undefined;
      target = match[1]!;
    }
    if (offset === header.length) return target;
    if (header[offset] !== ',' || ++offset === header.length) return undefined;
  }
  return undefined;
}
export const MAX_REVOCATION_LINK_LENGTH = 2048;
