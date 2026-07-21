// reconcile.rs 테스트. #[path]로 reconcile 모듈에 포함되어 super::*는 reconcile을 가리킨다.
// 골든 문자열과 봉투 의미론을 byte-for-byte로 고정한다.
// chunk 1: 순수 헬퍼(is_done·pick_ready·build_ledger·exec_result_to_edit·build_add_params·
// resolve_directive·gen_skeleton_args·build_secret_env_map·build_spawn_cmd·lease_active).
use super::*;
use serde_json::json;
use std::cell::RefCell;

// 노드 리터럴 헬퍼 — json! → Node.
fn node(v: Value) -> Node {
    serde_json::from_value(v).expect("node fixture")
}

// ── recording FakeDeps ──────────────────────────────────────
#[derive(Default)]
struct Calls {
    get: Vec<String>,
    edit: Vec<(String, Value)>,
    exec: Vec<String>,
    stage: Vec<String>,
    add: Vec<Value>,
    poke: u32,
    put: Vec<Value>,
}
impl Calls {
    fn edit_of(&self, id: &str) -> Option<&Value> {
        self.edit.iter().find(|(i, _)| i == id).map(|(_, f)| f)
    }
    fn add_find(&self, pred: impl Fn(&Value) -> bool) -> Option<&Value> {
        self.add.iter().find(|p| pred(p))
    }
}

struct FakeDeps {
    nodes: Vec<Node>,
    exec_out: Option<Value>,
    exec_throw: Option<String>,
    staged: Option<StageOut>,
    stage_throw: Option<String>,
    ledger: Option<Result<Vec<Value>, String>>,
    facts: Option<Result<Vec<Value>, String>>,
    resolve_out: Option<Value>,
    get_prompt_out: Option<Value>,
    put_hash: Option<String>,
    assemble_out: Option<Result<Value, String>>,
    stage_with_output_out: Option<Result<StageOut, String>>,
    edit_err_ids: std::collections::HashSet<String>,
    proof_outcomes: Option<std::collections::HashMap<String, CmdOutcome>>,
    calls: RefCell<Calls>,
}

impl FakeDeps {
    fn new(nodes: Vec<Node>) -> Self {
        FakeDeps {
            nodes,
            exec_out: None,
            exec_throw: None,
            staged: None,
            stage_throw: None,
            ledger: None,
            facts: None,
            resolve_out: None,
            get_prompt_out: None,
            put_hash: None,
            assemble_out: None,
            stage_with_output_out: None,
            edit_err_ids: std::collections::HashSet::new(),
            proof_outcomes: None,
            calls: RefCell::new(Calls::default()),
        }
    }
    fn proof(mut self, outcomes: Vec<(&str, CmdOutcome)>) -> Self {
        self.proof_outcomes = Some(
            outcomes
                .into_iter()
                .map(|(c, o)| (c.to_string(), o))
                .collect(),
        );
        self
    }
    fn exec(mut self, out: Value) -> Self {
        self.exec_out = Some(out);
        self
    }
    fn exec_throws(mut self, msg: &str) -> Self {
        self.exec_throw = Some(msg.to_string());
        self
    }
    fn stage(mut self, s: StageOut) -> Self {
        self.staged = Some(s);
        self
    }
    fn ledger(mut self, v: Vec<Value>) -> Self {
        self.ledger = Some(Ok(v));
        self
    }
    fn ledger_throws(mut self, msg: &str) -> Self {
        self.ledger = Some(Err(msg.to_string()));
        self
    }
    fn facts(mut self, v: Vec<Value>) -> Self {
        self.facts = Some(Ok(v));
        self
    }
    fn c(&self) -> std::cell::Ref<'_, Calls> {
        self.calls.borrow()
    }
}

// children/result → StageOut::Children 헬퍼.
fn staged_children(children: Vec<Value>, result: Value) -> StageOut {
    StageOut::Children { children, result }
}

impl Deps for FakeDeps {
    fn list_nodes(&self) -> Vec<Node> {
        self.nodes.clone()
    }
    fn get_node(&self, id: &str) -> Option<Node> {
        self.calls.borrow_mut().get.push(id.to_string());
        self.nodes.iter().find(|n| n.id == id).cloned()
    }
    fn edit_node(&self, id: &str, fields: Value) -> EditResult {
        self.calls.borrow_mut().edit.push((id.to_string(), fields));
        if self.edit_err_ids.contains(id) {
            EditResult::err(format!("edit 실패: {id}"))
        } else {
            EditResult::ok()
        }
    }
    fn add_node(&self, params: Value) -> Option<String> {
        let mut c = self.calls.borrow_mut();
        c.add.push(params);
        Some(format!("k-{}", c.add.len()))
    }
    fn poke(&self) {
        self.calls.borrow_mut().poke += 1;
    }
    fn exec_one(&self, body: &str) -> Result<Value, String> {
        self.calls.borrow_mut().exec.push(body.to_string());
        if let Some(e) = &self.exec_throw {
            return Err(e.clone());
        }
        Ok(self.exec_out.clone().unwrap_or(Value::Null))
    }
    fn exec_stage(&self, body: &str) -> Result<StageOut, String> {
        self.calls.borrow_mut().stage.push(body.to_string());
        if let Some(e) = &self.stage_throw {
            return Err(e.clone());
        }
        Ok(self.staged.clone().expect("staged 미설정"))
    }
    fn materialize_ledger(&self, _chunk_id: &str) -> Result<Vec<Value>, String> {
        self.ledger
            .clone()
            .unwrap_or_else(|| Err("materializeLedger 미설정".into()))
    }
    fn materialize_facts(&self, _chunk_id: &str) -> Result<Vec<Value>, String> {
        self.facts.clone().unwrap_or_else(|| Ok(vec![]))
    }
    fn put_prompt(&self, value: Value) -> Option<String> {
        let mut c = self.calls.borrow_mut();
        c.put.push(value);
        Some(
            self.put_hash
                .clone()
                .unwrap_or_else(|| format!("h-{}", c.put.len())),
        )
    }
    fn resolve_prompt(&self, _hash: &str, _vars: Value, _refs: Value) -> Option<Value> {
        self.resolve_out.clone()
    }
    fn get_prompt(&self, _hash: &str) -> Option<Value> {
        self.get_prompt_out.clone()
    }
    fn has_assemble_stage(&self) -> bool {
        self.assemble_out.is_some()
    }
    fn assemble_stage(&self, _body: &str) -> Result<Value, String> {
        self.assemble_out
            .clone()
            .unwrap_or_else(|| Err("assembleStage 미배선".into()))
    }
    fn has_exec_stage_with_output(&self) -> bool {
        self.stage_with_output_out.is_some()
    }
    fn exec_stage_with_output(&self, _body: &str, _out: Value) -> Result<StageOut, String> {
        self.stage_with_output_out
            .clone()
            .unwrap_or_else(|| Err("execStageWithOutput 미배선".into()))
    }
    fn run_proof_command(&self, _cwd: &str, cmd: &str) -> Option<CmdOutcome> {
        self.proof_outcomes
            .as_ref()
            .and_then(|m| m.get(cmd).cloned())
    }
}
// exit 코드만 지정한 CmdOutcome 헬퍼.
fn outc(code: i64) -> CmdOutcome {
    CmdOutcome {
        exit_code: Some(code),
        stdout: String::new(),
        stderr: String::new(),
    }
}
fn nodes(vs: Vec<Value>) -> Vec<Node> {
    vs.into_iter().map(node).collect()
}
fn ids(ns: &[Node]) -> Vec<String> {
    ns.iter().map(|n| n.id.clone()).collect()
}
fn sorted_ids(ns: &[Node]) -> Vec<String> {
    let mut v = ids(ns);
    v.sort();
    v
}
// Value 배열(원장/document 엔트리)의 id 문자열 추출.
fn ids_of(vs: &[Value]) -> Vec<String> {
    vs.iter()
        .filter_map(|v| v.get("id").and_then(|x| x.as_str()).map(String::from))
        .collect()
}

// ── isDone ───────────────────────────────────────────────────────────────────
#[test]
fn is_done_status_done_only() {
    assert!(is_done(Some(&node(json!({ "id": "a", "status": "done" })))));
    assert!(!is_done(Some(&node(
        json!({ "id": "a", "status": "todo" })
    ))));
    assert!(!is_done(None));
}

#[test]
fn is_done_item_badge_axis() {
    // 항목은 badge o/x/f 가 done(status 축 아님) — ① deadlock 방지.
    let mk =
        |badge: Value| node(json!({ "id": "i", "kind": "item", "badge": badge, "status": "todo" }));
    assert!(is_done(Some(&mk(json!("o")))));
    assert!(is_done(Some(&mk(json!("x")))));
    assert!(is_done(Some(&mk(json!("f")))));
    assert!(
        !is_done(Some(&mk(json!("검수전")))),
        "미검증 항목은 done 아님"
    );
    assert!(
        !is_done(Some(&node(
            json!({ "id": "i", "kind": "item", "status": "todo" })
        ))),
        "badge 없으면 done 아님"
    );
}

// ── pickReady ──────────────────────────────────────────────────────────────
#[test]
fn pick_ready_verified_item_unblocks_hunt() {
    let ns = nodes(vec![
        json!({ "id": "i1", "kind": "item", "badge": "o", "status": "todo", "parentId": "g0", "blockedBy": [] }),
        json!({ "id": "i2", "kind": "item", "badge": "x", "status": "todo", "parentId": "g0", "blockedBy": [] }),
        json!({ "id": "hunt", "kind": "task", "status": "todo", "parentId": "chunk", "blockedBy": ["i1", "i2"] }),
    ]);
    assert_eq!(ids(&pick_ready(&ns)), vec!["hunt"]);
}

#[test]
fn pick_ready_pending_leaf_deps_done() {
    let ns = nodes(vec![
        json!({ "id": "a", "badge": "검수전", "blockedBy": [], "parentId": null, "status": "todo" }),
        json!({ "id": "b", "badge": "o", "blockedBy": [], "parentId": null, "status": "todo" }),
        json!({ "id": "c", "badge": "검수전", "blockedBy": ["a"], "parentId": null, "status": "todo" }),
        json!({ "id": "p", "badge": "검수전", "blockedBy": [], "parentId": null, "status": "todo" }),
        json!({ "id": "ch", "badge": "검수전", "blockedBy": [], "parentId": "p", "status": "todo" }),
    ]);
    assert_eq!(sorted_ids(&pick_ready(&ns)), vec!["a", "ch"]);
}

#[test]
fn pick_ready_blocked_by_done_unblocks() {
    let ns = nodes(vec![
        json!({ "id": "a", "badge": "o", "blockedBy": [], "parentId": null, "status": "done" }),
        json!({ "id": "c", "badge": "검수전", "blockedBy": ["a"], "parentId": null, "status": "todo" }),
    ]);
    assert_eq!(ids(&pick_ready(&ns)), vec!["c"]);
}

#[test]
fn pick_ready_stage_task_by_status() {
    let ns = nodes(vec![
        json!({ "id": "gen", "kind": "task", "status": "todo", "blockedBy": [], "parentId": null }),
        json!({ "id": "aud", "kind": "task", "status": "done", "blockedBy": [], "parentId": null }),
        json!({ "id": "hunt", "kind": "task", "status": "todo", "blockedBy": ["gen"], "parentId": null }),
    ]);
    assert_eq!(ids(&pick_ready(&ns)), vec!["gen"]);
}

#[test]
fn pick_ready_mixed_item_and_stage() {
    let ns = nodes(vec![
        json!({ "id": "gen", "kind": "task", "status": "done", "blockedBy": [], "parentId": null }),
        json!({ "id": "i1", "badge": "검수전", "kind": "item", "status": "todo", "blockedBy": [], "parentId": "g0" }),
        json!({ "id": "hunt", "kind": "task", "status": "todo", "blockedBy": ["gen"], "parentId": null }),
    ]);
    assert_eq!(sorted_ids(&pick_ready(&ns)), vec!["hunt", "i1"]);
}

#[test]
fn pick_ready_empty_safe() {
    assert_eq!(pick_ready(&[]).len(), 0);
}

