export function LegacyProfileNotice() {
  return (
    <section className="console__empty" aria-labelledby="legacy-profile-title">
      <p className="console__eyebrow">Profiles retired</p>
      <h1 id="legacy-profile-title">Configure Agents directly</h1>
      <p>
        Agent setup is now saved inline with each Agent. Open New agent to configure an Agent. New
        Sandboxes use the deployed runtime image.
      </p>
      <a href="/">Return to Agent Manager</a>
    </section>
  );
}
