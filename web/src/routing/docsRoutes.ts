export const DOCS_PATHS = {
  'start-a-task': '/docs/start-a-task',
  'environments-and-capacity': '/docs/environments-and-capacity',
  'recover-and-continue': '/docs/recover-and-continue',
  'find-your-outputs': '/docs/find-your-outputs',
  event_gateway_protocol: '/docs/event-gateway',
} as const;

export type DocsView = keyof typeof DOCS_PATHS;
export type HelpTopic = Exclude<DocsView, 'event_gateway_protocol'>;

export function docsView(path: string): DocsView | undefined {
  return (Object.keys(DOCS_PATHS) as DocsView[]).find((view) => DOCS_PATHS[view] === path);
}
