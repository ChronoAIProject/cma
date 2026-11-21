export function visibleUserMessage(content: string): string {
  const marker = '\n\nevent_ref: ';
  const index = content.lastIndexOf(marker);
  if (index <= 0) return content;
  const reference = content.slice(index + marker.length);
  const match = /^cmaeg1\.ev\.(0|[1-9]\d{0,2})\.[A-Za-z0-9_-]{38,}$/.exec(reference);
  if (match === null || match[0] !== reference || Number(match[1]) > 255 || reference.length > 4096)
    return content;
  return content.slice(0, index);
}
