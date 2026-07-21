# workflow-doc@0.0.1 — field reference

> Single source of truth: `src/doc_interp.rs` (validate + run). This file mirrors it for the authoring LLM.

## Document

| key | type | rules |
|---|---|---|
| `spec` | string | MUST be `"workflow-doc@0.0.1"` |
| `meta.name` | string | non-empty |
| `meta.description` | string | one plain line |
| `args` | object | `{ name: {"from": [key…], "default": any} }` — `from` keys are tried in order against the invocation args; first non-null wins; else `default`; else null. Invocation args always pass through under their own names too. |
| `values` | object | constants. NEVER rendered — `{{…}}` inside a value survives to consumption time. Composition: `{"concat": [part…]}` where part = string literal or `{"$": "values.NAME"}` referencing a **plain string** value (one level only). |
| `prompts` | object | `{ name: template }`. Every `{{ph}}` must be a values key, a declared args key, or `ledger`. |
| `stages` | object | `{ stageName: [op…] }`. `""` = skeleton stage (publish-only — `agent` op rejected there). |

## Ops

### `{"op": "agent", "prompt": P, "schema"?: S, "label"?: L, "bind": V}`
Renders `prompts.P` ({{ph}} → values / args / locals / builtin `ledger`), calls the runner (claude), binds the result JSON to local `V`. `S` must name a values entry that is a JSON object (passed as forced output schema). Runner failure PROPAGATES — the stage exits non-zero (no silent empty success).

### `{"op": "forEach", "in": PATH, "when"?: PATH, "collect"?: V, "do": [op…]}`
`in` must resolve to an array (null/missing = empty). Binds `item` and `index` per element. `when` skips elements whose path is falsy (null/false/""/0). `collect` binds local `V` to the array of node ids published inside the loop (for later `blockedBy`). Note: `index` is the ORIGINAL array index (filtered elements still advance it).

### `{"op": "publish", "node": {…}}`
Emits one kanban node event (same wire as the legacy interp path). Fields — every value may be a literal or `{"$": path, "or": default}`:

| field | notes |
|---|---|
| `id` | literal string, or `{"auto": "prefix"}` inside forEach → `prefix + index`. Literal ids must be unique per stage. |
| `kind` | required — `chunk` / `item` / `task` (draft model) |
| `parent` | parent ref — a local emit id (same run) or an existing kanban id (e.g. `{"$": "args.chunkRef", "or": "chunk"}`) |
| `title` `description` `origin` `badge` `category` | strings |
| `stage` | task nodes only — which stage the scheduler will exec-stage |
| `blockedBy` | array; element = literal id or `{"$": path}` — an array value is spread inline |
| `schema` | values key (string, must name an object) or inline object — per-node output contract |
| `promptRole` | logical role label (relay maps role → registered prompt hash) |
| `vars` | `{k: expr}` — SMALL per-node values only (title/description). Big shared text goes through registerPromptsOnce + varRefs. |
| `varRefs` | `{templateKey: roleLabel}` — consumption-time content-address deref (e.g. `{"directive": "directive"}`) |
| `registerPromptsOnce` | `{role: expr}` — attached to the FIRST published node of this run only; relay content-addresses each value (sha256 dedup) |
| `isDraft` | bool — draft chunk marker |
| `parentDraftId` | string — draft lineage |

### `{"op": "return", "value": {k: expr}}`
Ends the stage; the object becomes the stage result (`{ev:"result"}` for stream stages; folded into the DraftDoc for `generate`).

## Builtins

- `{{ledger}}` (prompts only) — renders `args.ledger` entries as `- [id] [badge] (category?) title | 근거: verified_value?` lines, badge falling back to `검수전`.

## Execution contract (what the executor guarantees)

- `--emit` runs stage `""` — LLM is never called; events stream to stdout as `{"ev":"add",…}` lines.
- `exec-stage` runs a named stage — `generate` output is folded into a normalized DraftDoc and **validated (violations = loud rejection, nothing published)**; other stages stream events plus a final `{"ev":"result","value":…}`.
- Document validation runs at authoring, `--emit`, and `exec-stage`. Unknown ops, dangling prompt/schema references, unresolvable placeholders, duplicate ids — all rejected with the exact violation listed.
