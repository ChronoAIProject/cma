import { useState } from 'react';
import type { OperatorClient } from '../operator/client';
import { useSandboxOwners } from '../operator/useSandboxOwners';
import { SandboxUsagePanel } from './SandboxUsagePanel';

export function AdminSandboxUsage({ client }: { readonly client: OperatorClient }) {
  const [scope, setScope] = useState({ client, generation: 0 });
  if (scope.client !== client) setScope({ client, generation: scope.generation + 1 });
  const owners = useSandboxOwners(client);
  return <SandboxUsagePanel key={scope.generation} client={client.usage} owners={owners} admin />;
}