#[test]
fn pick_ready_audit_gated_by_pending_item() {
    // audit(다른 task 의존)는 덩어리에 검수전 항목 남으면 not-ready (#6 게이트).
    let ns = nodes(vec![
        json!({ "id": "chunk", "kind": "chunk", "parentId": null, "status": "todo" }),
        json!({ "id": "g0", "kind": "group", "parentId": "chunk", "status": "todo" }),
        json!({ "id": "i1", "kind": "item", "parentId": "g0", "badge": "o", "blockedBy": [], "status": "todo" }),
        json!({ "id": "hunt", "kind": "task", "parentId": "chunk", "blockedBy": ["i1"], "status": "done" }),
        json!({ "id": "add0", "kind": "item", "parentId": "chunk", "badge": "검수전", "blockedBy": [], "status": "todo" }),
        json!({ "id": "audit", "kind": "task", "parentId": "chunk", "blockedBy": ["i1", "hunt"], "status": "todo" }),
    ]);
    assert_eq!(sorted_ids(&pick_ready(&ns)), vec!["add0"]);
}

#[test]
fn pick_ready_audit_ready_when_no_pending() {
    let ns = nodes(vec![
        json!({ "id": "chunk", "kind": "chunk", "parentId": null, "status": "todo" }),
        json!({ "id": "i1", "kind": "item", "parentId": "chunk", "badge": "o", "blockedBy": [], "status": "todo" }),
        json!({ "id": "hunt", "kind": "task", "parentId": "chunk", "blockedBy": ["i1"], "status": "done" }),
        json!({ "id": "add0", "kind": "item", "parentId": "chunk", "badge": "x", "blockedBy": [], "status": "todo" }),
        json!({ "id": "audit", "kind": "task", "parentId": "chunk", "blockedBy": ["i1", "hunt"], "status": "todo" }),
    ]);
    assert_eq!(ids(&pick_ready(&ns)), vec!["audit"]);
}

// ── buildLedger ────────────────────────────────────────────────────────────
#[test]
fn build_ledger_flat_descendants_items() {
    let ns = nodes(vec![
        json!({ "id": "chunk", "kind": "chunk", "parentId": null }),
        json!({ "id": "i1", "kind": "item", "parentId": "chunk", "title": "재고 차감", "description": "수량 확정 시 재고를 원자적으로 차감한다", "badge": "o", "category": "재고 관리" }),
        json!({ "id": "i2", "kind": "item", "parentId": "chunk", "title": "창고 연결", "badge": "검수전" }),
        json!({ "id": "other", "kind": "item", "parentId": "other-chunk", "title": "남의 항목", "badge": "o" }),
        json!({ "id": "gen", "kind": "task", "parentId": "chunk" }),
    ]);
    let ledger = build_ledger(&ns, "chunk", "item");
    assert_eq!(ledger.len(), 2);
    assert_eq!(
        ledger[0],
        json!({ "id": "i1", "title": "재고 차감", "description": "수량 확정 시 재고를 원자적으로 차감한다", "badge": "o", "category": "재고 관리" })
    );
    assert_eq!(
        ledger[1],
        json!({ "id": "i2", "title": "창고 연결", "description": null, "badge": "검수전", "category": null })
    );
}

// ── execResultToEdit ────────────────────────────────────────────────────────
#[test]
fn exec_result_to_edit_valid_oxf() {
    assert_eq!(
        exec_result_to_edit(&json!({ "oxf": "o", "result": { "reason": "실재" } }), None),
        json!({ "badge": "o", "result": json!({ "reason": "실재" }).to_string() })
    );
    assert_eq!(
        exec_result_to_edit(&json!({ "oxf": "f", "result": "치명" }), None),
        json!({ "badge": "f", "result": "치명" })
    );
}

#[test]
fn exec_result_to_edit_no_oxf() {
    let e = exec_result_to_edit(&json!({ "oxf": null, "result": { "items": [1, 2] } }), None);
    assert!(e.get("badge").is_none());
    assert_eq!(e["result"], json!({ "items": [1, 2] }).to_string());
}

// 합의 add-history clobber 회귀 — add 로 생성된 검수전 프레임 result 는 {reason,history}. 그 프레임이
// per-item 검증되면 exec_result_to_edit 가 result 를 verdict 로 덮어썼다. 이제 history 를 보존·누적한다.
#[test]
fn exec_result_to_edit_preserves_consensus_history() {
    // build_consensus_create 가 실은 result 형상 — add 결정 이력을 담는다.
    let prior = json!({
        "history": [{ "round": 1, "action": "add", "reason": "중심 엔티티 입력면" }]
    })
    .to_string();
    let edit = exec_result_to_edit(
        &json!({ "oxf": "o", "result": { "reason": "실재 확인" } }),
        Some(&prior),
    );
    assert_eq!(edit["badge"], "o", "verdict badge 유지");
    let result: serde_json::Value =
        serde_json::from_str(edit["result"].as_str().unwrap()).expect("result 는 JSON");
    let history = result["history"].as_array().expect("history 보존");
    assert_eq!(history.len(), 2, "add 이력 + verify 누적: {history:?}");
    assert_eq!(history[0]["action"], "add", "add 이력 소실 금지");
    assert_eq!(history[1]["action"], "verify", "verdict 를 history 로 누적");
    assert_eq!(history[1]["verdict"], "o");
    assert_eq!(
        result["reason"], "실재 확인",
        "verdict reason 도 유지(issuerize)"
    );
}

// ── buildAddParams ──────────────────────────────────────────────────────────
#[test]
fn build_add_params_item_body_is_exec_input() {
    let ev = json!({ "id": "i1", "kind": "item", "title": "재고 차감", "description": "주문 시 차감", "prompt": "verify…", "schema": { "type": "object" }, "badge": "검수전" });
    let p = build_add_params(&ev, Some("k-1"), &[], None, &HashMap::new());
    assert_eq!(p["title"], "재고 차감");
    assert_eq!(p["parentId"], "k-1");
    assert_eq!(p["kind"], "item");
    assert_eq!(p["badge"], "검수전");
    assert_eq!(p["description"], "주문 시 차감");
    let body: Value = serde_json::from_str(p["body"].as_str().unwrap()).unwrap();
    assert_eq!(
        body,
        json!({ "prompt": "verify…", "schema": { "type": "object" } })
    );
    assert_eq!(p["locked"], true);
}

#[test]
fn build_add_params_empty_tier_is_no_tier() {
    // forEach 의 "or":"" 로 effort/model 미emit item → 빈 문자열. tier 아님 → params 에 키 없음.
    // (삽입하면 node.effort=Some("") 가 with_routing 으로 exec body 를 오염 → 기본 최고를 덮는다.)
    let ev = json!({ "id": "i1", "kind": "item", "title": "t", "prompt": "p", "effort": "", "model": "" });
    let p = build_add_params(&ev, Some("k-1"), &[], None, &HashMap::new());
    assert!(
        p.get("effort").is_none(),
        "빈 effort 는 tier 아님 → 미삽입(기본 최고 보존)"
    );
    assert!(p.get("model").is_none(), "빈 model 는 tier 아님 → 미삽입");
}

#[test]
fn build_add_params_nonempty_tier_passthrough() {
    // 저작이 실은 실제 tier → params 로 흐른다(reconcile 이 exec 에 honor).
    let ev = json!({ "id": "i1", "kind": "item", "title": "t", "prompt": "p", "effort": "high", "model": "gpt-5.6-sol" });
    let p = build_add_params(&ev, Some("k-1"), &[], None, &HashMap::new());
    assert_eq!(p["effort"], "high");
    assert_eq!(p["model"], "gpt-5.6-sol");
}

#[test]
fn build_add_params_group_empty_body() {
    let ev = json!({ "id": "g0", "kind": "group", "title": "재고", "category": "재고" });
    let p = build_add_params(&ev, Some("chunk-7"), &[], None, &HashMap::new());
    assert_eq!(p["kind"], "group");
    assert_eq!(p["body"], "");
    assert!(p.get("description").is_none());
    assert!(p.get("badge").is_none());
    assert!(p.get("isDraft").is_none());
}

#[test]
fn build_add_params_task_embeds_skeleton() {
    let ev = json!({ "id": "hunt", "kind": "task", "title": "Hunt", "stage": "hunt" });
    let ctx = json!({ "skeleton": { "program": { "type": "Program" } }, "directive": "약국 SaaS" });
    let p = build_add_params(&ev, Some("k-chunk"), &[], Some(&ctx), &HashMap::new());
    assert_eq!(p["kind"], "task");
    let body: Value = serde_json::from_str(p["body"].as_str().unwrap()).unwrap();
    assert_eq!(
        body["skeleton"],
        json!({ "program": { "type": "Program" } })
    );
    assert_eq!(body["stage"], "hunt");
    assert_eq!(body["args"]["directive"], "약국 SaaS");
    assert_eq!(body["args"]["chunkRef"], "k-chunk");
    assert!(p.get("badge").is_none());
}

#[test]
fn build_add_params_task_no_ctx_stage_only() {
    let ev = json!({ "id": "hunt", "kind": "task", "stage": "hunt" });
    let p = build_add_params(&ev, Some("k1"), &[], None, &HashMap::new());
    let body: Value = serde_json::from_str(p["body"].as_str().unwrap()).unwrap();
    assert_eq!(body["stage"], "hunt");
    assert!(body.get("skeleton").is_none());
}

// ── genSkeletonArgs ─────────────────────────────────────────────────────────
#[test]
fn gen_skeleton_args_idea_only() {
    assert_eq!(
        gen_skeleton_args(Some("약국 SaaS"), None, None, None, None).unwrap(),
        vec!["generate-skeleton", "--idea", "약국 SaaS", "--lang", "ko"]
    );
}

#[test]
fn gen_skeleton_args_full() {
    assert_eq!(
        gen_skeleton_args(
            Some("novel"),
            Some("glm-5.2"),
            Some("/cc/references"),
            Some("/o/gen.js"),
            Some("en")
        )
        .unwrap(),
        vec![
            "generate-skeleton",
            "--idea",
            "novel",
            "--lang",
            "en",
            "--model",
            "glm-5.2",
            "--refs",
            "/cc/references",
            "--gen-out",
            "/o/gen.js"
        ]
    );
}

#[test]
fn gen_skeleton_args_idea_required() {
    let e = gen_skeleton_args(None, Some("x"), None, None, None).unwrap_err();
    assert!(e.contains("idea 필수"));
}

// ── buildSecretEnvMap ───────────────────────────────────────────────────────
#[test]
fn build_secret_env_map_env_prefix_only() {
    let m = build_secret_env_map(&[
        "env:ANTHROPIC_BASE_URL".into(),
        "env:ANTHROPIC_AUTH_TOKEN".into(),
        "other".into(),
        "env:".into(),
    ]);
    let mut expected = HashMap::new();
    expected.insert(
        "ANTHROPIC_BASE_URL".to_string(),
        "env:ANTHROPIC_BASE_URL".to_string(),
    );
    expected.insert(
        "ANTHROPIC_AUTH_TOKEN".to_string(),
        "env:ANTHROPIC_AUTH_TOKEN".to_string(),
    );
    assert_eq!(m, expected);
    assert!(build_secret_env_map(&[]).is_empty());
}

// ── buildSpawnCmd ───────────────────────────────────────────────────────────
#[test]
fn build_spawn_cmd_bin_vs_default() {
    assert_eq!(
        build_spawn_cmd(Some("/x/bin/wf"), vec!["exec-one".into()]),
        ("/x/bin/wf".to_string(), vec!["exec-one".to_string()])
    );
    assert_eq!(
        build_spawn_cmd(None, vec!["exec-one".into(), "--lang".into(), "ko".into()]),
        (
            "sidecar:workflow".to_string(),
            vec![
                "exec-one".to_string(),
                "--lang".to_string(),
                "ko".to_string()
            ]
        )
    );
}

// ── resolveDirective ────────────────────────────────────────────────────────
#[test]
fn resolve_directive_priority() {
    let doc =
        json!({ "spec": "workflow-doc@0.0.1", "args": { "directive": { "default": "정련본" } } });
    assert_eq!(
        resolve_directive(Some("명시"), Some(&doc), Some("raw")).as_deref(),
        Some("명시")
    );
    assert_eq!(
        resolve_directive(None, Some(&doc), Some("raw")).as_deref(),
        Some("정련본")
    );
    assert_eq!(
        resolve_directive(Some(""), Some(&doc), Some("raw")).as_deref(),
        Some("정련본")
    );
    let non_doc = json!({ "program": {} });
    assert_eq!(
        resolve_directive(None, Some(&non_doc), Some("raw")).as_deref(),
        Some("raw")
    );
    assert_eq!(
        resolve_directive(None, None, Some("raw")).as_deref(),
        Some("raw")
    );
    let empty_default =
        json!({ "spec": "workflow-doc@0.0.1", "args": { "directive": { "default": "" } } });
    assert_eq!(
        resolve_directive(None, Some(&empty_default), Some("raw")).as_deref(),
        Some("raw")
    );
}

