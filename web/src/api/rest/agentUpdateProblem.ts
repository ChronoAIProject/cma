/** Preserve machine-readable decisions without carrying arbitrary upstream text into the UI. */
export async function agentUpdateProblemCode(response: Response): Promise<string | undefined> {
  const problem: unknown = await response.json().catch(() => undefined);
  if (!problem || typeof problem !== 'object' || !('code' in problem)) return undefined;
  return typeof problem.code === 'string' && /^[a-z][a-z0-9_]{0,127}$/.test(problem.code)
    ? problem.code
    : undefined;
}
