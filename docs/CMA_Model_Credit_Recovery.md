# Managed model credit refusal and recovery

An admitted turn can fail when a later model request receives HTTP 402 with
the gateway error `insufficient_credits` and diagnostic code `11300`. Admission
and Sandbox readiness do not prove that the model billing account can fund every
subsequent request. This result is independent of CMA's disabled compute billing.

The managed model proxy installs the Sandbox owner's scoped credential on the
outbound request. The profile does not select a shared CMA wallet. A model token
allowance or a grant to a different account is not proof that the account used
by the request has spendable billing credits. Wallet values and incident
identities belong in private operational evidence, never GitHub comments.

## Terminal contract

The adapter recognizes the observed HTTP status and structured error together.
It publishes the stable failure class `insufficient_credits` with a safe message
that names HTTP 402 and explains the next action. Unrelated errors retain their
existing classification. A warning from a successfully recovered provider retry
does not turn a successful response into a failure.

The existing admitted turn ends as failed. AG-UI emits `RUN_ERROR`; Responses
emits `response.failed` and retrieval returns `status=failed` with
`error.code=insufficient_credits`. An already accepted HTTP stream retains its
original transport status; the terminal event carries the model's 402 outcome.
The first-message record retains the same failure class. Shared readers receive
static recovery guidance instead of private upstream diagnostics.

The Agent becomes Ready on its original conversation. Its failed receipt,
history, prior output and already executed effects remain recorded. The failure
does not queue another attempt, create a replacement Agent, or reset the home.
Reusing the same command key returns the original failed receipt.

## Recovery

1. Retrieve the original response and confirm its terminal failure class.
   Preserve its command/response identity for private correlation. Do not
   resubmit uncertain or failed work merely to obtain another diagnostic.
2. The Sandbox owner or authorized funding operator checks the billing account
   actually used by managed model requests and restores spendable credit there.
   Pending debits and reservations can make an apparent token grant inadequate.
   No billing flag needs to be enabled in CMA to perform this recovery.
3. After funding is confirmed, the user explicitly sends a new continuation on
   the same Agent. Review any already executed effects before choosing the new
   input. Use a new command key for this new intent; the old receipt stays failed.
4. Confirm that the new turn completes and that the old receipt/history remain
   unchanged. A continuing refusal requires private gateway attribution of the
   new request, rather than automatic replay of prior user work.

Historical receipts are not rewritten. A retained generic `agent_turn_failed`
receipt may still carry the original gateway diagnostic for its authorized
owner. New typed failures require the updated control plane.

## Release verification

Production doubles as staging. The release operator performs this procedure;
cma-cost does not access production. Use a dedicated test owner and controlled
failure fixtures. Do not deplete shared funds or change global thresholds.

- Confirm an ordinary funded turn succeeds.
- Exercise the exact refusal before the first response and after partial work.
  Check terminal event, retrieval, first-message state and absence of indefinite
  Thinking. Preserve prior tool effects and output.
- Replay the original key and refresh/reconnect. Confirm the same failed receipt
  and no additional execution or reservation.
- Restore the test owner's model funding through the authorized private channel.
  Submit one explicit new continuation and confirm successful completion on the
  original Agent/conversation, with the earlier receipt still failed.
- Verify that a different owner and unrelated active work are unaffected, and
  that all paid-billing flags remain disabled.

Record source SHA, image digests, environment, UTC window, sample count, relevant
configuration revision/flags and the private raw-evidence location. Publish only
aggregate pass/fail facts. Local fixtures validate lifecycle and projection;
they do not establish a production top-up or gateway funding proof. Gate #1546
and #1548 also require their separate reservation and spend-control evidence.