// ── leaseActive ─────────────────────────────────────────────────────────────
#[test]
fn lease_active_expiry() {
    let mut st = ReconcileState::default();
    assert!(!lease_active(&mut st, "n1", 100), "미설정 lease 는 비활성");
    st.leases.insert("n1".into(), 200);
    assert!(lease_active(&mut st, "n1", 100), "만료 전 활성");
    assert!(
        !lease_active(&mut st, "n1", 200),
        "만료 시각 도달 = 비활성 + 삭제"
    );
    assert!(!st.leases.contains_key("n1"), "만료 lease 는 lazy 삭제");
}

// ── reconcileTick (chunk 2) ─────────────────────────────────────────────────
fn tick(deps: &FakeDeps) -> Value {
    let mut st = ReconcileState::default();
    reconcile_tick(deps, &mut st, 0)
}

#[test]
fn reconcile_tick_item_verify() {
    let ns = nodes(vec![
        json!({ "id": "n1", "badge": "검수전", "blockedBy": [], "parentId": null, "status": "todo", "body": "{\"prompt\":\"verify\"}" }),
    ]);
    let d = FakeDeps::new(ns).exec(json!({ "oxf": "o", "result": { "reason": "실재 요건" } }));
    let r = tick(&d);
    assert_eq!(r["ok"], true);
    assert_eq!(r["processed"], 1);
    assert_eq!(r["id"], "n1");
    assert_eq!(r["badge"], "o");
    assert_eq!(d.c().get, vec!["n1"]);
    assert_eq!(d.c().exec, vec!["{\"prompt\":\"verify\"}"]);
    assert_eq!(d.c().edit.len(), 1);
    assert_eq!(d.c().edit_of("n1").unwrap()["badge"], "o");
    assert_eq!(d.c().poke, 1);
}

#[test]
fn reconcile_tick_no_ready() {
    let ns = nodes(vec![
        json!({ "id": "n1", "badge": "o", "blockedBy": [], "parentId": null, "status": "done" }),
    ]);
    let d = FakeDeps::new(ns).exec(json!({ "oxf": "o", "result": {} }));
    let r = tick(&d);
    assert_eq!(r["ok"], true);
    assert_eq!(r["processed"], 0);
    assert_eq!(d.c().exec.len(), 0);
    assert_eq!(d.c().edit.len(), 0);
    assert_eq!(d.c().poke, 0);
}

#[test]
fn reconcile_tick_no_verdict_single() {
    let ns = nodes(vec![
        json!({ "id": "n1", "badge": "검수전", "blockedBy": [], "parentId": null, "status": "todo", "body": "{\"prompt\":\"x\"}" }),
    ]);
    let d = FakeDeps::new(ns).exec(json!({ "oxf": null, "result": "무판정 출력" }));
    let r = tick(&d);
    assert_eq!(r["badge"], Value::Null);
    assert!(d.c().edit_of("n1").unwrap().get("badge").is_none());
    assert_eq!(d.c().poke, 0);
}

#[test]
fn reconcile_tick_exec_fail() {
    let ns = nodes(vec![
        json!({ "id": "n1", "badge": "검수전", "blockedBy": [], "parentId": null, "status": "todo", "body": "{\"prompt\":\"x\"}" }),
    ]);
    let d = FakeDeps::new(ns).exec_throws("exec-one exit 1 (529)");
    let r = tick(&d);
    assert_eq!(r["ok"], false);
    assert_eq!(r["processed"], 0);
    assert_eq!(d.c().edit.len(), 0);
    assert_eq!(d.c().poke, 0);
}

#[test]
fn reconcile_tick_task_publish() {
    let ns = nodes(vec![
        json!({ "id": "gen", "kind": "task", "status": "todo", "blockedBy": [], "parentId": "chunk-7", "body": "{\"stage\":\"generate\"}" }),
    ]);
    let staged = staged_children(
        vec![
            json!({ "ev": "add", "id": "g0", "kind": "group", "parent": "chunk-7", "title": "재고" }),
            json!({ "ev": "add", "id": "g0i0", "kind": "item", "parent": "g0", "title": "재고 차감", "prompt": "verify…", "badge": "검수전" }),
        ],
        json!({ "chunkTitle": "약국 재고 SaaS", "titleOrigin": "agent" }),
    );
    let d = FakeDeps::new(ns).stage(staged);
    let r = tick(&d);
    assert_eq!(r["ok"], true);
    assert_eq!(r["stage"], true);
    assert_eq!(r["published"], 2);
    assert_eq!(d.c().stage, vec!["{\"stage\":\"generate\"}"]);
    assert_eq!(d.c().add.len(), 2);
    assert_eq!(d.c().add[0]["parentId"], "chunk-7");
    assert_eq!(d.c().add[1]["kind"], "item");
    assert_eq!(
        d.c().add[1]["parentId"],
        "k-1",
        "항목 parent=그룹 칸반 id(keyOf)"
    );
    assert_eq!(d.c().edit_of("chunk-7").unwrap()["title"], "약국 재고 SaaS");
    assert_eq!(d.c().edit_of("gen").unwrap()["status"], "done");
    assert_eq!(d.c().poke, 1);
    assert_eq!(d.c().exec.len(), 0);
}

#[test]
fn apply_draft_doc_emits_routing_tier_on_item() {
    // draft(주 워크플로) 요건이 실은 tier 가 item 노드 발행 params 까지 흘러야 reconcile 이 exec 에 honor.
    let doc = json!({
        "kind": "draft-chunk", "chunk_ref": "chunk",
        "verify_contract": { "template": "T {{title}}", "directive": "D", "schema": { "type": "object" }, "initial_badge": "검수전" },
        "requirements": [
            { "id": "i0", "title": "auth 경계", "description": "d", "origin": "agent", "badge": "검수전", "effort": "max", "model": "gpt-5.6-sol" },
            { "id": "i1", "title": "날짜 포맷", "description": "d", "origin": "user", "badge": "검수전" }
        ],
        "tasks": []
    });
    let d = FakeDeps::new(vec![]);
    let n = crate::reconcile::draft::apply_draft_doc(&d, &doc, Some("chunk-k"), None).unwrap();
    assert_eq!(n, 2, "요건 2개 발행(Spec 섹션은 계수 밖)");
    let add = &d.c().add;
    // add[0] = Spec 섹션, add[1..] = 요건 프레임(Spec 섹션 밑).
    assert_eq!(add[1]["effort"], "max", "tier 실은 요건 → item 노드 effort");
    assert_eq!(add[1]["model"], "gpt-5.6-sol");
    assert!(
        add[2].get("effort").is_none(),
        "미지정 요건 = item 노드 effort 없음(기본 최고 보존)"
    );
    assert!(add[2].get("model").is_none());
}

#[test]
fn apply_draft_doc_nests_requirements_under_spec_section() {
    // 보드 모델 — 요건 프레임은 chunk 직속이 아니라 chunk 밑 "Spec" 섹션(collapsed) 밑에 locked 로 붙는다.
    // Spec 섹션은 검수전 item 처럼 보이면 안 됨(badge 없음 + item 아닌 kind) → pick_ready 가 실행/검증 대상으로 안 잡는다.
    let doc = json!({
        "kind": "draft-chunk", "chunk_ref": "chunk",
        "verify_contract": { "template": "T {{title}}", "directive": "D", "schema": { "type": "object" }, "initial_badge": "검수전" },
        "requirements": [
            { "id": "i0", "title": "auth 경계", "description": "d", "origin": "agent", "badge": "검수전" },
            { "id": "i1", "title": "날짜 포맷", "description": "d", "origin": "user", "badge": "검수전" }
        ],
        "tasks": []
    });
    let d = FakeDeps::new(vec![]);
    crate::reconcile::draft::apply_draft_doc(&d, &doc, Some("chunk-k"), None).unwrap();
    let add = &d.c().add;
    // (a) 첫 발행 = Spec 섹션(chunk 직속, locked, 섹션 kind, badge 없음).
    let spec = &add[0];
    assert_eq!(spec["parentId"], "chunk-k", "Spec 섹션은 chunk 직속");
    assert_eq!(spec["locked"], true, "Spec 섹션 locked");
    assert_eq!(
        spec["kind"], "section",
        "섹션 kind — item/task 아님(pick_ready 제외)"
    );
    assert!(
        spec.get("badge").is_none(),
        "Spec 섹션은 badge 없음(검수전 item 아님)"
    );
    // (c) collapse 는 자식을 숨기는 것 → leaf 프레임이 아니라 Spec 섹션에 실린다(프레임 기본 접힘).
    assert_eq!(spec["collapsed"], true, "Spec 섹션 collapsed");
    // (b) 요건 프레임 parentId = Spec 섹션 id(chunk 직속 아님). FakeDeps: 첫 add → "k-1".
    assert_eq!(add[1]["parentId"], "k-1", "요건 프레임은 Spec 섹션 밑");
    assert_eq!(add[2]["parentId"], "k-1");
    assert!(
        add[1].get("collapsed").is_none(),
        "leaf 프레임엔 collapsed 없음"
    );
    // 프레임은 여전히 item + 검수전 + locked(부모만 바뀜, id·badge 불변 → blockedBy 유효).
    assert_eq!(add[1]["kind"], "item");
    assert_eq!(add[1]["badge"], "검수전");
    assert_eq!(add[1]["locked"], true);
}

#[test]
fn reconcile_tick_hunt_ledger_injected() {
    let ns = nodes(vec![
        json!({ "id": "hunt", "kind": "task", "status": "todo", "blockedBy": [], "parentId": "chunk", "body": "{\"skeleton\":{},\"stage\":\"hunt\",\"args\":{\"directive\":\"약국\"}}" }),
    ]);
    let d = FakeDeps::new(ns)
        .stage(staged_children(vec![], Value::Null))
        .ledger(vec![json!({ "title": "재고 차감", "badge": "o" })]);
    tick(&d);
    let sent: Value = serde_json::from_str(&d.c().stage[0]).unwrap();
    assert_eq!(
        sent["args"]["ledger"],
        json!([{ "title": "재고 차감", "badge": "o" }])
    );
    assert_eq!(sent["stage"], "hunt");
}

#[test]
fn reconcile_tick_classify_ledger_injected() {
    let ns = nodes(vec![
        json!({ "id": "classify", "kind": "task", "status": "todo", "blockedBy": [], "parentId": "chunk", "body": "{\"stage\":\"classify\",\"args\":{\"directive\":\"약국\"}}" }),
    ]);
    let d = FakeDeps::new(ns)
        .stage(staged_children(
            vec![],
            json!({ "dimension": "", "assignments": [] }),
        ))
        .ledger(vec![
            json!({ "id": "i0", "title": "재고 차감", "badge": "o" }),
        ]);
    tick(&d);
    let sent: Value = serde_json::from_str(&d.c().stage[0]).unwrap();
    assert_eq!(
        sent["args"]["ledger"],
        json!([{ "id": "i0", "title": "재고 차감", "badge": "o" }])
    );
    assert_eq!(sent["stage"], "classify");
}

#[test]
fn reconcile_tick_classify_result_assign() {
    let ns = nodes(vec![
        json!({ "id": "classify", "kind": "task", "status": "todo", "blockedBy": [], "parentId": "chunk", "body": "{\"stage\":\"classify\"}" }),
    ]);
    let staged = staged_children(
        vec![],
        json!({ "dimension": "기능 영역", "assignments": [{ "id": "i0", "category": "재고" }, { "id": "i1", "category": "발주" }] }),
    );
    let d = FakeDeps::new(ns).stage(staged).ledger(vec![
        json!({ "id": "i0", "title": "차감", "badge": "o" }),
        json!({ "id": "i1", "title": "발주", "badge": "o" }),
    ]);
    let r = tick(&d);
    assert_eq!(r["ok"], true);
    assert_eq!(r["stage"], true);
    assert_eq!(r["assigned"], 2);
    assert_eq!(d.c().edit_of("i0").unwrap(), &json!({ "category": "재고" }));
    assert_eq!(d.c().edit_of("i1").unwrap(), &json!({ "category": "발주" }));
    assert_eq!(d.c().edit_of("chunk").unwrap()["result"], "기능 영역");
    assert_eq!(d.c().edit_of("classify").unwrap()["status"], "done");
    assert_eq!(d.c().poke, 1);
    assert_eq!(d.c().add.len(), 0);
}

