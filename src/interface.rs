use serde_json::{json, Value};

pub const UNIT_ID: &str = "soksak-sidecar-workflow";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const INTERFACE_ID: &str = "soksak-spec-sidecar-workflow";
pub const SERVICE_OPS: [&str; 8] = [
    "run",
    "ping",
    "reconcile",
    "research",
    "next",
    "submit",
    "issuerize",
    "export",
];

pub fn handshake() -> Value {
    json!({
        "unit": { "kind": "sidecar", "id": UNIT_ID, "version": VERSION },
        "interface": { "id": INTERFACE_ID, "version": VERSION },
        "transport": "stdio-json-lines",
        "ops": SERVICE_OPS,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_identity_is_exact() {
        let value = handshake();
        assert_eq!(value["unit"]["id"], UNIT_ID);
        assert_eq!(value["unit"]["version"], VERSION);
        assert_eq!(value["interface"]["id"], INTERFACE_ID);
        assert_eq!(value["interface"]["version"], VERSION);
        assert_eq!(
            value["ops"].as_array().map(Vec::len),
            Some(SERVICE_OPS.len())
        );
    }
}
