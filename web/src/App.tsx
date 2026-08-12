import { interfaceCatalogue } from './interfaces';

/**
 * chrono-bot's own frontend is a read-only operational dashboard plus the
 * terminal surface. It is not the primary interaction path — see CLAUDE.md.
 */
export function App() {
  return (
    <main>
      <h1>chrono-bot</h1>
      <ul>
        {interfaceCatalogue.map((iface) => (
          <li key={iface.id}>
            {iface.id} — priority {iface.priority}
          </li>
        ))}
      </ul>
    </main>
  );
}
