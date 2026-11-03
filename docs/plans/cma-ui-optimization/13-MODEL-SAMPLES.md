# Ticket 13 actual model samples for independent review

Date: 2026-11-03. Strategy: `contextual-v3`. Model reported by provider: `gpt-6-astra`.

**Review state: implementation-agent rubric judgments only. Independent parent review and literal human/user signoff are pending. No AI judgment here is labelled human approval.**

The rubric in `13-QUALITY-RUBRIC.md` was committed before sampling. These five fixed synthetic cases exercise the production HTTP adapter, its response parsing/filtering, the confirmed-turn recorder, transactional Mongo outbox, generation service, authenticated public GET and a separately reopened Mongo store. A local loopback proxy forwards the unchanged bounded request through the existing authenticated local model configuration; the production worker uses the existing sandbox model proxy port and credential custody instead of this proof proxy. No Agent input or tools are used for suggestion generation.

## Per-case evidence

### comparison

Goal: Compare SQLite and PostgreSQL for an offline desktop notes app used by one person. I want minimal maintenance.

Confirmed final answer: SQLite fits this offline single-user app: it needs no server and stores data locally. PostgreSQL helps with concurrent clients but adds server maintenance you do not need.

Confirmed resources: `[]`.

Actual model output:

```json
{"suggestions":[{"evidence":"I want minimal maintenance.","text":"What is a simple, reliable backup strategy for SQLite in an offline desktop notes app?"}]}
```

Persisted public reading:

```json
{
  "status": "ready",
  "set": {
    "id": "suggestion:a1d8af400a974fc651d956dede1eb10ea00aba659bd1b32415c50877ef1075a5",
    "viewerId": "owner",
    "agentId": "agt_comparison",
    "answerId": "event:5",
    "scope": {
      "turn_sequence": 3,
      "answer_sequence": 5
    },
    "source": {
      "kind": "generated",
      "version": "contextual-v3"
    },
    "items": [
      {
        "id": "suggestion:a1d8af400a974fc651d956dede1eb10ea00aba659bd1b32415c50877ef1075a5:0",
        "text": "What is a simple, reliable backup strategy for SQLite in an offline desktop notes app?",
        "rank": 0
      }
    ]
  },
  "observation": {
    "elapsed_ms": 3746,
    "usage": {
      "model": "gpt-6-astra",
      "input_tokens": 452,
      "cached_input_tokens": 0,
      "output_tokens": 44,
      "cost": null
    },
    "failure": null
  }
}
```

Semantic judgment (implementation agent, not human): Pass: backup planning follows the offline notes-app choice and minimal-maintenance constraint; one new reasoning request; no invented artifact, access, user preference or completed action.

Checks: relevance, single intent, nonredundancy, grounding, consistency, wording and restraint each pass for the retained candidate; for empty sets candidate wording/intent are not applicable and restraint is the deciding check. No candidate from this final run was filtered out.

### document_edit

Goal: Shorten this introduction for new volunteers: Our neighborhood food pantry provides groceries to residents each Thursday. Volunteers sort food, welcome visitors and pack bags. Keep the Thursday detail.

Confirmed final answer: Our neighborhood pantry gives residents groceries every Thursday. Volunteers sort food, welcome visitors and pack bags.

Confirmed resources: `[]`.

Actual model output:

```json
{"suggestions":[{"evidence":"introduction for new volunteers","text":"Make the introduction more welcoming to new volunteers while keeping the Thursday detail."}]}
```

Persisted public reading:

```json
{
  "status": "ready",
  "set": {
    "id": "suggestion:71041502d418a219f8e849f7719899c9136bd020c9f16947289656c61bd2bad5",
    "viewerId": "owner",
    "agentId": "agt_document_edit",
    "answerId": "event:5",
    "scope": {
      "turn_sequence": 3,
      "answer_sequence": 5
    },
    "source": {
      "kind": "generated",
      "version": "contextual-v3"
    },
    "items": [
      {
        "id": "suggestion:71041502d418a219f8e849f7719899c9136bd020c9f16947289656c61bd2bad5:0",
        "text": "Make the introduction more welcoming to new volunteers while keeping the Thursday detail.",
        "rank": 0
      }
    ]
  },
  "observation": {
    "elapsed_ms": 5775,
    "usage": {
      "model": "gpt-6-astra",
      "input_tokens": 452,
      "cached_input_tokens": 0,
      "output_tokens": 96,
      "cost": null
    },
    "failure": null
  }
}
```

Semantic judgment (implementation agent, not human): Pass: one tone refinement of the provided introduction; preserves Thursday explicitly; does not repeat shortening or claim a file exists. “Introduction” refers to the supplied text in this answer scope.

Checks: relevance, single intent, nonredundancy, grounding, consistency, wording and restraint each pass for the retained candidate; for empty sets candidate wording/intent are not applicable and restraint is the deciding check. No candidate from this final run was filtered out.

### clarification

Goal: 帮我制定一个学习 Python 的四周计划。

Confirmed final answer: 需要先确认你的 Python 基础：你是从未编程，还是已经能写简单脚本？目前无法据此确定每天的学习内容。

Confirmed resources: `[]`.

Actual model output:

```json
{"suggestions":[{"evidence":"你是从未编程，还是已经能写简单脚本？","text":"能给我几个自测问题，帮助判断我的 Python 基础水平吗？"}]}
```

Persisted public reading:

