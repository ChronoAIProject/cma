export function publicApiPath(path: string): string {
  return path.replace(/^\/api\/agent-manager\/v1\//, '/api/v1/');
}
