import { routePath, type AppRoute } from '../routing/routes';

export function SandboxLink({
  id,
  name,
  onNavigate,
}: {
  readonly id: string;
  readonly name: string;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const route = { kind: 'sandbox', sandboxId: id } as const;
  return (
    <a
      href={routePath(route)}
      onClick={(event) => {
        if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey)
          return;
        event.preventDefault();
        onNavigate(route);
      }}
    >
      {name}
    </a>
  );
}
