import type { ToolCall } from './model';

/** Read literal public command titles. Shell programs and substitutions stay ordinary tools. */
function words(command: string): string[] | undefined {
  const result: string[] = [];
  let word = '';
  let quote = '';
  let escaped = false;
  for (const character of command.trim()) {
    if (escaped) {
      word += character;
      escaped = false;
      continue;
    }
    if (character === '\\' && quote !== "'") {
      escaped = true;
      continue;
    }
    if (quote) {
      if (character === quote) quote = '';
      else if (quote === '"' && (character === '$' || character === '`')) return undefined;
      else word += character;
      continue;
    }
    if (character === '"' || character === "'") {
      quote = character;
      continue;
    }
    if (';&|`$\n\r'.includes(character)) return undefined;
    if (/\s/.test(character)) {
      if (word) {
        result.push(word);
        word = '';
      }
    } else word += character;
  }
  if (quote || escaped) return undefined;
  if (word) result.push(word);
  return result;
}
export interface ScreenOperation {
  readonly action: 'navigate' | 'click' | 'drag' | 'type' | 'key';
  readonly application: 'Browser' | 'Desktop';
  readonly description: string;
  readonly at?: string;
}
/** Surface following also covers window activation/inspection without inventing a step row. */
export function screenApplication(tool: ToolCall): ScreenOperation['application'] | undefined {
  if (tool.kind !== 'execute') return undefined;
  const program = words(tool.title)?.[0]?.split('/').pop();
  if (program === 'cma-browser') return 'Browser';
  if (program === 'xdotool' || program === 'wmctrl') return 'Desktop';
  return undefined;
}
export function screenOperation(tool: ToolCall): ScreenOperation | undefined {
  const timestamp = tool.startedAt ?? tool.finishedAt;
  const at = Number.isFinite(Date.parse(timestamp ?? '')) ? timestamp : undefined;
  if (tool.kind !== 'execute') return undefined;
  const tokens = words(tool.title);
  if (!tokens) return undefined;
  const [program, operation, ...arguments_] = tokens;
  if (program?.split('/').pop() === 'cma-browser') {
    if (operation === 'navigate' && arguments_.length === 1) {
      try {
        const url = new URL(arguments_[0]!);
        if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password)
          return undefined;
        return {
          action: 'navigate',
          application: 'Browser',
          description: `Opened ${url.host}${url.pathname === '/' ? '' : url.pathname}`,
          at,
        };
      } catch {
        return undefined;
      }
    }
    if (operation === 'input' && arguments_.length === 1) {
      try {
        const value = JSON.parse(arguments_[0]!) as Record<string, unknown>;
        if (value.kind === 'click' && Number.isInteger(value.x) && Number.isInteger(value.y))
          return {
            action: 'click',
            application: 'Browser',
            description: `Clicked at ${value.x}, ${value.y}`,
            at,
          };
        if (value.kind === 'drag')
          return { action: 'drag', application: 'Browser', description: 'Dragged the pointer', at };
        if (value.kind === 'text' && typeof value.text === 'string')
          return { action: 'type', application: 'Browser', description: 'Typed text', at };
        if (value.kind === 'key' && typeof value.key === 'string')
          return { action: 'key', application: 'Browser', description: `Pressed ${value.key}`, at };
      } catch {
        return undefined;
      }
    }
  }
  if (program?.split('/').pop() !== 'xdotool') return undefined;
  const tail = arguments_.join(' ');
  if (operation === 'type')
    return { action: 'type', application: 'Desktop', description: 'Typed text', at };
  if (operation === 'key' && arguments_.length === 1)
    return { action: 'key', application: 'Desktop', description: `Pressed ${arguments_[0]}`, at };
  if (operation === 'click' && /^\d+$/.test(tail))
    return { action: 'click', application: 'Desktop', description: 'Clicked', at };
  if (operation === 'mousemove' && /^(?:--sync )?\d+ \d+ click \d+$/.test(tail))
    return { action: 'click', application: 'Desktop', description: 'Clicked', at };
  if (
    ['mousemove', 'mousedown'].includes(operation ?? '') &&
    /^[-\d\s]+mousedown \d+ mousemove (?:--sync )?\d+ \d+ mouseup \d+$/.test(
      (operation === 'mousedown' ? '0 0 mousedown ' : '') + tail,
    )
  )
    return { action: 'drag', application: 'Desktop', description: 'Dragged the pointer', at };
  return undefined;
}
export function screenOperationTime(at: string): string {
  return new Intl.DateTimeFormat(undefined, {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hourCycle: 'h23',
  }).format(new Date(at));
}