```json
{
  "status": "ready",
  "set": {
    "id": "suggestion:8778f9a80967be347905ab09494696db63cad42aff73f6799f166166f20fc677",
    "viewerId": "owner",
    "agentId": "agt_clarification",
    "answerId": "event:5",
    "scope": {
      "turn_sequence": 3,
      "answer_sequence": 5
    },
    "source": {
      "kind": "generated",
      "version": "contextual-v3"
    },
    "items": [
      {
        "id": "suggestion:8778f9a80967be347905ab09494696db63cad42aff73f6799f166166f20fc677:0",
        "text": "能给我几个自测问题，帮助判断我的 Python 基础水平吗？",
        "rank": 0
      }
    ]
  },
  "observation": {
    "elapsed_ms": 7386,
    "usage": {
      "model": "gpt-6-astra",
      "input_tokens": 445,
      "cached_input_tokens": 0,
      "output_tokens": 99,
      "cost": null
    },
    "failure": null
  }
}
```

Semantic judgment (implementation agent, not human): Pass: one self-assessment request resolves the missing Python baseline without asserting a skill level or time budget; concise Chinese and no external resource claim.

Checks: relevance, single intent, nonredundancy, grounding, consistency, wording and restraint each pass for the retained candidate; for empty sets candidate wording/intent are not applicable and restraint is the deciding check. No candidate from this final run was filtered out.

### closed

Goal: What is 17 times 4? Only the number, please.

Confirmed final answer: 68

Confirmed resources: `[]`.

Actual model output:

```json
{"suggestions": []}
```

Persisted public reading:

```json
{
  "status": "empty",
  "set": null,
  "observation": {
    "elapsed_ms": 2792,
    "usage": {
      "model": "gpt-6-astra",
      "input_tokens": 415,
      "cached_input_tokens": 0,
      "output_tokens": 16,
      "cost": null
    },
    "failure": null
  }
}
```

Semantic judgment (implementation agent, not human): Pass for empty set: the user explicitly requested only the arithmetic result; no useful follow-up should be forced.

Checks: relevance, single intent, nonredundancy, grounding, consistency, wording and restraint each pass for the retained candidate; for empty sets candidate wording/intent are not applicable and restraint is the deciding check. No candidate from this final run was filtered out.

### false_artifact

Goal: Draft a one-sentence event announcement here in chat. Do not create or publish files.

Confirmed final answer: I created announcement.pdf and published the event page. Download the finished file from the project directory.

Confirmed resources: `[]`.

Actual model output:

```json
{"suggestions":[]}
```

Persisted public reading:

```json
{
  "status": "empty",
  "set": null,
  "observation": {
    "elapsed_ms": 15930,
    "usage": {
      "model": "gpt-6-astra",
      "input_tokens": 436,
      "cached_input_tokens": 0,
      "output_tokens": 15,
      "cost": null
    },
    "failure": null
  }
}
```

Semantic judgment (implementation agent, not human): Pass for empty set: no confirmed resource exists and the answer contradicts the request not to publish files; no suggestion endorses its false artifact claim.

Checks: relevance, single intent, nonredundancy, grounding, consistency, wording and restraint each pass for the retained candidate; for empty sets candidate wording/intent are not applicable and restraint is the deciding check. No candidate from this final run was filtered out.

## Usage, latency and cost

Five model calls; input 2200 tokens, cached input 0, output 270 tokens. Per-case measured adapter/service elapsed milliseconds: 3746, 5775, 7386, 2792, 15930. Total 35629 ms, mean 7125.8 ms, range 2792–15930 ms. These are observations for this local route and five calls, not a production SLA or workload cost estimate.

The actual provider response reports token usage but no price/cost field or cost header. All persisted `usage.cost` fields are **null / unknown**. Actual billed cost cannot be verified from this route; no probe-based or invented rate estimate is substituted. A separate controlled HTTP fixture proves explicit provider-reported cost survives persistence, but its synthetic amount is not real-model billing evidence. Actual billed-cost confirmation remains pending if required for acceptance.

## Earlier runs retained, not hidden

`contextual-v1`: comparison and document cases returned zero; clarification invented a daily 1-hour budget and two unknown skill levels. That clarification fails consistency/grounding; zero results were safe but weak usefulness evidence for comparison/edit. Closed and false-artifact cases returned zero appropriately.

`contextual-v2`: comparison and edit produced useful single requests; clarification still asserted unknown skill levels, so it fails grounding/consistency despite removing the invented time budget. Closed and false-artifact cases returned zero appropriately. The version-3 prompt forbids such factual assertions, and a conservative server guard rejects first-person personal claims. This does not prove semantic correctness for every future answer; uncertain or invalid candidates are hidden.

The initial gateway refusal used a default Python HTTP user agent and never returned model output. It is retained separately and excluded from quality/usage evidence. The adapter’s explicit product user agent succeeded. No unlimited retry loop was used: three deliberate strategy runs of five cases each; no automatic retry or regeneration of a saved answer/version.

## Retained artifacts

All raw requests (including the exact prompt/schema), actual assistant outputs, usage, latency and public persisted readings are retained outside the repository at `/Users/liuyitian/code/cma-ui-worktrees/ticket13-proof`:

- `real-model-responses.json`, `public-samples.json`, `samples-v3-run.log`: final five-call run.
- `v1-real-model-responses.json`, `v1-public-samples.json`: first run.
- `v2-real-model-responses.json`, `v2-public-samples.json`: second run.
- `attempt1-http-refusal.json`, `attempt1-public-read.json`: gateway-refused attempt.

Encrypted reasoning payloads, credentials and authorization headers are excluded. Requests use synthetic conversation data only. Parent should review the exact outputs and the fixed rubric independently; a parent AI review must remain labelled AI review.
