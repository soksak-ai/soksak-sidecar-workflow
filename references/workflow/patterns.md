# workflow-doc@0.0.1 — authoring patterns

> Shapes the draft pipeline uses; compose them for new roles.

## Stage routing is the stages map — no branching code

The legacy JS path routed with `if (STAGE === …)` blocks. In a doc, each stage is simply a key in `stages`. The scheduler invokes one stage per task node (`task.stage` field); the skeleton stage `""` publishes the initial chunk + first task.

## Skeleton stage: chunk + first task

```json
"": [
  { "op": "publish", "node": { "id": "chunk", "kind": "chunk", "isDraft": true,
      "title": {"$": "args.title", "or": "구체화 덩어리"}, "description": {"$": "args.directive"},
      "parentDraftId": {"$": "args.parentDraftId", "or": ""} } },
  { "op": "publish", "node": { "id": "gen", "kind": "task", "stage": "generate", "parent": "chunk", "title": "요건 도출" } }
]
```

## Fan-out publish with ordering chain

`agent` → `forEach` over the result array → publish one node per element → follow-up task nodes gated by `blockedBy` on the collected ids:

```json
"generate": [
  { "op": "agent", "prompt": "gen", "schema": "GEN_SCHEMA", "bind": "tree" },
  { "op": "forEach", "in": "tree.requirements", "when": "item.title", "collect": "itemIds",
    "do": [ { "op": "publish", "node": { "id": {"auto": "i"}, "kind": "item", "…": "…" } } ] },
  { "op": "publish", "node": { "id": "hunt", "kind": "task", "stage": "hunt", "blockedBy": [ {"$": "itemIds"} ], "…": "…" } },
  { "op": "return", "value": { "chunkTitle": {"$": "tree.title", "or": ""} } }
]
```

Chained gates append literals after the spread: `"blockedBy": [ {"$": "itemIds"}, "hunt" ]`, then `[ {"$": "itemIds"}, "hunt", "classify" ]` — the DAG order lives in data, not control flow.

## Prompt normalization (3-level content addressing)

Never inline the full verify prompt per item. Split three ways:
1. **Template** (global, byte-stable): `values.VERIFY_TMPL` with `{{title}}/{{description}}/{{directive}}` consumption-time markers — registered once via the first item's `registerPromptsOnce`.
2. **Big per-chunk value**: the directive — registered alongside (`"directive": {"$": "args.directive"}`), referenced per item via `varRefs: {"directive": "directive"}`.
3. **Small per-item values**: `vars: {"title": …, "description": …}` only.

Compose the template from a shared fragment without duplicating it in the doc:

```json
"values": { "COMMON": "…big shared text…",
            "VERIFY_TMPL": { "concat": [ {"$": "values.COMMON"}, "\n\nYOUR ROLE — VERIFIER …" ] } }
```

## Ledger-consuming stages

Stages that review the whole set (`hunt`/`classify`/`audit`) take the materialized ledger via `{{ledger}}` in their prompt and publish additions or just `return` a result. `return {}` is valid (hunt's additions travel as published events, not the result).

## Fail-loud is the default

Don't add defensive fallbacks in the doc. If a reference or placeholder is wrong, validation rejects the document with the exact violation — that is the designed failure surface. A doc that "works around" its own typos hides bugs.