#[test]
fn reconcile_tick_no_verdict_cap_then_f() {
    let ns = nodes(vec![
        json!({ "id": "n1", "badge": "검수전", "blockedBy": [], "parentId": null, "status": "todo", "body": "{\"prompt\":\"x\"}" }),
    ]);
    let mut st = ReconcileState::default();
    let mk = || FakeDeps::new(ns.clone()).exec(json!({ "oxf": null, "result": "무판정 출력" }));
    let d1 = mk();
    reconcile_tick(&d1, &mut st, 0);
    assert!(d1.c().edit_of("n1").unwrap().get("badge").is_none());
    assert_eq!(d1.c().poke, 0);
    let d2 = mk();
    reconcile_tick(&d2, &mut st, 0);
    assert!(d2.c().edit_of("n1").unwrap().get("badge").is_none());
    let d3 = mk();
    let r3 = reconcile_tick(&d3, &mut st, 0);
    assert_eq!(d3.c().edit_of("n1").unwrap()["badge"], "f");
    assert!(d3.c().edit_of("n1").unwrap()["result"]
        .as_str()
        .unwrap()
        .contains("무판정 3회"));
    assert_eq!(d3.c().poke, 1);
    assert_eq!(r3["badge"], "f");
}

#[test]
fn reconcile_tick_no_verdict_reset_on_success() {
    let ns = nodes(vec![
        json!({ "id": "n1", "badge": "검수전", "blockedBy": [], "parentId": null, "status": "todo", "body": "{\"prompt\":\"x\"}" }),
    ]);
    let mut st = ReconcileState::default();
    reconcile_tick(
        &FakeDeps::new(ns.clone()).exec(json!({ "oxf": null, "result": "무판정" })),
        &mut st,
        0,
    );
    reconcile_tick(
        &FakeDeps::new(ns.clone()).exec(json!({ "oxf": null, "result": "무판정" })),
        &mut st,
        0,
    );
    let d_ok = FakeDeps::new(ns.clone()).exec(json!({ "oxf": "o", "result": "판정" }));
    reconcile_tick(&d_ok, &mut st, 0);
    assert_eq!(d_ok.c().edit_of("n1").unwrap()["badge"], "o");
    let d4 = FakeDeps::new(ns.clone()).exec(json!({ "oxf": null, "result": "무판정" }));
    reconcile_tick(&d4, &mut st, 0);
    assert!(d4.c().edit_of("n1").unwrap().get("badge").is_none());
}

#[test]
fn reconcile_tick_starvation_fail_min_pick() {
    let ns = nodes(vec![
        json!({ "id": "n1", "badge": "검수전", "blockedBy": [], "parentId": null, "status": "todo", "body": "{\"prompt\":\"a\"}" }),
        json!({ "id": "n2", "badge": "검수전", "blockedBy": [], "parentId": null, "status": "todo", "body": "{\"prompt\":\"b\"}" }),
    ]);
    let mut st = ReconcileState::default();
    let d1 = FakeDeps::new(ns.clone()).exec_throws("영구 실패");
    let r1 = reconcile_tick(&d1, &mut st, 0);
    assert_eq!(r1["ok"], false);
    assert_eq!(r1["id"], "n1");
    let d2 = FakeDeps::new(ns.clone()).exec(json!({ "oxf": "o", "result": "ok" }));
    let r2 = reconcile_tick(&d2, &mut st, 0);
    assert_eq!(r2["id"], "n2", "n1 실패 → n2 선택");
    assert_eq!(r2["badge"], "o");
}

#[test]
fn reconcile_tick_item_rolls_up_chunk_progress() {
    // 보드 모델 — 프레임 badge 확정 시 부모 chunk 진척을 롤업(변화무쌍한 진행중). 확정(o/x/f) 프레임 /
    // 전체 프레임을 description 에 싣고, done 아니면 status=inprogress. 섹션-밑 프레임도 chunk 까지 올라간다.
    // badge 축(audit 인증)은 안 건드린다. 방금 확정한 프레임은 스냅샷이 아직 검수전이라 계수 override 로 반영.
    let ns = nodes(vec![
        json!({ "id": "chunk", "kind": "chunk", "parentId": null, "status": "todo" }),
        json!({ "id": "spec", "kind": "section", "parentId": "chunk", "title": "Spec" }),
        json!({ "id": "i0", "kind": "item", "parentId": "spec", "badge": "o" }),
        json!({ "id": "i1", "kind": "item", "parentId": "spec", "badge": "검수전", "status": "todo", "blockedBy": [], "body": "{\"prompt\":\"verify\"}" }),
        json!({ "id": "f0", "kind": "fact", "parentId": "chunk", "badge": "검수전", "status": "todo", "blockedBy": [] }),
    ]);
    let d = FakeDeps::new(ns).exec(json!({ "oxf": "o", "result": { "reason": "실재" } }));
    let r = tick(&d);
    assert_eq!(r["ok"], true);
    assert_eq!(r["id"], "i1");
    assert_eq!(r["badge"], "o");
    // i0(o) + i1(방금 o) 확정, f0(검수전) 미확정 → 확정 2/3(item·fact 통합 계수).
    let ce = d
        .c()
        .edit_of("chunk")
        .cloned()
        .expect("chunk 진척 롤업 edit");
    assert_eq!(ce["description"], "확정 2/3");
    assert_eq!(ce["status"], "inprogress");
    assert!(
        ce.get("badge").is_none(),
        "badge 축은 audit 소관 — 롤업이 안 건드림"
    );
    assert!(ce.get("title").is_none(), "롤업은 title 을 안 건드림");
}

#[test]
fn reconcile_tick_rollup_skips_done_chunk() {
    // Step 3 이슈라이즈 게이트가 status=done 설정한 chunk 는 롤업이 안 덮는다(status != "done" 가드).
    let ns = nodes(vec![
        json!({ "id": "chunk", "kind": "chunk", "parentId": null, "status": "done" }),
        json!({ "id": "i0", "kind": "item", "parentId": "chunk", "badge": "o" }),
        json!({ "id": "i1", "kind": "item", "parentId": "chunk", "badge": "검수전", "status": "todo", "blockedBy": [], "body": "{\"prompt\":\"v\"}" }),
    ]);
    let d = FakeDeps::new(ns).exec(json!({ "oxf": "o", "result": {} }));
    let r = tick(&d);
    assert_eq!(r["badge"], "o");
    assert_eq!(
        d.c().edit_of("i1").unwrap()["badge"],
        "o",
        "프레임 badge 는 확정"
    );
    assert!(d.c().edit_of("chunk").is_none(), "done chunk 는 롤업 불변");
}

#[test]
fn reconcile_tick_audit_certify() {
    let ns = nodes(vec![
        json!({ "id": "audit", "kind": "task", "status": "todo", "blockedBy": [], "parentId": "chunk", "body": "{\"stage\":\"audit\"}" }),
    ]);
    let d = FakeDeps::new(ns)
        .stage(staged_children(
            vec![],
            json!({ "verdict": "완결 — 목표 도달", "complete": true }),
        ))
        .ledger(vec![
            json!({ "id": "i0", "title": "a", "badge": "o" }),
            json!({ "id": "i1", "title": "b", "badge": "x" }),
        ]);
    let r = tick(&d);
    assert_eq!(r["ok"], true);
    assert_eq!(d.c().edit_of("chunk").unwrap()["badge"], "o");
    assert_eq!(
        d.c().edit_of("chunk").unwrap()["result"],
        "완결 — 목표 도달"
    );
    assert_eq!(d.c().edit_of("audit").unwrap()["status"], "done");
}

#[test]
fn reconcile_tick_audit_applies_removals() {
    // 합의 루프의 remove — audit reviewer 가 removals 로 지목한 현재 항목을 badge→x(자기교정). 삭제 아님.
    let ns = nodes(vec![
        json!({ "id": "audit", "kind": "task", "status": "todo", "blockedBy": [], "parentId": "chunk", "body": "{\"stage\":\"audit\"}" }),
    ]);
    let d = FakeDeps::new(ns)
        .stage(staged_children(
            vec![],
            json!({
                "verdict": "1건 범위밖 제거",
                "complete": true,
                "removals": [{ "id": "i1", "reason": "지시서가 명시 배제한 범위 — 범위밖" }]
            }),
        ))
        .ledger(vec![
            json!({ "id": "i0", "badge": "o" }),
            json!({ "id": "i1", "badge": "o" }),
        ]);
    let r = tick(&d);
    assert_eq!(r["ok"], true);
    assert_eq!(
        d.c().edit_of("i1").unwrap()["badge"],
        "x",
        "removals 대상 → badge x"
    );
    assert_eq!(
        d.c().edit_of("i1").unwrap()["result"],
        "지시서가 명시 배제한 범위 — 범위밖",
        "사유 기록(ledger 잔존→히스토리)"
    );
    assert!(d.c().edit_of("i0").is_none(), "지목 안 된 항목 불변");
}

#[test]
fn reconcile_tick_audit_no_removals_field_noop() {
    // removals 필드 없는 기존 audit 은 remove 무발생(회귀 없음).
    let ns = nodes(vec![
        json!({ "id": "audit", "kind": "task", "status": "todo", "blockedBy": [], "parentId": "chunk", "body": "{\"stage\":\"audit\"}" }),
    ]);
    let d = FakeDeps::new(ns)
        .stage(staged_children(
            vec![],
            json!({ "verdict": "완결", "complete": true }),
        ))
        .ledger(vec![json!({ "id": "i0", "badge": "o" })]);
    tick(&d);
    assert!(
        d.c().edit_of("i0").is_none(),
        "removals 없으면 항목 badge 불변"
    );
}

#[test]
fn build_stage_input_injects_document_and_round_for_consensus() {
    // 합의 스테이지(research-audit)는 옛 facts/removed 채널이 아니라 단일 document 채널을 받는다.
    // {{document}}=args.ledger(state+history), {{round}}=args.round. reviewer 가 이 한 채널로
    // [현재 집합 + 변경 히스토리] 를 본다. state=badge 매핑(x→x, 그 밖→o), history=result JSON.
    let n = node(
        json!({ "id": "research-audit", "kind": "task", "parentId": "chunk", "status": "todo",
        "body": "{\"workflow\":\"research\",\"stage\":\"research-audit\",\"args\":{\"directive\":\"d\",\"round\":4}}" }),
    );
    let d = FakeDeps::new(nodes(vec![
        json!({ "id": "chunk", "kind": "chunk", "parentId": null }),
        json!({ "id": "f1", "kind": "fact", "parentId": "chunk", "title": "o-fact", "badge": "o" }),
        json!({ "id": "f2", "kind": "fact", "parentId": "chunk", "title": "뺀-fact", "badge": "x",
            "result": "{\"reason\":\"지시서 범위밖\",\"history\":[{\"round\":1,\"action\":\"remove\",\"reason\":\"지시서 범위밖\"}]}" }),
    ]));
    let si = build_stage_input(&d, &n, n.body_str(), "research-audit").expect("build_stage_input");
    let body: Value = serde_json::from_str(&si.stage_body).unwrap();
    assert_eq!(body.pointer("/args/round"), Some(&json!(4)), "round 주입");
    let doc = body
        .pointer("/args/ledger")
        .and_then(|v| v.as_array())
        .expect("합의 document(=args.ledger) 주입");
    assert_eq!(doc.len(), 2, "research fact 전부(state 무관)");
    assert_eq!(doc[0]["state"], "o", "badge o → state o");
    assert_eq!(
        doc[1]["state"], "x",
        "badge x → state x(뺀 항목도 문서에 잔존)"
    );
    assert_eq!(
        doc[1]["history"][0]["action"], "remove",
        "history 는 result JSON 에서 복원(진동 차단 채널)"
    );
    let facts = body.pointer("/args/facts");
    assert!(
        facts.is_none(),
        "옛 facts 채널 폐기 — document 한 채널만: {facts:?}"
    );
}

#[test]
fn build_consensus_document_scopes_research_vs_design_facts() {
    // 한 chunk 를 research·design 이 공유 — design-audit 는 설계 카테고리 fact 만, research-audit 는 그 밖만.
    let ns = nodes(vec![
        json!({ "id": "chunk", "kind": "chunk", "parentId": null }),
        json!({ "id": "rf", "kind": "fact", "parentId": "chunk", "title": "기초지식", "category": "규제", "badge": "o" }),
        json!({ "id": "df", "kind": "fact", "parentId": "chunk", "title": "도메인 모델", "category": "domain-model", "badge": "o" }),
    ]);
    let research =
        build_consensus_document(&ns, "chunk", &consensus_spec("research-audit").unwrap());
    assert_eq!(
        ids_of(&research),
        vec!["rf"],
        "research scope = 설계 밖 fact"
    );
    let design = build_consensus_document(&ns, "chunk", &consensus_spec("design-audit").unwrap());
    assert_eq!(
        ids_of(&design),
        vec!["df"],
        "design scope = 설계 카테고리 fact"
    );
}

