use serde_json::{json, Value};
use std::process::Command;

#[test]
fn fresh_process_declares_owned_sidecar_interface() {
    let out = Command::new(env!("CARGO_BIN_EXE_soksak-sidecar-workflow"))
        .arg("--handshake")
        .output()
        .expect("spawn handshake");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stderr.is_empty());
    let actual: Value = serde_json::from_slice(&out.stdout).expect("handshake JSON");
    assert_eq!(
        actual,
        json!({
            "unit": { "kind": "sidecar", "id": "soksak-sidecar-workflow", "version": "0.0.1" },
            "interface": { "id": "soksak-spec-sidecar-workflow", "version": "0.0.1" },
            "transport": "stdio-json-lines",
            "ops": ["run", "ping", "reconcile", "research", "next", "submit", "issuerize", "export"]
        })
    );
}
