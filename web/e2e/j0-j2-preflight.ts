import preflight from './preflight';
import { requireJourneyMutation } from './j0-j2-policy';

export default async function journeyPreflight() {
  requireJourneyMutation();
  // Keep the existing account, credential, provenance and cleanup requirements.
  await preflight();
}
