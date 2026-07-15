# soksak-sidecar-workflow

The private native workflow runtime for soksak. It owns execution of `workflow-doc@0.0.1`,
the resident service implementation, provider process supervision, and the
`soksak-spec-sidecar-workflow` interface. Plugin manifests and UI remain owned by the
workflow plugin repository.

## Boundaries

- The platform service wire and harness come from public `soksak-spec` at the exact Git
  commit declared in `Cargo.toml` and `validation/spec-validator.json`.
- The sidecar-owned payload, commands, bundled workflow documents, and conformance tests
  live here.
- Workflow documents and the default authoring reference are compiled into the binary.
  Runtime behavior does not depend on the executable location or current directory.
- `--refs <directory>` is an explicit development override. No implicit local path is
  searched.
- Run streams are immutable regular files. `latest.json` is a regular JSON pointer naming
  the latest stream under the configured run directory.

## Interfaces

`soksak-sidecar-workflow --handshake` reports the unit and domain interface without
starting a provider. `soksak-sidecar-workflow serve` starts the public
`soksak-spec-service` NDJSON protocol. Standalone `exec-one`, `exec-stage`, `synth`,
`build-ledger`, and `validate-draft` commands are deterministic entry points around the
same implementation.

See [INTERFACE.md](INTERFACE.md) for the exact contract.

## Development

```sh
make test-unit
```

The release workflow builds five declared desktop targets and publishes only immutable
GitHub Release assets for `v0.0.1`. Cargo and npm registries are not publication surfaces.
