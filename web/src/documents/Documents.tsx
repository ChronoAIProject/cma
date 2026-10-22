import { useCmaClient } from '../api/context';
import type { AppRoute } from '../routing/routes';
import type { User } from '../auth/types';
import { DocumentRegistry } from './DocumentRegistry';
import { DocumentPage } from './DocumentPage';
import './Documents.css';
export default function Documents({
  route,
  user,
  onNavigate,
}: {
  readonly route: Extract<AppRoute, { kind: 'artifacts' | 'document' }>;
  readonly user: User;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const client = useCmaClient().documents;
  if (!client)
    return (
      <section className="documents-page">
        <h1>Artifacts</h1>
        <p>Documents are unavailable.</p>
      </section>
    );
  return route.kind === 'document' ? (
    <DocumentPage
      key={route.documentId}
      client={client}
      id={route.documentId}
      viewer={user.id}
      onNavigate={onNavigate}
    />
  ) : (
    <DocumentRegistry client={client} canCreate={!user.visitor} onNavigate={onNavigate} />
  );
}