// ── 합의 changes 물질화 이음부 (draft-review) ──────────────────────────────────
// reconcile_stage 를 직접 호출 — pick_ready 순서(검수전 item 우선) 우회, 합의 task 를 결정적으로 타깃.

// draft-review task + 프레임/섹션 픽스처. round 를 인자로 body args.round 에 싣는다.
fn draft_review_fixture(item_badge: &str, round: u32) -> (Vec<Node>, Node) {
    let ns = nodes(vec![
        json!({ "id": "chunk", "kind": "chunk", "isDraft": true, "parentId": null, "status": "inprogress" }),
        json!({ "id": "spec", "kind": "section", "parentId": "chunk", "title": "Spec" }),
        json!({ "id": "i0", "kind": "item", "parentId": "spec", "title": "재고 차감", "description": "원자적 차감", "badge": item_badge,
            "body": "{\"promptHash\":\"h-verify\",\"vars\":{\"title\":\"재고 차감\",\"description\":\"원자적 차감\"},\"refs\":{\"directive\":\"r-dir\"}}" }),
    ]);
    let target = node(
        json!({ "id": "draft-review", "kind": "task", "parentId": "chunk", "status": "todo", "blockedBy": [],
        "body": format!("{{\"workflow\":\"draft\",\"stage\":\"draft-review\",\"args\":{{\"directive\":\"d\",\"round\":{round}}}}}") }),
    );
    (ns, target)
}

#[test]
fn changes_remove_flips_frame_to_x_and_appends_history() {
    // (a) op:remove → 대상 프레임 badge→x + result 에 {reason, history[]} 누적(다음 라운드 document 채널).
    let (ns, target) = draft_review_fixture("o", 2);
    let d = FakeDeps::new(ns).stage(staged_children(
        vec![],
        json!({ "changes": [{ "op": "remove", "id": "i0", "reason": "i7 이 흡수 — 중복" }] }),
    ));
    let body = target.body_str().to_string();
    let nodes_snapshot = d.list_nodes();
    let r = reconcile_stage(&d, &target, &body, &nodes_snapshot);
    assert_eq!(r["ok"], true);
    let edit = d.c().edit_of("i0").cloned().expect("i0 편집");
    assert_eq!(edit["badge"], "x", "remove → 프레임 badge x");
    let result: Value = serde_json::from_str(edit["result"].as_str().unwrap()).unwrap();
    assert_eq!(result["reason"], "i7 이 흡수 — 중복", "사유 기록");
    assert_eq!(
        result["history"].as_array().unwrap().last().unwrap()["action"],
        "remove",
        "history append(진동 차단)"
    );
    assert_eq!(result["history"][0]["round"], 2, "라운드 번호 탑재");
}

#[test]
fn changes_add_publishes_pending_frame_under_correct_section() {
    // (b) op:add → 검수전 프레임 신규, 올바른 섹션(Spec) 밑, 형제 검증 body 복제(검증 대상화).
    let (ns, target) = draft_review_fixture("o", 1);
    let d = FakeDeps::new(ns).stage(staged_children(
        vec![],
        json!({ "changes": [{ "op": "add", "title": "동시성 승자 규칙", "description": "충돌 입력의 승자", "reason": "운영 견고성 누락" }] }),
    ));
    let body = target.body_str().to_string();
    let snap = d.list_nodes();
    reconcile_stage(&d, &target, &body, &snap);
    let added = d
        .c()
        .add_find(|p| p["kind"] == "item" && p["title"] == "동시성 승자 규칙")
        .cloned()
        .expect("신규 item 프레임 발행");
    assert_eq!(added["parentId"], "spec", "올바른 섹션(Spec) 밑");
    assert_eq!(added["badge"], "검수전", "검수전 프레임(검증 대상)");
    assert_eq!(added["kind"], "item");
    // 형제 body 복제 — promptHash 상속 + vars 교체(검증 배선).
    let cloned: Value = serde_json::from_str(added["body"].as_str().unwrap()).unwrap();
    assert_eq!(cloned["promptHash"], "h-verify", "형제 검증 배선 상속");
    assert_eq!(cloned["vars"]["title"], "동시성 승자 규칙", "vars 교체");
    let result: Value = serde_json::from_str(added["result"].as_str().unwrap()).unwrap();
    assert_eq!(result["history"][0]["action"], "add", "add history 시작");
}

#[test]
fn changes_republish_increments_round() {
    // (c) 이견 잔존 → -again 자기재발행 task 의 body args.round 가 +1 누적.
    let (ns, target) = draft_review_fixture("o", 3);
    let d = FakeDeps::new(ns).stage(staged_children(
        vec![json!({ "id": "draft-review-again", "kind": "task", "stage": "draft-review", "parent": "chunk", "title": "재검" })],
        json!({ "changes": [{ "op": "add", "title": "X", "description": "d", "reason": "r" }] }),
    ));
    let body = target.body_str().to_string();
    let snap = d.list_nodes();
    reconcile_stage(&d, &target, &body, &snap);
    let again = d
        .c()
        .add_find(|p| p["kind"] == "task" && p["title"] == "재검")
        .cloned()
        .expect("-again 재발행");
    let again_body: Value = serde_json::from_str(again["body"].as_str().unwrap()).unwrap();
    assert_eq!(again_body["args"]["round"], 4, "round 3 → 재발행 4(누적)");
    assert_eq!(
        again_body["stage"], "draft-review",
        "같은 합의 스테이지 재발행"
    );
}

#[test]
fn changes_round_cap_seals_chunk_instead_of_republish() {
    // (d) round 상한(20) 도달 시 -again 억제 + chunk 봉인(badge=f). 무한 재발행 차단.
    let (ns, target) = draft_review_fixture("o", CONSENSUS_ROUND_MAX);
    let d = FakeDeps::new(ns).stage(staged_children(
        vec![json!({ "id": "draft-review-again", "kind": "task", "stage": "draft-review", "parent": "chunk", "title": "재검" })],
        json!({ "changes": [{ "op": "remove", "id": "i0", "reason": "막판 이견" }] }),
    ));
    let body = target.body_str().to_string();
    let snap = d.list_nodes();
    reconcile_stage(&d, &target, &body, &snap);
    assert!(
        d.c().add_find(|p| p["kind"] == "task").is_none(),
        "상한 도달 — 재발행 억제(task 미발행)"
    );
    assert_eq!(
        d.c().edit_of("chunk").unwrap()["badge"],
        "f",
        "봉인 — chunk badge f"
    );
}

#[test]
fn reconcile_tick_audit_f_propagate() {
    let ns = nodes(vec![
        json!({ "id": "audit", "kind": "task", "status": "todo", "blockedBy": [], "parentId": "chunk", "body": "{\"stage\":\"audit\"}" }),
    ]);
    let d = FakeDeps::new(ns)
        .stage(staged_children(
            vec![],
            json!({ "verdict": "감사 통과 주장", "complete": true }),
        ))
        .ledger(vec![
            json!({ "id": "i0", "badge": "o" }),
            json!({ "id": "i1", "badge": "f" }),
        ]);
    tick(&d);
    assert_eq!(d.c().edit_of("chunk").unwrap()["badge"], "f");
}

#[test]
fn reconcile_tick_audit_incomplete() {
    let ns = nodes(vec![
        json!({ "id": "audit", "kind": "task", "status": "todo", "blockedBy": [], "parentId": "chunk", "body": "{\"stage\":\"audit\"}" }),
    ]);
    let d = FakeDeps::new(ns)
        .stage(staged_children(
            vec![],
            json!({ "verdict": "누락 존재", "complete": false }),
        ))
        .ledger(vec![json!({ "id": "i0", "badge": "o" })]);
    tick(&d);
    assert_eq!(d.c().edit_of("chunk").unwrap()["badge"], "f");
    assert_eq!(d.c().edit_of("chunk").unwrap()["result"], "누락 존재");
}

#[test]
fn reconcile_tick_audit_no_result() {
    let ns = nodes(vec![
        json!({ "id": "audit", "kind": "task", "status": "todo", "blockedBy": [], "parentId": "chunk", "body": "{\"stage\":\"audit\"}" }),
    ]);
    let d = FakeDeps::new(ns)
        .stage(staged_children(vec![], Value::Null))
        .ledger(vec![json!({ "id": "i0", "badge": "o" })]);
    let r = tick(&d);
    assert_eq!(r["ok"], false);
    assert!(r["message"].as_str().unwrap().contains("audit 결과 없음"));
    assert_eq!(d.c().edit.len(), 0);
}

#[test]
fn reconcile_tick_materialize_fail_rejects_before_stage() {
    let ns = nodes(vec![
        json!({ "id": "audit", "kind": "task", "status": "todo", "blockedBy": [], "parentId": "chunk", "body": "{\"stage\":\"audit\"}" }),
    ]);
    let d = FakeDeps::new(ns)
        .stage(staged_children(
            vec![],
            json!({ "verdict": "v", "complete": true }),
        ))
        .ledger_throws("kanban 응답 없음");
    let r = tick(&d);
    assert_eq!(r["ok"], false);
    assert!(r["message"]
        .as_str()
        .unwrap()
        .contains("원장 materialize 실패(audit)"));
    assert_eq!(d.c().stage.len(), 0);
}

#[test]
fn reconcile_stage_workflowref_propagates_to_child_task() {
    let ns = nodes(vec![
        json!({ "id": "research", "kind": "task", "status": "todo", "blockedBy": [], "parentId": "chunk", "body": "{\"workflow\":\"research\",\"stage\":\"research\",\"args\":{\"directive\":\"정련\"}}" }),
    ]);
    let staged = staged_children(
        vec![
            json!({ "ev": "add", "id": "fact0", "kind": "fact", "parent": "chunk", "title": "저장소 확정", "badge": "검수전" }),
            json!({ "ev": "add", "id": "plan", "kind": "task", "parent": "chunk", "stage": "plan", "title": "슈도코드화", "blocked_by": ["fact0"] }),
        ],
        Value::Null,
    );
    let d = FakeDeps::new(ns)
        .stage(staged)
        .ledger(vec![json!({ "id": "i0", "title": "요건", "badge": "o" })]);
    tick(&d);
    let plan_add = d.c().add_find(|p| p["kind"] == "task").cloned().unwrap();
    let body: Value = serde_json::from_str(plan_add["body"].as_str().unwrap()).unwrap();
    assert_eq!(
        body,
        json!({ "workflow": "research", "stage": "plan", "args": { "directive": "정련", "chunkRef": "chunk" } })
    );
}

// ── extractOxf(exec_one 재사용 확인) ────────────────────────────────────────
#[test]
fn extract_oxf_keys_and_normalization() {
    use crate::exec_one::extract_oxf;
    assert_eq!(extract_oxf(&json!({ "oxf": "o" })).as_deref(), Some("o"));
    assert_eq!(extract_oxf(&json!({ "oxf": " X " })).as_deref(), Some("x"));
    assert_eq!(
        extract_oxf(&json!({ "verdict": "f" })).as_deref(),
        Some("f")
    );
    assert_eq!(extract_oxf(&json!({ "oxf": "pass" })), None);
    assert_eq!(extract_oxf(&json!("문자열")), None);
    assert_eq!(extract_oxf(&Value::Null), None);
}

// ── nextTick / submitTick (chunk 3) ─────────────────────────────────────────
// getNode가 fullBody를 body로 주입하는 resolve_body 실경로 —
// body 의 promptHash→resolve_out(prompt), schemaHash→get_prompt_out(value) 로 조립(스텁 대신).
fn cli_deps(nodes: Vec<Node>, full_body: &str) -> FakeDeps {
    let mut ns = nodes;
    for n in &mut ns {
        n.body = Some(full_body.to_string());
    }
    let mut d = FakeDeps::new(ns);
    d.resolve_out = Some(json!({ "prompt": "VERIFY: 항목을 판정하라" }));
    d.get_prompt_out = Some(json!({ "value": { "required": ["oxf"] } }));
    d
}

#[test]
fn next_tick_returns_verify_package_with_lease() {
    let ns = nodes(vec![
        json!({ "id": "t1", "kind": "task", "status": "todo", "blockedBy": [] }),
        json!({ "id": "v1", "kind": "item", "badge": "검수전", "blockedBy": [], "title": "요건 검증" }),
    ]);
    let d = cli_deps(ns, "{\"promptHash\":\"h\",\"schemaHash\":\"sh\"}");
    let mut st = ReconcileState::default();
    let r = next_tick(&d, &mut st, None, 0);
    assert_eq!(r["ok"], true);
    assert_eq!(r["node"]["id"], "v1");
    assert!(r["prompt"].as_str().unwrap().contains("VERIFY"));
    assert!(r["schema"].is_object());
    assert!(lease_active(&mut st, "v1", 0));
}

