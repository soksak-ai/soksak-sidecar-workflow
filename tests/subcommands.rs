//! build-ledger·validate-draft 서브커맨드 통합 테스트 — app-0 러너(run-full-chain)가 소비하는
//! CLI 미러. 로직 단일진실은 lib(reconcile::build_ledger·draft_doc::validate)이고 유닛이 그걸
//! 덮는다 — 여기서는 stdin/stdout JSON 왕복(얇은 CLI 글루)만 프로세스 경계에서 고정한다.

use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Stdio};

fn run(args: &[&str], stdin: &str) -> (bool, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_soksak-sidecar-workflow"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("스폰");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("대기");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

#[test]
fn build_ledger_mirrors_reconcile_ledger() {
    // item a(badge o)가 chunk 직속 → 원장에 badge 보존. 비-item(chunk 자신)은 제외.
    let stdin = json!({
        "nodes": [
            { "id": "a", "kind": "item", "parentId": "chunk", "badge": "o" },
            { "id": "b", "kind": "item", "parentId": "chunk", "badge": "검수전" },
            { "id": "chunk", "kind": "chunk" }
        ]
    })
    .to_string();
    let (ok, out) = run(
        &["build-ledger", "--chunk", "chunk", "--kind", "item"],
        &stdin,
    );
    assert!(ok, "build-ledger 성공");
    let ledger: Vec<Value> = serde_json::from_str(out.trim()).expect("원장 JSON");
    assert_eq!(ledger.len(), 2, "item 2개");
    let ids: Vec<&str> = ledger.iter().filter_map(|e| e["id"].as_str()).collect();
    assert!(
        ids.contains(&"a") && ids.contains(&"b"),
        "item id 보존: {ids:?}"
    );
    let a = ledger.iter().find(|e| e["id"] == "a").unwrap();
    assert_eq!(a["badge"], "o", "badge 보존");
}

#[test]
fn validate_draft_reports_no_violations_for_a_valid_doc() {
    let doc = json!({
        "kind": "draft-chunk",
        "chunk_ref": "c1",
        "verify_contract": { "template": "t", "directive": "d", "schema": {}, "initial_badge": "검수전" },
        "requirements": [{ "id": "r1", "title": "T", "description": "D", "origin": "user", "badge": "검수전" }],
        "tasks": []
    })
    .to_string();
    let (ok, out) = run(&["validate-draft"], &doc);
    assert!(ok, "validate-draft 성공");
    let v: Value = serde_json::from_str(out.trim()).expect("violations JSON");
    assert_eq!(
        v["violations"].as_array().map(|a| a.len()),
        Some(0),
        "유효 doc → 위반 0: {out}"
    );
}
