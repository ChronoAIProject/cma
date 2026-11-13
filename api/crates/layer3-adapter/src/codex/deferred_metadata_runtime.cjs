// Embedded in the pinned provider; only advisory load metadata uses this queue.
function createDeferredMetadata({ current, commands, goal, report }, limit = 32) {
  const jobs = new Map();
  return function schedule(state, generation) {
    const job = { state, generation };
    const live = value => current(value.state, value.generation);
    if (!live(job)) return;
    const previous = jobs.get(state.sessionId);
    if (previous) {
      if (previous.active.state !== state || previous.active.generation !== generation)
        previous.next = job;
      return;
    }
    if (jobs.size >= limit) { report("capacity"); return; }
    const entry = { active: job, next: null };
    jobs.set(state.sessionId, entry);
    // Yield until the load handler has delivered history and returned its reply.
    setImmediate(async () => {
      try {
        while (entry.active) {
          const active = entry.active;
          const isCurrent = () => live(active);
          try {
            if (isCurrent()) await commands(active.state, isCurrent);
            if (isCurrent()) await goal(active.state, active.generation);
          } catch { report("failed"); }
          entry.active = entry.next;
          entry.next = null;
        }
      } finally { jobs.delete(state.sessionId); }
    });
  };
}

module.exports = { createDeferredMetadata };
