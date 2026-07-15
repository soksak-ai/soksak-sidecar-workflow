---
name: workflow-doc
description: Author a workflow-doc@0.0.1 — a declarative, language-neutral JSON document that the soksak-workflow executor runs stage by stage. Replaces JS (gen.js) authoring: no syntax risk, schema-validated, fail-loud.
provenance: soksak-workflow doc executor (src/doc_exec.rs) is the single runtime contract. This skill teaches its input format.
---

# workflow-doc@0.0.1 — the document IS the program

A workflow is authored as ONE JSON document. The executor (`soksak-workflow`) runs one **stage** per invocation: the empty stage `""` publishes the initial kanban nodes (`--emit`, never calls an LLM), and named stages (`generate`, `hunt`, …) run under `exec-stage` (LLM allowed). Publishing emits kanban node events; the scheduler executes published nodes later. You never execute anything yourself — you DECLARE.

## Top-level shape

```json
{
  "spec": "workflow-doc@0.0.1",
  "meta":    { "name": "draft", "description": "one line, plain" },
  "args":    { "directive": { "from": ["directive", "DIRECTIVE", "IDEA"], "default": "…" } },
  "values":  { "NAME": "verbatim text or JSON object — NEVER rendered" },
  "prompts": { "name": "rendered at agent time — {{ref}} placeholders" },
  "stages":  { "": [ops…], "generate": [ops…], "hunt": [ops…] }
}
```

- `args` — declares runtime inputs: `from` is a priority list of keys looked up in the invocation args; `default` applies when none present. Runtime-injected keys (`stage`, `chunkRef`, `ledger`, `lang`) pass through without declaration.
- `values` — constants: prompt fragments, JSON Schemas, templates. Values are **verbatim** — `{{…}}` markers inside a value survive untouched (consumption-time substitution elsewhere). A value may be composed once at load: `{"concat": [{"$": "values.COMMON"}, "…suffix…"]}` (references plain string values only).
- `prompts` — templates rendered when an `agent` op runs. `{{name}}` resolves from values, args, bound locals, or the builtin `{{ledger}}` (renders `args.ledger` as the canonical ledger lines). An unresolvable placeholder is a validation error — nothing fails silently.
- `stages` — each key is a stage name; the value is an ordered op list. Stage `""` is the skeleton (publish-only; an `agent` op there is rejected).

## The 4 ops

| op | does | key fields |
|---|---|---|
| `agent` | render a prompt, call the LLM, bind the JSON result | `prompt`(prompts key), `schema`(values key, optional), `label`, `bind`(local name) |
| `forEach` | iterate an array binding `item`/`index` | `in`(path), `when`(path filter), `collect`(gathers published node ids), `do`(ops) |
| `publish` | emit one kanban node event | `node`{…} — see api-reference |
| `return` | end the stage with a result object | `value`{k: expr} |

## Expressions

A field value is either a JSON literal, or a reference `{"$": "path", "or": default}`. Paths are dot chains whose first segment is a bound local (`tree`, `item`, `index`, `itemIds`, …) or the roots `args` / `values`. `or` applies when the path is missing or null. In `blockedBy` arrays, a reference resolving to an array is spread inline.

## Discipline

- Validation is the gate: the document is schema-checked at authoring, at `--emit`, and at `exec-stage`. A violation is a loud rejection — prefer failing here over a runtime surprise.
- Output ONLY the JSON document. No markdown fence, no prose before or after.