#[test]
fn next_tick_leased_node_not_redistributed() {
    let ns = nodes(vec![
        json!({ "id": "v1", "kind": "item", "badge": "검수전", "blockedBy": [], "title": "요건" }),
    ]);
    let mut st = ReconcileState::default();
    next_tick(
        &cli_deps(ns.clone(), "{\"promptHash\":\"h\"}"),
        &mut st,
        None,
        0,
    );
    let r2 = next_tick(&cli_deps(ns, "{\"promptHash\":\"h\"}"), &mut st, None, 0);
    assert_eq!(r2["ok"], true);
    assert_eq!(r2["node"], Value::Null);
}

#[test]
fn reconcile_tick_skips_leased_node() {
    let ns = nodes(vec![
        json!({ "id": "v1", "kind": "item", "badge": "검수전", "blockedBy": [], "title": "요건", "body": "" }),
    ]);
    let mut st = ReconcileState::default();
    st.leases.insert("v1".into(), 60_000);
    let d = FakeDeps::new(ns).exec(json!({ "oxf": "o", "result": "ok" }));
    let r = reconcile_tick(&d, &mut st, 0);
    assert_eq!(r["processed"], 0);
}

#[test]
fn submit_tick_pipe_and_lease_release() {
    let ns = nodes(vec![
        json!({ "id": "v1", "kind": "item", "badge": "검수전", "title": "요건" }),
    ]);
    let mut st = ReconcileState::default();
    st.leases.insert("v1".into(), 60_000);
    let d = cli_deps(ns, "{}");
    let r = submit_tick(
        &d,
        &mut st,
        "v1",
        &json!({ "oxf": "o", "origin": "agent", "reason": "실재 요건" }),
    );
    assert_eq!(r["ok"], true);
    assert_eq!(r["badge"], "o");
    assert_eq!(d.c().edit[0].0, "v1");
    assert_eq!(d.c().edit[0].1["badge"], "o");
    assert!(d.c().edit[0].1["result"]
        .as_str()
        .unwrap()
        .contains("실재 요건"));
    assert_eq!(d.c().poke, 1);
    assert!(!lease_active(&mut st, "v1", 0));
}

#[test]
fn submit_tick_already_done_rejected() {
    let ns = nodes(vec![
        json!({ "id": "v1", "kind": "item", "badge": "o", "title": "요건" }),
    ]);
    let d = cli_deps(ns, "{}");
    let mut st = ReconcileState::default();
    let r = submit_tick(&d, &mut st, "v1", &json!({ "oxf": "x" }));
    assert_eq!(r["ok"], false);
    assert_eq!(r["code"], "ALREADY_DONE");
    assert_eq!(d.c().edit.len(), 0);
}

#[test]
fn submit_tick_no_verdict_rejected() {
    let ns = nodes(vec![
        json!({ "id": "v1", "kind": "item", "badge": "검수전", "title": "요건" }),
    ]);
    let d = cli_deps(ns, "{}");
    let mut st = ReconcileState::default();
    let r = submit_tick(&d, &mut st, "v1", &json!({ "reason": "판정 없이" }));
    assert_eq!(r["ok"], false);
    assert_eq!(r["code"], "INVALID_INPUT");
    assert_eq!(d.c().edit.len(), 0);
}

// ── reconcileStage plan/design ground(o only) ───────────────────────────────
#[test]
fn reconcile_stage_plan_ground_o_only() {
    let mixed = vec![
        json!({ "id": "i1", "title": "요건A", "badge": "o" }),
        json!({ "id": "i2", "title": "요건B", "badge": "x" }),
        json!({ "id": "f1", "title": "fact치명", "badge": "f" }),
    ];
    let ns = nodes(vec![
        json!({ "id": "plan", "kind": "task", "status": "todo", "blockedBy": [], "parentId": "chunk", "body": "{\"workflow\":\"research\",\"stage\":\"plan\",\"args\":{\"directive\":\"d\"}}" }),
    ]);
    let d = FakeDeps::new(ns)
        .stage(staged_children(vec![], Value::Null))
        .ledger(mixed.clone())
        .facts(mixed);
    tick(&d);
    let sent: Value = serde_json::from_str(&d.c().stage[0]).unwrap();
    let ledger_ids: Vec<&str> = sent["args"]["ledger"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    assert_eq!(ledger_ids, vec!["i1"], "plan 요건 원장 = o 만");
    let fact_ids: Vec<&str> = sent["args"]["facts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    assert_eq!(fact_ids, vec!["i1"], "plan ground = o 만");
}

// ── 스테이지 섹션(Research/Design/Plan) 주입 ─────────────────────────────────
// 보드 모델 — 스테이지 프레임(fact/plan-unit)은 chunk 직속이 아니라 chunk 밑 스테이지 섹션 밑에 매단다.
// 섹션은 stage_name 으로 결정(research→Research, design-*→Design(공유), plan/plan-patch→Plan). Spec 은
// apply_draft_doc 소관. 섹션은 검수전 item 처럼 보이면 안 되므로 badge 없음 + kind=section(pick_ready 게이트)
// + collapsed(자식 숨김). task/code 자식은 섹션이 아니라 chunk 직속으로 남는다.
#[test]
fn research_frames_nest_under_research_section() {
    let ns = nodes(vec![
        json!({ "id": "research", "kind": "task", "status": "todo", "blockedBy": [], "parentId": "chunk", "body": "{\"workflow\":\"research\",\"stage\":\"research\",\"args\":{\"directive\":\"정련\"}}" }),
    ]);
    let staged = staged_children(
        vec![
            json!({ "ev": "add", "id": "fact0", "kind": "fact", "parent": "chunk", "title": "저장소 확정", "badge": "검수전" }),
            json!({ "ev": "add", "id": "research-audit", "kind": "task", "parent": "chunk", "stage": "research-audit", "title": "완전성 감사", "blocked_by": ["fact0"] }),
        ],
        Value::Null,
    );
    let d = FakeDeps::new(ns).stage(staged).ledger(vec![]);
    tick(&d);
    let add = &d.c().add;
    // add[0] = Research 섹션(chunk 직속, locked, collapsed, badge 없음, kind=section).
    assert_eq!(add[0]["kind"], "section", "섹션 kind — pick_ready 제외");
    assert_eq!(add[0]["title"], "Research");
    assert_eq!(add[0]["parentId"], "chunk", "Research 섹션은 chunk 직속");
    assert_eq!(add[0]["locked"], true);
    assert_eq!(add[0]["collapsed"], true);
    assert!(add[0].get("badge").is_none(), "섹션은 badge 없음");
    // add[1] = fact 프레임(Research 섹션 밑, id·badge 불변 → blockedBy 유효). FakeDeps: 섹션 add → "k-1".
    assert_eq!(add[1]["kind"], "fact");
    assert_eq!(add[1]["parentId"], "k-1", "fact 는 Research 섹션 밑");
    assert!(
        add[1].get("collapsed").is_none(),
        "leaf 프레임엔 collapsed 없음"
    );
    // add[2] = research-audit task 는 섹션이 아니라 chunk 직속.
    assert_eq!(add[2]["kind"], "task");
    assert_eq!(add[2]["parentId"], "chunk", "task 는 chunk 직속");
}

#[test]
fn design_interface_creates_design_section() {
    let ns = nodes(vec![
        json!({ "id": "design-interface", "kind": "task", "status": "todo", "blockedBy": [], "parentId": "chunk", "body": "{\"workflow\":\"research\",\"stage\":\"design-interface\",\"args\":{\"directive\":\"d\"}}" }),
    ]);
    let staged = staged_children(
        vec![
            json!({ "ev": "add", "id": "design-interface0", "kind": "fact", "parent": "chunk", "title": "API 경계", "category": "interface", "badge": "o" }),
            json!({ "ev": "add", "id": "design-domain", "kind": "task", "parent": "chunk", "stage": "design-domain", "title": "도메인 모델", "blocked_by": ["design-interface0"] }),
        ],
        Value::Null,
    );
    let d = FakeDeps::new(ns).stage(staged).ledger(vec![]).facts(vec![]);
    tick(&d);
    let add = &d.c().add;
    assert_eq!(add[0]["kind"], "section");
    assert_eq!(
        add[0]["title"], "Design",
        "design-interface 는 Design 섹션 발행"
    );
    assert_eq!(add[0]["collapsed"], true);
    assert_eq!(add[1]["kind"], "fact");
    assert_eq!(
        add[1]["parentId"], "k-1",
        "interface fact 는 Design 섹션 밑"
    );
    assert_eq!(
        add[2]["parentId"], "chunk",
        "design-domain task 는 chunk 직속"
    );
}

#[test]
fn design_stages_share_one_design_section() {
    // Design 3스테이지(interface/domain/criteria)는 하나의 Design 섹션을 공유 — 멱등 find-or-create.
    // 이미 Design 섹션이 있으면 재사용(새 섹션 발행 금지), 프레임은 그 밑에 매단다.
    let ns = nodes(vec![
        json!({ "id": "sec-design", "kind": "section", "parentId": "chunk", "title": "Design" }),
        json!({ "id": "design-domain", "kind": "task", "status": "todo", "blockedBy": [], "parentId": "chunk", "body": "{\"workflow\":\"research\",\"stage\":\"design-domain\",\"args\":{\"directive\":\"d\"}}" }),
    ]);
    let staged = staged_children(
        vec![
            json!({ "ev": "add", "id": "design-domain0", "kind": "fact", "parent": "chunk", "title": "주문 애그리게잇", "category": "domain-model", "badge": "o" }),
            json!({ "ev": "add", "id": "design-criteria", "kind": "task", "parent": "chunk", "stage": "design-criteria", "title": "수용 기준", "blocked_by": ["design-domain0"] }),
        ],
        Value::Null,
    );
    let d = FakeDeps::new(ns).stage(staged).ledger(vec![]).facts(vec![]);
    tick(&d);
    let add = &d.c().add;
    assert!(
        add.iter().all(|p| p["kind"] != "section"),
        "기존 Design 섹션 재사용 — 중복 섹션 발행 금지"
    );
    let fact = add.iter().find(|p| p["kind"] == "fact").unwrap();
    assert_eq!(
        fact["parentId"], "sec-design",
        "domain fact 는 공유 Design 섹션 밑"
    );
    let crit = add.iter().find(|p| p["kind"] == "task").unwrap();
    assert_eq!(
        crit["parentId"], "chunk",
        "design-criteria task 는 chunk 직속"
    );
}

#[test]
fn plan_units_nest_under_plan_section() {
    let ns = nodes(vec![
        json!({ "id": "plan", "kind": "task", "status": "todo", "blockedBy": [], "parentId": "chunk", "body": "{\"workflow\":\"research\",\"stage\":\"plan\",\"args\":{\"directive\":\"d\"}}" }),
    ]);
    let staged = staged_children(
        vec![
            json!({ "ev": "add", "id": "unit0", "kind": "plan-unit", "parent": "chunk", "title": "재고 차감", "description": "PSEUDO", "category": "src/a.ts", "badge": "o" }),
        ],
        Value::Null,
    );
    let d = FakeDeps::new(ns).stage(staged).ledger(vec![]).facts(vec![]);
    tick(&d);
    let add = &d.c().add;
    assert_eq!(add[0]["kind"], "section");
    assert_eq!(add[0]["title"], "Plan");
    assert_eq!(add[0]["collapsed"], true);
    assert!(add[0].get("badge").is_none());
    assert_eq!(add[1]["kind"], "plan-unit", "프레임 kind 불변");
    assert_eq!(add[1]["parentId"], "k-1", "plan-unit 은 Plan 섹션 밑");
    assert!(
        add[1].get("collapsed").is_none(),
        "leaf 프레임엔 collapsed 없음"
    );
}

#[test]
fn pick_ready_excludes_section_and_ledger_descends_through_section() {
    // 가드레일 — 섹션은 pick_ready 대상 아님(kind=section, badge 없음). build_ledger 는 섹션-밑 프레임을
    // chunk 자손으로 수집(descends 부모 체인 2단: fact→section→chunk).
    let ns = nodes(vec![
        json!({ "id": "chunk", "kind": "chunk", "parentId": null, "badge": "o" }),
        json!({ "id": "sec", "kind": "section", "parentId": "chunk", "title": "Research" }),
        json!({ "id": "f0", "kind": "fact", "parentId": "sec", "title": "저장소", "badge": "검수전" }),
    ]);
    let ready = pick_ready(&ns);
    assert!(
        ready.iter().all(|n| n.kind.as_deref() != Some("section")),
        "섹션은 실행/검증 대상 아님"
    );
    let led = build_ledger(&ns, "chunk", "fact");
    assert_eq!(led.len(), 1, "섹션 밑 fact 도 chunk 자손 원장에 수집");
    assert_eq!(led[0]["id"], "f0");
}

// ── exportTick (chunk 4) ────────────────────────────────────────────────────
#[test]
fn export_tick_o_codes_only_proof_stripped() {
    let ns = nodes(vec![
        json!({ "id": "chunk", "kind": "chunk", "parentId": null, "badge": "o" }),
        json!({ "id": "c1", "kind": "code", "parentId": "chunk", "title": "src/a.ts", "badge": "o", "description": "export const a = 1;\n\n---- PROOF ----\ncommands: [\"tsc\"]" }),
        json!({ "id": "c2", "kind": "code", "parentId": "chunk", "title": "src/b.ts", "badge": "x", "description": "버려진 코드" }),
    ]);
    let d = FakeDeps::new(ns);
    let written = RefCell::new(Vec::<(String, String)>::new());
    // write_file 은 기본 no-op — 기록을 위해 별도 FakeDeps 확장 대신 결과만 검증.
    let r = export_tick(
        &ExportDeps {
            inner: d,
            written: &written,
        },
        "chunk",
        "/tmp/out",
    );
    assert_eq!(r["ok"], true);
    assert_eq!(r["files"], json!(["src/a.ts"]));
    let w = written.borrow();
    assert_eq!(w[0].0, "src/a.ts");
    assert!(w[0].1.contains("export const a = 1;"));
    assert!(!w[0].1.contains("PROOF"));
}

// export 는 write_file 을 기록해야 하므로 얇은 래퍼로 위임.
struct ExportDeps<'a> {
    inner: FakeDeps,
    written: &'a RefCell<Vec<(String, String)>>,
}
impl Deps for ExportDeps<'_> {
    fn list_nodes(&self) -> Vec<Node> {
        self.inner.list_nodes()
    }
    fn get_node(&self, id: &str) -> Option<Node> {
        self.inner.get_node(id)
    }
    fn edit_node(&self, id: &str, f: Value) -> EditResult {
        self.inner.edit_node(id, f)
    }
    fn add_node(&self, p: Value) -> Option<String> {
        self.inner.add_node(p)
    }
    fn poke(&self) {
        self.inner.poke()
    }
    fn exec_one(&self, b: &str) -> Result<Value, String> {
        self.inner.exec_one(b)
    }
    fn exec_stage(&self, b: &str) -> Result<StageOut, String> {
        self.inner.exec_stage(b)
    }
    fn materialize_ledger(&self, c: &str) -> Result<Vec<Value>, String> {
        self.inner.materialize_ledger(c)
    }
    fn materialize_facts(&self, c: &str) -> Result<Vec<Value>, String> {
        self.inner.materialize_facts(c)
    }
    fn put_prompt(&self, v: Value) -> Option<String> {
        self.inner.put_prompt(v)
    }
    fn write_file(&self, rel: &str, content: &str) {
        self.written
            .borrow_mut()
            .push((rel.to_string(), content.to_string()));
    }
}

