const release = /^v?\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;

export function isPromotableVersion(version: string, revision: string): boolean {
  return release.test(version) || version === `git-${revision.slice(0, 12)}`;
}
