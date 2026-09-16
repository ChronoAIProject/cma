import { expect, it, vi } from 'vitest';
import { placementCatalog, placementTransport, savedShared } from './placement.test-support';
import { verifyPlacementForSave } from './placementSave';

it('refuses new consent when target authority expires during the final catalog read', async () => {
  const started = Date.now();
  const now = vi.spyOn(Date, 'now').mockReturnValue(started);
  let catalogs = 0;
  const test = placementTransport(undefined, (url) => {
    if (url.pathname.endsWith('/capabilities')) {
      catalogs += 1;
      if (catalogs === 2) now.mockReturnValue(started + 56_000);
      return Response.json(placementCatalog);
    }
    return undefined;
  });
  try {
    await expect(
      verifyPlacementForSave(test.client, savedShared.config, undefined, undefined, undefined),
    ).rejects.toThrow('Shared placement availability or resource access changed.');
    expect(catalogs).toBe(2);
    expect(test.writes).toHaveLength(0);
  } finally {
    now.mockRestore();
  }
});