#[test]
fn export_tick_pending_code_rejected() {
    let ns = nodes(vec![
        json!({ "id": "chunk", "kind": "chunk", "parentId": null, "badge": "o" }),
        json!({ "id": "c1", "kind": "code", "parentId": "chunk", "title": "src/a.ts", "badge": "검수전", "description": "code" }),
    ]);
    let r = export_tick(&FakeDeps::new(ns), "chunk", "/tmp/out");
    assert_eq!(r["ok"], false);
    assert!(r["message"].as_str().unwrap().contains("미확정 code 1건"));
}

#[test]
fn export_tick_no_code_gate() {
    let ns = nodes(vec![
        json!({ "id": "chunk", "kind": "chunk", "parentId": null, "badge": "o" }),
    ]);
    let r = export_tick(&FakeDeps::new(ns), "chunk", "/tmp/out");
    assert_eq!(r["ok"], false);
    assert!(r["message"].as_str().unwrap().contains("code 노드 없음"));
}

#[test]
fn export_tick_path_escape_rejected() {
    for bad in ["/etc/passwd", "../evil.ts", "a/../../evil.ts"] {
        let ns = nodes(vec![
            json!({ "id": "chunk", "kind": "chunk", "parentId": null, "badge": "o" }),
            json!({ "id": "c1", "kind": "code", "parentId": "chunk", "title": bad, "badge": "o", "description": "x" }),
        ]);
        let r = export_tick(&FakeDeps::new(ns), "chunk", "/tmp/out");
        assert_eq!(r["ok"], false, "{bad}");
        assert_eq!(r["code"], "INVALID_INPUT", "{bad}");
    }
}

// ── PROOF: parse_proof ──────────────────────────────────────────────────────
#[test]
fn parse_proof_extracts_commands_and_pass_condition() {
    let desc = "export const a = 1;\n\n---- PROOF ----\ncommands: [\"tsc\",\"npm test\"]\npass_condition: exit 0\nimplements: [\"i1\"]";
    let pb = parse_proof(desc).expect("PROOF 블록");
    assert_eq!(pb.commands, vec!["tsc", "npm test"]);
    assert_eq!(pb.pass_condition, "exit 0");
}

#[test]
fn parse_proof_none_without_marker() {
    assert!(parse_proof("just code, no proof block").is_none());
}

#[test]
fn parse_proof_empty_commands_when_default_array() {
    let pb = parse_proof("code\n\n---- PROOF ----\ncommands: []\npass_condition: ").expect("블록");
    assert!(pb.commands.is_empty());
    assert_eq!(pb.pass_condition, "");
}

#[test]
fn parse_proof_survives_code_containing_marker_word() {
    // 코드 본문에 "commands" 라는 낱말이 있어도 마커 뒤에서만 읽는다.
    let desc = "// commands here are fake\ncode\n\n---- PROOF ----\ncommands: [\"cargo test\"]\npass_condition: all tests pass";
    let pb = parse_proof(desc).expect("블록");
    assert_eq!(pb.commands, vec!["cargo test"]);
    assert_eq!(pb.pass_condition, "all tests pass");
}

// ── PROOF: evaluate_pass_condition ──────────────────────────────────────────
#[test]
fn evaluate_default_all_exit_zero_pass() {
    let v = evaluate_pass_condition("all tests pass", &[outc(0), outc(0)]);
    assert!(v.pass);
}

#[test]
fn evaluate_default_any_nonzero_fail() {
    let v = evaluate_pass_condition("all tests pass", &[outc(0), outc(1)]);
    assert!(!v.pass);
}

#[test]
fn evaluate_explicit_exit_zero() {
    assert!(evaluate_pass_condition("exit 0", &[outc(0)]).pass);
    assert!(!evaluate_pass_condition("exit code 0", &[outc(2)]).pass);
}

#[test]
fn evaluate_explicit_nonzero_exit_matches_last() {
    // 실패를 증명하는 PROOF — 마지막 명령이 그 코드로 끝나야 pass.
    assert!(evaluate_pass_condition("exit 1", &[outc(0), outc(1)]).pass);
    assert!(!evaluate_pass_condition("exit 1", &[outc(0), outc(0)]).pass);
}

#[test]
fn evaluate_contains_directive_checks_output() {
    let ok = CmdOutcome {
        exit_code: Some(0),
        stdout: "3 tests passed".to_string(),
        stderr: String::new(),
    };
    // 대소문자 무시 부분 문자열 — "passed" 는 있고 "compilation error" 는 없다.
    assert!(evaluate_pass_condition("output contains \"PASSED\"", &[ok.clone()]).pass);
    assert!(!evaluate_pass_condition("output contains \"compilation error\"", &[ok]).pass);
}

#[test]
fn evaluate_no_exit_code_is_fail() {
    let timed_out = CmdOutcome {
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
    };
    assert!(!evaluate_pass_condition("exit 0", &[timed_out]).pass);
}

#[test]
fn evaluate_empty_outcomes_is_fail() {
    assert!(!evaluate_pass_condition("exit 0", &[]).pass);
}

// ── PROOF: proof_edit_fields ────────────────────────────────────────────────
#[test]
fn proof_edit_fields_writes_proof_axis_not_badge() {
    // 실행 축은 정적 badge 축을 건드리지 않는다 — `proof` 필드에만 쓴다.
    let f = proof_edit_fields("pass", "기대 exit 0", &["cargo test".to_string()]);
    assert_eq!(f["proof"]["status"], "pass");
    assert_eq!(f["proof"]["reason"], "기대 exit 0");
    assert_eq!(f["proof"]["commands"], json!(["cargo test"]));
    assert!(f.get("badge").is_none(), "badge 축은 건드리지 않는다");
}

// ── PROOF: proof_tick ───────────────────────────────────────────────────────
fn proof_desc(commands: &str, pass: &str) -> String {
    format!("code body\n\n---- PROOF ----\ncommands: {commands}\npass_condition: {pass}")
}

#[test]
fn proof_tick_no_code_gate() {
    let ns = nodes(vec![
        json!({ "id": "chunk", "kind": "chunk", "parentId": null, "badge": "o" }),
    ]);
    let r = proof_tick(&FakeDeps::new(ns), "chunk", "/tmp/out");
    assert_eq!(r["ok"], false);
    assert!(r["message"].as_str().unwrap().contains("code 노드 없음"));
}

#[test]
fn proof_tick_pending_code_gate() {
    let ns = nodes(vec![
        json!({ "id": "chunk", "kind": "chunk", "parentId": null, "badge": "o" }),
        json!({ "id": "c1", "kind": "code", "parentId": "chunk", "title": "a.rs", "badge": "검수전", "description": "x" }),
    ]);
    let r = proof_tick(&FakeDeps::new(ns), "chunk", "/tmp/out");
    assert_eq!(r["ok"], false);
    assert_eq!(r["code"], "GATE_REQUIRED");
}

#[test]
fn proof_tick_gated_when_runner_unwired() {
    // 기본 FakeDeps 는 run_proof_command 미배선(None) — 아무 명령도 스폰하지 않고 gated 로 기록.
    let ns = nodes(vec![
        json!({ "id": "chunk", "kind": "chunk", "parentId": null, "badge": "o" }),
        json!({ "id": "c1", "kind": "code", "parentId": "chunk", "title": "a.rs", "badge": "o", "description": proof_desc("[\"cargo test\"]", "exit 0") }),
    ]);
    let d = FakeDeps::new(ns);
    let r = proof_tick(&d, "chunk", "/tmp/out");
    assert_eq!(r["ok"], true);
    assert_eq!(r["gated"], 1);
    assert_eq!(r["passed"], 0);
    let edit = d.c().edit_of("c1").cloned().expect("c1 edit");
    assert_eq!(edit["proof"]["status"], "gated");
}

#[test]
fn proof_tick_runs_and_records_pass() {
    let ns = nodes(vec![
        json!({ "id": "chunk", "kind": "chunk", "parentId": null, "badge": "o" }),
        json!({ "id": "c1", "kind": "code", "parentId": "chunk", "title": "a.rs", "badge": "o", "description": proof_desc("[\"cargo test\"]", "exit 0") }),
    ]);
    let d = FakeDeps::new(ns).proof(vec![("cargo test", outc(0))]);
    let r = proof_tick(&d, "chunk", "/tmp/out");
    assert_eq!(r["passed"], 1);
    assert_eq!(r["failed"], 0);
    assert_eq!(r["results"][0]["status"], "pass");
    assert_eq!(d.c().edit_of("c1").unwrap()["proof"]["status"], "pass");
}

#[test]
fn proof_tick_runs_and_records_fail() {
    let ns = nodes(vec![
        json!({ "id": "chunk", "kind": "chunk", "parentId": null, "badge": "o" }),
        json!({ "id": "c1", "kind": "code", "parentId": "chunk", "title": "a.rs", "badge": "o", "description": proof_desc("[\"cargo test\"]", "exit 0") }),
    ]);
    let d = FakeDeps::new(ns).proof(vec![("cargo test", outc(1))]);
    let r = proof_tick(&d, "chunk", "/tmp/out");
    assert_eq!(r["failed"], 1);
    assert_eq!(r["results"][0]["status"], "fail");
    assert_eq!(d.c().edit_of("c1").unwrap()["proof"]["status"], "fail");
}

