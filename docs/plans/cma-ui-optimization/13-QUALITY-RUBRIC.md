# Contextual suggestion rubric fixed before sampling

Date: 2026-11-03. This rubric precedes real model sampling. The availability probe
is excluded from quality, adapter, persistence and cost evidence.

Each candidate must pass every applicable check; zero suggestions is a valid
result. Do not average a serious failure away with a numerical score.

| Check | Pass condition |
| --- | --- |
| Relevance | A specific useful next step follows from the supplied user goal and final answer. |
| Single intent | One independently actionable request; no bundled decisions, questions or unrelated tasks. |
| Nonredundancy | Adds useful information or action beyond the answer; no duplicate or paraphrased candidates. |
| Grounding | No unconfirmed file, artifact, deployment, tool, access, or success claim. Resource references must be in the independently supplied confirmed-resource list. |
| Consistency | Respects explicit user constraints and the answer's uncertainty; does not repeat a refused or completed task. |
| Wording | Exact text is suitable as the user's next message, concise and self-contained, in the conversation's language. |
| Restraint | No generic “continue”, filler or obligation to produce three items. A closed task or unsupported achievement should produce zero unless a clearly grounded next step exists. |

Mandatory synthetic cases: comparison with a concrete constraint; document edit
with supplied text but no published file; clarification of one missing decision;
closed factual answer with no suitable next step; answer falsely claiming a file
or deployment without confirmed resources. Include English and Chinese. Record
the exact bounded request, raw model output, filtered persisted output, model,
strategy version, elapsed time, usage and the provider-reported cost if present.
Missing cost is explicitly unknown, never zero or an invented token-price estimate.

For each case, retain semantic judgments against every check and reasons for any
rejected candidate. These implementation-agent judgments are evidence for review,
not human approval. Parent AI review is separately labelled independent AI review.
Literal human/user signoff remains pending until an actual person supplies it.
