/** Durable projection frames that may precede the frame carrying the cursor. */
export function isDurableProjectionPrefix(kind: string): boolean {
  switch (kind) {
    case 'TEXT_MESSAGE_START':
    case 'TEXT_MESSAGE_CONTENT':
    case 'TOOL_CALL_START':
    case 'TOOL_CALL_END':
      return true;
    default:
      return false;
  }
}