#[test]
fn proof_tick_only_o_codes_and_no_proof_recorded() {
    // x 확정 code 는 제외; PROOF 블록 없는 o code 는 no-proof.
    let ns = nodes(vec![
        json!({ "id": "chunk", "kind": "chunk", "parentId": null, "badge": "o" }),
        json!({ "id": "c1", "kind": "code", "parentId": "chunk", "title": "a.rs", "badge": "o", "description": "코드만, PROOF 없음" }),
        json!({ "id": "c2", "kind": "code", "parentId": "chunk", "title": "b.rs", "badge": "x", "description": proof_desc("[\"cargo test\"]", "exit 0") }),
    ]);
    let d = FakeDeps::new(ns).proof(vec![("cargo test", outc(0))]);
    let r = proof_tick(&d, "chunk", "/tmp/out");
    assert_eq!(r["results"].as_array().unwrap().len(), 1, "o code 만 대상");
    assert_eq!(r["results"][0]["node"], "c1");
    assert_eq!(r["results"][0]["status"], "no-proof");
    assert!(d.c().edit_of("c2").is_none(), "x code 는 실행 대상 아님");
}

// ── issuerizeTick (chunk 4) ─────────────────────────────────────────────────
fn issuerize_nodes() -> Vec<Node> {
    nodes(vec![
        json!({ "id": "chunk", "kind": "chunk", "parentId": null, "badge": "o", "status": "todo", "description": "정련 지시 전문" }),
        json!({ "id": "i0", "kind": "item", "parentId": "chunk", "title": "요건", "badge": "o" }),
        json!({ "id": "f0", "kind": "fact", "parentId": "chunk", "title": "프레임워크: X 채택", "badge": "o" }),
        json!({ "id": "f1", "kind": "fact", "parentId": "chunk", "title": "방법론: 근거 부족", "badge": "x" }),
        json!({ "id": "u0", "kind": "plan-unit", "parentId": "chunk", "title": "재고 차감 구현", "description": "PSEUDO:\n차감(order)…", "category": "src/deduct.ts", "badge": "o" }),
        json!({ "id": "u1", "kind": "plan-unit", "parentId": "chunk", "title": "동기화 구현", "description": "PSEUDO:\nsync()…", "category": "src/sync.ts", "badge": "x" }),
    ])
}

#[test]
fn issuerize_gate_pass_issues_o_units() {
    let d = FakeDeps::new(issuerize_nodes());
    let r = issuerize_tick(&d, "chunk");
    assert_eq!(r["ok"], true);
    assert_eq!(r["issued"], 1);
    let first = &d.c().add[0];
    assert_eq!(first["kind"], "task");
    assert_eq!(first["parentId"], "chunk");
    let body: Value = serde_json::from_str(first["body"].as_str().unwrap()).unwrap();
    assert_eq!(body["workflow"], "research");
    assert_eq!(body["stage"], "body");
    assert_eq!(body["args"]["file_path"], "src/deduct.ts");
    assert!(body["args"]["pseudocode"]
        .as_str()
        .unwrap()
        .contains("PSEUDO"));
    assert_eq!(body["args"]["directive"], "정련 지시 전문");
    assert_eq!(body["args"]["chunkRef"], "chunk");
}

#[test]
fn issuerize_unverified_unit_rejected() {
    let mut ns = issuerize_nodes();
    ns[4].badge = Some("검수전".into());
    let r = issuerize_tick(&FakeDeps::new(ns), "chunk");
    assert_eq!(r["ok"], false);
    assert!(r["message"].as_str().unwrap().contains("유닛 미검증"));
}

#[test]
fn issuerize_unconfirmed_chunk_rejected() {
    let mut ns = issuerize_nodes();
    ns[0].badge = Some("f".into());
    let d = FakeDeps::new(ns);
    let r = issuerize_tick(&d, "chunk");
    assert_eq!(r["ok"], false);
    assert!(r["message"].as_str().unwrap().contains("미인증"));
    assert_eq!(d.c().add.len(), 0);
}

#[test]
fn issuerize_gate_missing_stages() {
    let no_facts: Vec<Node> = issuerize_nodes()
        .into_iter()
        .filter(|n| n.kind.as_deref() != Some("fact"))
        .collect();
    assert!(issuerize_tick(&FakeDeps::new(no_facts), "chunk")["message"]
        .as_str()
        .unwrap()
        .contains("research 미경유"));
    let mut pending = issuerize_nodes();
    pending[2].badge = Some("검수전".into());
    assert!(issuerize_tick(&FakeDeps::new(pending), "chunk")["message"]
        .as_str()
        .unwrap()
        .contains("미검증 1건"));
    let no_units: Vec<Node> = issuerize_nodes()
        .into_iter()
        .filter(|n| n.kind.as_deref() != Some("plan-unit"))
        .collect();
    assert!(issuerize_tick(&FakeDeps::new(no_units), "chunk")["message"]
        .as_str()
        .unwrap()
        .contains("plan 미경유"));
}

#[test]
fn issuerize_idempotent_when_covered() {
    let mut with_code = issuerize_nodes();
    with_code.push(node(json!({ "id": "c0", "kind": "code", "parentId": "chunk", "title": "src/deduct.ts", "category": "src/deduct.ts", "badge": "검수전" })));
    assert!(
        issuerize_tick(&FakeDeps::new(with_code), "chunk")["message"]
            .as_str()
            .unwrap()
            .contains("이미 이슈라이즈")
    );
    let mut with_task = issuerize_nodes();
    with_task.push(node(json!({ "id": "t0", "kind": "task", "parentId": "chunk", "title": "실코드화", "status": "todo", "body": "{\"workflow\":\"research\",\"stage\":\"body\",\"args\":{\"file_path\":\"src/deduct.ts\"}}" })));
    let d = FakeDeps::new(with_task);
    assert!(issuerize_tick(&d, "chunk")["message"]
        .as_str()
        .unwrap()
        .contains("이미 이슈라이즈"));
    assert_eq!(d.c().add.len(), 0);
}

#[test]
fn issuerize_rework_on_rejected_code() {
    let mut ns = issuerize_nodes();
    ns.push(node(json!({ "id": "c0", "kind": "code", "parentId": "chunk", "title": "src/deduct.ts", "category": "src/deduct.ts", "badge": "f", "result": "{\"oxf\":\"f\",\"reason\":\"store 계약 위반 — mutate 반환형 오용\"}" })));
    let d = FakeDeps::new(ns);
    let r = issuerize_tick(&d, "chunk");
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(d.c().add.len(), 1);
    let body: Value = serde_json::from_str(d.c().add[0]["body"].as_str().unwrap()).unwrap();
    assert_eq!(body["args"]["file_path"], "src/deduct.ts");
    assert!(body["args"]["pseudocode"]
        .as_str()
        .unwrap()
        .contains("store 계약 위반"));
}

// ── issuerize: Draft done + 작업 task unlocked (보드 모델 — 두 축) ────────────
#[test]
fn issuerize_marks_chunk_done() {
    // 게이트 통과(플랜 완결·fact/plan-unit 전부 확정) = Draft chunk 완료. badge 축(검증) 아닌 status 축(완료).
    let d = FakeDeps::new(issuerize_nodes());
    assert_eq!(issuerize_tick(&d, "chunk")["ok"], true);
    assert_eq!(d.c().edit_of("chunk").unwrap()["status"], "done");
}

#[test]
fn issuerize_work_task_unlocked() {
    // 팬아웃 작업 task 는 스펙 프레임(locked)과 대비되는 분리·성장 가능한 정상 노드 — locked 금지.
    let d = FakeDeps::new(issuerize_nodes());
    issuerize_tick(&d, "chunk");
    assert_eq!(d.c().add[0]["locked"], false);
}

#[test]
fn issuerize_gate_fail_keeps_chunk_status() {
    // 게이트 미통과(유닛 미검증) — 완결 전 Draft chunk done 이동 금지.
    let mut ns = issuerize_nodes();
    ns[4].badge = Some("검수전".into());
    let d = FakeDeps::new(ns);
    issuerize_tick(&d, "chunk");
    assert!(d.c().edit_of("chunk").is_none());
}

// ── researchGate (chunk 4) ──────────────────────────────────────────────────
fn gate_deps(nodes: Vec<Node>, bodies: Vec<(&str, &str)>) -> FakeDeps {
    let bmap: HashMap<String, String> = bodies
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let mut ns = nodes;
    for n in &mut ns {
        if let Some(b) = bmap.get(&n.id) {
            n.body = Some(b.clone());
        }
    }
    FakeDeps::new(ns)
}

#[test]
fn research_gate_pass() {
    let ns = nodes(vec![
        json!({ "id": "chunk", "kind": "chunk", "badge": "o", "description": "정련 지시" }),
    ]);
    let g = research_gate(&gate_deps(ns, vec![]), "chunk");
    assert_eq!(g["ok"], true);
    assert_eq!(g["directive"], "정련 지시");
}

#[test]
fn research_gate_rejections() {
    assert!(research_gate(&FakeDeps::new(vec![]), "chunk")["message"]
        .as_str()
        .unwrap()
        .contains("미존재"));
    let unconfirmed = nodes(vec![
        json!({ "id": "chunk", "badge": "검수전", "description": "d" }),
    ]);
    assert!(
        research_gate(&FakeDeps::new(unconfirmed), "chunk")["message"]
            .as_str()
            .unwrap()
            .contains("미인증")
    );
    let no_desc = nodes(vec![
        json!({ "id": "chunk", "badge": "o", "description": " " }),
    ]);
    assert!(research_gate(&FakeDeps::new(no_desc), "chunk")["message"]
        .as_str()
        .unwrap()
        .contains("비어있음"));
}

#[test]
fn research_gate_idempotent() {
    let with_fact = nodes(vec![
        json!({ "id": "chunk", "kind": "chunk", "badge": "o", "description": "d" }),
        json!({ "id": "f0", "kind": "fact", "parentId": "chunk" }),
    ]);
    assert!(research_gate(&FakeDeps::new(with_fact), "chunk")["message"]
        .as_str()
        .unwrap()
        .contains("fact 존재"));
    let with_task = nodes(vec![
        json!({ "id": "chunk", "kind": "chunk", "badge": "o", "description": "d" }),
        json!({ "id": "t1", "kind": "task", "parentId": "chunk" }),
    ]);
    let g = research_gate(
        &gate_deps(
            with_task,
            vec![("t1", "{\"workflow\":\"research\",\"stage\":\"research\"}")],
        ),
        "chunk",
    );
    assert!(g["message"]
        .as_str()
        .unwrap()
        .contains("research task 발행됨"));
}

// ── stagePublishedMarker ────────────────────────────────────────────────────
#[test]
fn stage_published_marker_variants() {
    let target =
        node(json!({ "id": "gen", "kind": "task", "parentId": "chunk", "blockedBy": ["i1"] }));
    // generate: 부모에 다른 task 있으면 발행됨.
    let ns = nodes(vec![
        json!({ "id": "gen", "kind": "task", "parentId": "chunk" }),
        json!({ "id": "hunt", "kind": "task", "parentId": "chunk" }),
    ]);
    assert!(stage_published_marker(&target, "{}", "generate", &ns));
    // body: file_path 일치 code(badge≠f/x) 있으면 발행됨.
    let bodyt = node(json!({ "id": "b", "kind": "task", "parentId": "chunk" }));
    let ns3 = nodes(vec![
        json!({ "id": "c1", "kind": "code", "parentId": "chunk", "category": "src/x.rs", "badge": "o" }),
    ]);
    assert!(stage_published_marker(
        &bodyt,
        r#"{"args":{"file_path":"src/x.rs"}}"#,
        "body",
        &ns3
    ));
    // body: f 코드는 마커 아님(재작업 대상).
    let ns4 = nodes(vec![
        json!({ "id": "c1", "kind": "code", "parentId": "chunk", "category": "src/x.rs", "badge": "f" }),
    ]);
    assert!(!stage_published_marker(
        &bodyt,
        r#"{"args":{"file_path":"src/x.rs"}}"#,
        "body",
        &ns4
    ));
}

#[test]
fn with_routing_injects_node_effort_and_model() {
    // 저작 LLM 이 노드에 실은 tier → exec 입력 JSON 에 effort/model 주입.
    let n = node(json!({ "id": "i1", "kind": "item", "effort": "low", "model": "gpt-5.6-luna" }));
    let out = with_routing(
        r#"{"prompt":"p","schema":{"type":"object"}}"#.to_string(),
        &n,
    );
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["effort"], "low");
    assert_eq!(v["model"], "gpt-5.6-luna");
    assert_eq!(v["prompt"], "p", "기존 필드 보존");
    assert!(v["schema"].is_object());
}

#[test]
fn with_routing_noop_when_node_has_no_tier() {
    // 미지정 노드는 무주입 → 실행자 기본(최고, 품질우선). 문자열 불변.
    let n = node(json!({ "id": "i1", "kind": "item" }));
    let body = r#"{"prompt":"p"}"#.to_string();
    assert_eq!(with_routing(body.clone(), &n), body);
}
