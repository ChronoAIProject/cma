/** Commands reported by this Agent's running provider, never a guessed CLI list. */
export interface SessionCommand {
  readonly name: string;
  readonly description: string;
}

export function sessionCommands(value: unknown): readonly SessionCommand[] | undefined {
  if (!Array.isArray(value)) return undefined;
  const names = new Set<string>();
  return value.flatMap((item: unknown) => {
    if (typeof item !== 'object' || item === null) return [];
    const { name, description } = item as Record<string, unknown>;
    if (
      typeof name !== 'string' ||
      !/^\$?[^\s/$]+$/u.test(name) ||
      name.length > 512 ||
      typeof description !== 'string' ||
      names.has(name)
    )
      return [];
    names.add(name);
    return [{ name, description }];
  });
}

export function commandPrefix(command: SessionCommand): string {
  // Codex discovers skills as $name and expands that native mention in turn/start.
  return command.name.startsWith('$') ? command.name : `/${command.name}`;
}

export function commandQuery(content: string): string | undefined {
  return /^\/[^\s]*$/u.test(content) ? content.slice(1).toLocaleLowerCase() : undefined;
}

export function matchingCommands(commands: readonly SessionCommand[], query: string) {
  return commands.filter((command) =>
    `${command.name} ${command.description}`.toLocaleLowerCase().includes(query),
  );
}

export function commandRefusal(
  content: string,
  commands: readonly SessionCommand[] | undefined,
  hasAttachments: boolean,
): string | undefined {
  if (!content.startsWith('/')) return undefined;
  const token = content.split(/\s/u, 1)[0]!.slice(1).toLocaleLowerCase();
  const command = commands?.find((item) => item.name.toLocaleLowerCase() === token);
  if (command === undefined || command.name.startsWith('$'))
    return commands === undefined
      ? 'This Agent has not reported its available commands yet. Keep this draft and try again when the session is ready.'
      : 'This command is not available for this Agent. Choose a listed command, or write a message without a leading slash.';
  if (hasAttachments)
    return 'Send attachments with a message or skill invocation, separately from a session command.';
  return undefined;
}
