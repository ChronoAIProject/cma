"""Resume resource obligations before revoking the control sessions."""

import grant_cleanup
import launch_recovery
import retained
import identity
from safety import Refusal


def resources(p):
    failed = False
    with p.http.cleanup_budget():
        # Continue independent teardown obligations if one service is unavailable.
        tasks = [lambda: retained.reconcile(p), lambda: grant_cleanup.run(p)]
        tasks += [lambda a=a: launch_recovery.cleanup(p, a) for a in ("a", "b")]
        for task in tasks:
            try:
                task()
            except Exception:
                failed = True
        if failed:
            raise Refusal("resource_cleanup_pending; original_private_journals_required")


def run(p):
    identity.bind_live(p)
    resources(p)
    with p.http.cleanup_budget():
        retained.verify_expiration(p)
    # If resource cleanup needs credentials, leave them journaled for resumption.
    # Expiration of an existing report is passive; its TTL does not prevent logout.
    import finish
    finish.run(p)
