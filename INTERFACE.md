# Workflow sidecar interface 0.0.1

Provider identity:

```json
{"id":"soksak-spec-sidecar-workflow","version":"0.0.1"}
```

The process transport is UTF-8 JSON Lines over stdio. The platform-level resident service
frames are owned by `soksak-spec-service` at commit
`d7f54852754195527f125d1fc11362316157d19b`. This repository owns the workflow behavior of
the declared operations: `run`, `ping`, `reconcile`, `research`, `next`, `submit`,
`issuerize`, `export`, and `proof`.

The binary exposes a pre-start handshake:

```sh
soksak-sidecar-workflow --handshake
```

It returns one JSON object containing the unit identity, interface provider, transport, and
the exact operation list. Exit status is zero and stderr is empty.

The default `draft` and `research` workflow documents and `draft-skill.md` are embedded.
Named lookup accepts only ASCII alphanumeric, `-`, and `_`, and only declared names resolve.
An explicit `--refs <directory>` may replace the authoring reference during development.

Run-catalog location is declared by `SOKSAK_SIDECAR_WORKFLOW_RUNS`, or by the platform-owned
`SOKSAK_HOME/runs/soksak-sidecar-workflow` convention. The directory chain must contain only
regular directories. Every run writes an immutable `<time>-<pid>-<sequence>.jsonl`; the
regular `latest.json` document has shape `{"stream":"<filename>.jsonl"}`.
