//! 워크플로 오케스트레이션의 틱 로직, 순수 헬퍼, Deps 경계.
//!
//! 상주 서비스가 상태와 커맨드 처리를 소유한다. Deps는 board/scheduler 중개 호출과
//! in-process provider/doc_exec를 추상화한다. Emit.call이 동기이므로 Deps도 동기다.
//! 크로스 플러그인 읽기는 `{ok,data}` 봉투를 해석하고 ok:false는 None이다.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

/// 무판정(oxf 없음) 연속 상한 — 도달 시 badge=f로 확정한다.
pub const NO_VERDICT_MAX: u32 = 3;
/// next lease 수명(ms) — CLI 실행자가 노드를 잡는 기간(30분).
pub const NEXT_LEASE_MS: u64 = 30 * 60 * 1000;
/// 합의 재검(-again) 자기재발행 라운드 상한 — 도달 시 재발행 대신 chunk 를 badge=f 로 봉인(무한 루프 차단).
/// 프롬프트의 "round N of max 20" 을 실제 집행한다.
pub const CONSENSUS_ROUND_MAX: u32 = 20;
/// 설계 팩트 카테고리 — 한 chunk 를 research·design 이 공유하므로, design-audit 는 이 셋에 드는 fact 만,
/// research-audit 는 그 밖 fact 만 검토 대상으로 스코프한다.
const DESIGN_FACT_CATS: [&str; 3] = ["interface", "domain-model", "criterion"];

/// 칸반 노드 — kanban IPC 가 돌려주는 JSON 을 역직렬화. JS 는 camelCase(blockedBy/parentId).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Node {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub badge: Option<String>,
    #[serde(default, rename = "blockedBy", alias = "blocked_by")]
    pub blocked_by: Vec<String>,
    #[serde(
        default,
        rename = "parentId",
        alias = "parent_id",
        skip_serializing_if = "Option::is_none"
    )]
    pub parent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 검증 결과(JSON 문자열 등) — issuerize 가 반려 사유(result.reason) 를 읽는다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// 라우팅(자기선택) — 저작 LLM 이 이 노드의 난이도로 실은 tier. exec 입력에 실려 실행자가 honor.
    /// 미지정이면 실행자 기본(최고, 품질우선). model 은 provider 별 별칭/식별자.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl Node {
    fn badge_str(&self) -> &str {
        self.badge.as_deref().unwrap_or("")
    }
    fn body_str(&self) -> &str {
        self.body.as_deref().unwrap_or("")
    }
}

/// exec-stage 출력 3형. --assemble은 별도 반환이라 여기 없다.
#[derive(Debug, Clone)]
pub enum StageOut {
    /// generate/draft-chunk → DraftDoc 객체.
    DraftDoc(Value),
    /// 일반 stage → 자식 add 이벤트 스트림 + result.
    Children { children: Vec<Value>, result: Value },
}

/// editNode 결과 — consumeStageOutput 이 ok:false 를 검사하므로 봉투를 보존.
#[derive(Debug, Clone)]
pub struct EditResult {
    pub ok: bool,
    pub message: Option<String>,
}
impl EditResult {
    pub fn ok() -> Self {
        Self {
            ok: true,
            message: None,
        }
    }
    pub fn err(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            message: Some(message.into()),
        }
    }
}

/// 오케스트레이션 의존 경계. production은 중개 cmd+in-process exec, 테스트는 FakeDeps다.
/// presence 를 JS 가 검사하던 seam(assemble_stage 등)은 Option 반환으로 "미배선"을 표현(기본 None).
pub trait Deps {
    fn list_nodes(&self) -> Vec<Node>;
    fn get_node(&self, id: &str) -> Option<Node>;
    fn edit_node(&self, id: &str, fields: Value) -> EditResult;
    fn add_node(&self, params: Value) -> Option<String>;
    fn poke(&self);
    /// 진행 델타(선택) — item 검증 중 무엇을 검증 중인지 흘린다. 기본 no-op.
    fn progress(&self, _cmd: &str, _delta: &str) {}

    // exec seam — production=in-process provider/doc_exec. Err=throw(멱등: 노드 미변경).
    fn exec_one(&self, body: &str) -> Result<Value, String>;
    fn exec_stage(&self, body: &str) -> Result<StageOut, String>;

    // ledger/facts — kanban node.list 필터. Err=materialize 실패(throw).
    fn materialize_ledger(&self, chunk_id: &str) -> Result<Vec<Value>, String>;
    fn materialize_facts(&self, chunk_id: &str) -> Result<Vec<Value>, String>;

    // prompt 저장/해소 — kanban prompt.*.
    fn put_prompt(&self, value: Value) -> Option<String>;
    fn resolve_prompt(&self, _hash: &str, _vars: Value, _refs: Value) -> Option<Value> {
        None
    }
    fn get_prompt(&self, _hash: &str) -> Option<Value> {
        None
    }

    // pull(next/submit) seam — 배선 여부는 has_* 로 질의(JS 의 deps.assembleStage 존재 검사 대응).
    // 미배선(기본)이면 검증 노드 경로. 프로브 호출 금지(production 이 빈 body 로 조립하는 오류).
    fn has_assemble_stage(&self) -> bool {
        false
    }
    fn assemble_stage(&self, _body: &str) -> Result<Value, String> {
        Err("assembleStage 미배선".into())
    }
    fn has_exec_stage_with_output(&self) -> bool {
        false
    }
    fn exec_stage_with_output(&self, _body: &str, _out: Value) -> Result<StageOut, String> {
        Err("execStageWithOutput 미배선".into())
    }

    // export — 파일 쓰기.
    fn write_file(&self, _rel: &str, _content: &str) {}
}

/// 활성 프로세스 수명 동안 유지하는 상태. 재시작 시 리셋할 수 있다.
#[derive(Default)]
pub struct ReconcileState {
    /// 항목별 연속 무판정 카운터(캡 NO_VERDICT_MAX).
    pub no_verdict: HashMap<String, u32>,
    /// 노드별 연속 실패 카운터(head-of-line 기아 방지).
    pub fails: HashMap<String, u32>,
    /// 노드별 lease 만료 epoch(ms) — CLI 실행자 점유.
    pub leases: HashMap<String, u64>,
    /// stage 조립 문맥(next 가 잡고 submit 이 재생).
    pub stage_ctx: HashMap<String, StageCtx>,
}

#[derive(Clone, Debug)]
pub struct StageCtx {
    pub stage_body: String,
    pub stage_name: String,
    pub ledger: Option<Vec<Value>>,
    pub body: String,
}

/// lease 활성 판정(만료 시 lazy 삭제).
pub fn lease_active(state: &mut ReconcileState, node_id: &str, now_ms: u64) -> bool {
    match state.leases.get(node_id).copied() {
        None => false,
        Some(exp) if exp <= now_ms => {
            state.leases.remove(node_id);
            false
        }
        Some(_) => true,
    }
}

// ── 순수 헬퍼 ────────────────────────────────────────────────────────────────

/// done 판정 — badge o/x/f면 done, 아니면 status==="done".
pub fn is_done(node: Option<&Node>) -> bool {
    let Some(n) = node else { return false };
    let b = n.badge_str();
    if !b.is_empty() {
        return b == "o" || b == "x" || b == "f";
    }
    n.status.as_deref() == Some("done")
}

// 부모 사슬로 chunk_id 자손인가(guard 100).
fn descends(by_id: &HashMap<String, &Node>, node: &Node, chunk_id: &str) -> bool {
    let mut p = node.parent_id.clone();
    let mut guard = 0;
    while let Some(pid) = p {
        if guard >= 100 {
            break;
        }
        guard += 1;
        if pid == chunk_id {
            return true;
        }
        p = by_id.get(&pid).and_then(|n| n.parent_id.clone());
    }
    false
}

/// ready 노드 선택 — blockedBy가 전부 done인 미완 실행 대상.
/// 항목(badge=검수전 ∧ leaf) 또는 stage 작업(kind=task ∧ status≠done, #6 audit 게이트).
pub fn pick_ready(nodes: &[Node]) -> Vec<Node> {
    let by_id: HashMap<String, &Node> = nodes.iter().map(|n| (n.id.clone(), n)).collect();
    let mut has_child: HashSet<String> = HashSet::new();
    for n in nodes {
        if let Some(p) = &n.parent_id {
            has_child.insert(p.clone());
        }
    }
    let deps_done = |n: &Node| n.blocked_by.iter().all(|b| is_done(by_id.get(b).copied()));
    let chunk_has_pending = |chunk_id: &str| {
        nodes.iter().any(|n| {
            n.kind.as_deref() == Some("item")
                && n.badge_str() == "검수전"
                && descends(&by_id, n, chunk_id)
        })
    };
    let depends_on_task = |n: &Node| {
        n.blocked_by.iter().any(|b| {
            by_id
                .get(b)
                .map(|m| m.kind.as_deref() == Some("task"))
                .unwrap_or(false)
        })
    };
    nodes
        .iter()
        .filter(|n| {
            if !deps_done(n) {
                return false;
            }
            if n.badge_str() == "검수전" && !has_child.contains(&n.id) {
                return true; // 항목 검증
            }
            if n.kind.as_deref() == Some("task") && n.status.as_deref() != Some("done") {
                // #6 audit 게이트 — 덩어리에 검수전 항목 남아 있으면 not-ready.
                if let Some(pid) = &n.parent_id {
                    if depends_on_task(n) && chunk_has_pending(pid) {
                        return false;
                    }
                }
                return true; // stage 작업 실행
            }
            false
        })
        .cloned()
        .collect()
}

/// buildLedger — 덩어리 자손 중 지정 kind를 ledger 엔트리로 만든다.
pub fn build_ledger(nodes: &[Node], chunk_id: &str, kind: &str) -> Vec<Value> {
    let by_id: HashMap<String, &Node> = nodes.iter().map(|n| (n.id.clone(), n)).collect();
    nodes
        .iter()
        .filter(|n| n.kind.as_deref() == Some(kind) && descends(&by_id, n, chunk_id))
        .map(|n| {
            json!({
                "id": n.id,
                "title": n.title,
                "description": n.description,
                "badge": n.badge,
                "category": n.category,
            })
        })
        .collect()
}

/// 진척 롤업 계수 대상 — badge 를 지니는 프레임 3종(fact/item/plan-unit). 섹션/그룹/task 는 제외.
fn is_frame_kind(kind: Option<&str>) -> bool {
    matches!(kind, Some("fact") | Some("item") | Some("plan-unit"))
}

/// 프레임의 조상 chunk(kind=chunk)를 부모 사슬로 찾는다. 섹션-밑 프레임도 chunk 까지 올라간다(guard 100).
fn ancestor_chunk<'a>(by_id: &HashMap<String, &'a Node>, node: &Node) -> Option<&'a Node> {
    let mut p = node.parent_id.clone();
    let mut guard = 0;
    while let Some(pid) = p {
        if guard >= 100 {
            break;
        }
        guard += 1;
        let parent = by_id.get(&pid).copied()?;
        if parent.kind.as_deref() == Some("chunk") {
            return Some(parent);
        }
        p = parent.parent_id.clone();
    }
    None
}

/// chunk 진척 문자열 — 자손 프레임을 badge 로 집계한 "확정 N/M"(확정=o/x/f, 분모=전체 프레임).
/// override_id 프레임은 이번 틱에 막 확정됐으나 스냅샷이 아직 검수전이라 override_badge 로 계수에 반영한다.
/// 프레임 0개면 롤업 대상 아님(None).
fn chunk_progress_line(
    nodes: &[Node],
    chunk_id: &str,
    override_id: &str,
    override_badge: &str,
) -> Option<String> {
    let by_id: HashMap<String, &Node> = nodes.iter().map(|n| (n.id.clone(), n)).collect();
    let mut total = 0usize;
    let mut settled = 0usize;
    for n in nodes {
        if !is_frame_kind(n.kind.as_deref()) || !descends(&by_id, n, chunk_id) {
            continue;
        }
        total += 1;
        let badge = if n.id == override_id {
            override_badge
        } else {
            n.badge_str()
        };
        if matches!(badge, "o" | "x" | "f") {
            settled += 1;
        }
    }
    if total == 0 {
        return None;
    }
    Some(format!("확정 {settled}/{total}"))
}

/// exec-one {oxf,result} → node.edit 필드. oxf가 유효하면 badge를 갱신하고 result는 항상 기록한다.
pub fn exec_result_to_edit(exec_out: &Value) -> Value {
    let oxf = exec_out.get("oxf").and_then(|v| v.as_str());
    let raw = exec_out.get("result");
    let result = match raw {
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
        None => "null".to_string(),
    };
    match oxf {
        Some(o) if o == "o" || o == "x" || o == "f" => json!({ "badge": o, "result": result }),
        _ => json!({ "result": result }),
    }
}

/// stage 발행 멱등 마커 — 이미 발행됐으면 재실행하지 않는다.
pub fn stage_published_marker(target: &Node, body: &str, stage_name: &str, nodes: &[Node]) -> bool {
    let Some(parent_id) = &target.parent_id else {
        return false;
    };
    let child_of = |n: &Node| n.parent_id.as_ref() == Some(parent_id);
    match stage_name {
        "generate" => nodes
            .iter()
            .any(|n| child_of(n) && n.kind.as_deref() == Some("task") && n.id != target.id),
        "research" => nodes
            .iter()
            .any(|n| child_of(n) && n.kind.as_deref() == Some("fact")),
        "plan" => nodes
            .iter()
            .any(|n| child_of(n) && n.kind.as_deref() == Some("plan-unit")),
        "body" => {
            let fp = serde_json::from_str::<Value>(body).ok().and_then(|v| {
                v.get("args")
                    .and_then(|a| a.get("file_path"))
                    .and_then(|f| f.as_str())
                    .map(String::from)
            });
            match fp {
                Some(fp) => nodes.iter().any(|n| {
                    child_of(n)
                        && n.kind.as_deref() == Some("code")
                        && n.category.as_deref() == Some(fp.as_str())
                        && n.badge_str() != "f"
                        && n.badge_str() != "x"
                }),
                None => false,
            }
        }
        _ => false,
    }
}

/// directive 단일진실 — explicit > workflow-doc@0.0.1 refined > raw.
pub fn resolve_directive(
    explicit: Option<&str>,
    doc: Option<&Value>,
    raw: Option<&str>,
) -> Option<String> {
    if let Some(e) = explicit {
        if !e.trim().is_empty() {
            return Some(e.to_string());
        }
    }
    if let Some(d) = doc {
        if d.get("spec").and_then(|s| s.as_str()) == Some("workflow-doc@0.0.1") {
            if let Some(r) = d
                .pointer("/args/directive/default")
                .and_then(|v| v.as_str())
            {
                if !r.trim().is_empty() {
                    return Some(r.to_string());
                }
            }
        }
    }
    raw.map(String::from)
}

/// generate-skeleton CLI 인자 조립. idea는 필수다.
pub fn gen_skeleton_args(
    idea: Option<&str>,
    model: Option<&str>,
    refs: Option<&str>,
    gen_out: Option<&str>,
    lang: Option<&str>,
) -> Result<Vec<String>, String> {
    let idea = match idea {
        Some(i) if !i.is_empty() => i,
        _ => return Err("genSkeletonArgs: idea 필수".to_string()),
    };
    let mut args = vec![
        "generate-skeleton".to_string(),
        "--idea".to_string(),
        idea.to_string(),
        "--lang".to_string(),
        lang.unwrap_or("ko").to_string(),
    ];
    if let Some(m) = model {
        args.push("--model".into());
        args.push(m.into());
    }
    if let Some(r) = refs {
        args.push("--refs".into());
        args.push(r.into());
    }
    if let Some(g) = gen_out {
        args.push("--gen-out".into());
        args.push(g.into());
    }
    Ok(args)
}

/// secrets.keys() → spawn secretEnv 매핑(envVar→secretKey). "env:" prefix만 허용한다.
pub fn build_secret_env_map(keys: &[String]) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for k in keys {
        if let Some(env_var) = k.strip_prefix("env:") {
            if !env_var.is_empty() {
                m.insert(env_var.to_string(), k.clone());
            }
        }
    }
    m
}

/// spawn 명령 조립 — bin 명시면 직접, 기본은 "sidecar:workflow" 이름 참조.
pub fn build_spawn_cmd(bin: Option<&str>, args: Vec<String>) -> (String, Vec<String>) {
    match bin {
        Some(b) if !b.is_empty() => (b.to_string(), args),
        _ => ("sidecar:workflow".to_string(), args),
    }
}

/// node.add 파라미터 조립 — ev(add 이벤트) → board node.add params.
/// task_ctx: workflowRef|skeleton+directive 를 task body 에 임베드. role_to_hash: prompt role→hash 매핑.
pub fn build_add_params(
    ev: &Value,
    parent_id: Option<&str>,
    blocked_by: &[String],
    task_ctx: Option<&Value>,
    role_to_hash: &HashMap<String, String>,
) -> Value {
    let s = |k: &str| ev.get(k).and_then(|v| v.as_str());
    let kind = s("kind");
    let body: String;
    if kind == Some("task") {
        let stage = s("stage").unwrap_or("generate");
        let directive = task_ctx
            .and_then(|c| c.get("directive"))
            .cloned()
            .unwrap_or(Value::Null);
        body = if let Some(wref) = task_ctx
            .and_then(|c| c.get("workflowRef"))
            .and_then(|v| v.as_str())
        {
            json!({ "workflow": wref, "stage": stage, "args": { "directive": directive, "chunkRef": parent_id } }).to_string()
        } else if let Some(sk) = task_ctx.and_then(|c| c.get("skeleton")) {
            json!({ "skeleton": sk, "stage": stage, "args": { "directive": directive, "chunkRef": parent_id } }).to_string()
        } else {
            json!({ "stage": stage }).to_string()
        };
    } else if let Some(role) = s("prompt_role").or_else(|| s("promptRole")) {
        let hash = role_to_hash.get(role).cloned();
        let vars = ev.get("vars").cloned().unwrap_or_else(|| json!({}));
        let var_refs = ev.get("var_refs").or_else(|| ev.get("varRefs"));
        let mut refs = serde_json::Map::new();
        if let Some(Value::Object(vr)) = var_refs {
            for (k, label) in vr {
                if let Some(label) = label.as_str() {
                    if let Some(h) = role_to_hash.get(label) {
                        refs.insert(k.clone(), json!(h));
                    }
                }
            }
        }
        let mut base = serde_json::Map::new();
        base.insert("promptHash".into(), json!(hash));
        base.insert("vars".into(), vars);
        if !refs.is_empty() {
            base.insert("refs".into(), Value::Object(refs));
        }
        let schema_ref = s("schema_ref").or_else(|| s("schemaRef"));
        let schema_hash = schema_ref.and_then(|l| role_to_hash.get(l).cloned());
        if let Some(sh) = schema_hash {
            base.insert("schemaHash".into(), json!(sh));
        } else if let Some(schema) = ev.get("schema") {
            base.insert("schema".into(), schema.clone());
        }
        body = Value::Object(base).to_string();
    } else if let Some(prompt) = s("prompt") {
        body = if let Some(schema) = ev.get("schema") {
            json!({ "prompt": prompt, "schema": schema }).to_string()
        } else {
            json!({ "prompt": prompt }).to_string()
        };
    } else {
        body = String::new();
    }

    let title = s("title").or(kind).unwrap_or("");
    let mut params = serde_json::Map::new();
    params.insert("title".into(), json!(title));
    params.insert("parentId".into(), json!(parent_id));
    params.insert("body".into(), json!(body));
    params.insert("blockedBy".into(), json!(blocked_by));
    params.insert("locked".into(), json!(true));
    params.insert("type".into(), json!("task"));
    if let Some(k) = kind {
        params.insert("kind".into(), json!(k));
    }
    if let Some(c) = s("category") {
        params.insert("category".into(), json!(c));
    }
    if let Some(d) = s("description") {
        params.insert("description".into(), json!(d));
    }
    if let Some(o) = s("origin") {
        params.insert("origin".into(), json!(o));
    }
    if let Some(b) = s("badge") {
        params.insert("badge".into(), json!(b));
    }
    if ev
        .get("is_draft")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        params.insert("isDraft".into(), json!(true));
    }
    if let Some(pd) = s("parent_draft_id") {
        params.insert("parentDraftId".into(), json!(pd));
    }
    // 라우팅 tier(자기선택) — 저작이 노드에 실은 난이도. node.add 로 흘려 reconcile 이 exec 에 honor.
    // 미emit 이면 미삽입 → 실행자 기본(최고, 품질우선). routing-skill.md.
    if let Some(e) = s("effort").filter(|v| !v.is_empty()) {
        params.insert("effort".into(), json!(e));
    }
    if let Some(m) = s("model").filter(|v| !v.is_empty()) {
        params.insert("model".into(), json!(m));
    }
    Value::Object(params)
}

// ── 정규화 item body 해소 — promptHash → board prompt.resolve ──
fn resolve_body(body: &str, deps: &dyn Deps, extra_vars: &Value) -> String {
    let p: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return body.to_string(),
    };
    let prompt_hash = match p.get("promptHash").and_then(|v| v.as_str()) {
        Some(h) => h.to_string(),
        None => return body.to_string(),
    };
    // vars = { ...p.vars, ...extraVars }(extra 가 override).
    let mut vars = serde_json::Map::new();
    if let Some(pv) = p.get("vars").and_then(|v| v.as_object()) {
        for (k, v) in pv {
            vars.insert(k.clone(), v.clone());
        }
    }
    if let Some(ev) = extra_vars.as_object() {
        for (k, v) in ev {
            vars.insert(k.clone(), v.clone());
        }
    }
    let refs = p.get("refs").cloned().unwrap_or_else(|| json!({}));
    let rp = match deps.resolve_prompt(&prompt_hash, Value::Object(vars), refs) {
        Some(d) => d,
        None => return body.to_string(),
    };
    let prompt = match rp.get("prompt") {
        Some(v) if !v.is_null() => v.clone(),
        _ => return body.to_string(),
    };
    let mut schema = p.get("schema").cloned();
    if let Some(sh) = p.get("schemaHash").and_then(|v| v.as_str()) {
        let sr = match deps.get_prompt(sh) {
            Some(v) => v,
            None => return body.to_string(),
        };
        let value = sr.get("value").cloned().unwrap_or(sr);
        if !value.is_object() {
            return body.to_string();
        }
        schema = Some(value);
    }
    match schema {
        Some(s) => json!({ "prompt": prompt, "schema": s }).to_string(),
        None => json!({ "prompt": prompt }).to_string(),
    }
}

/// with_routing — 노드 라우팅(effort/model)을 exec 입력 JSON 에 주입한다. 저작 LLM 이 노드에 실은
/// 난이도 tier 를 실행자에게 흘려보내는 통로 — 미지정이면 실행자 기본(최고, 품질우선)이라 무주입.
fn with_routing(body: String, node: &Node) -> String {
    if node.effort.is_none() && node.model.is_none() {
        return body;
    }
    let mut v: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => return body,
    };
    if let Some(obj) = v.as_object_mut() {
        if let Some(e) = &node.effort {
            obj.insert("effort".into(), json!(e));
        }
        if let Some(m) = &node.model {
            obj.insert("model".into(), json!(m));
        }
    }
    v.to_string()
}

struct StageInput {
    stage_body: String,
    ledger: Option<Vec<Value>>,
}

// exec-stage args 주입 — ledger/facts를 stage args에 실어 보낸다.
// Err(Value) = 에러 TickResult(materialize 실패, hunt 제외).
fn build_stage_input(
    deps: &dyn Deps,
    target: &Node,
    body: &str,
    stage_name: &str,
) -> Result<StageInput, Value> {
    // 합의 스테이지(draft-review·research-audit·design-audit) — {{document}}=args.ledger(state+history 문서),
    // {{round}}=args.round 한 채널만 주입한다. reviewer 는 이 문서로 [현재 집합 + 변경 히스토리] 를 본다.
    // 옛 원장/facts/removed 주입을 대체(그 프롬프트들은 {{ledger}}/{{facts}} 를 안 쓴다).
    if let Some(spec) = consensus_spec(stage_name) {
        let Some(chunk) = target.parent_id.as_deref() else {
            return Ok(StageInput {
                stage_body: body.to_string(),
                ledger: None,
            });
        };
        let doc = build_consensus_document(&deps.list_nodes(), chunk, &spec);
        let round = read_round(body);
        let mut inp: Value = serde_json::from_str(body).map_err(|e| {
            json!({ "ok": false, "processed": 0, "id": target.id, "code": "INTERNAL", "message": format!("합의 스테이지 body 파싱 실패: {e}") })
        })?;
        if let Some(args) = inp.get_mut("args").and_then(|a| a.as_object_mut()) {
            args.insert("ledger".into(), json!(doc.clone()));
            args.insert("round".into(), json!(round));
        } else {
            inp["args"] = json!({ "ledger": doc.clone(), "round": round });
        }
        return Ok(StageInput {
            stage_body: inp.to_string(),
            ledger: Some(doc),
        });
    }
    let ledger_stages: HashSet<&str> = [
        "hunt",
        "classify",
        "audit",
        "research",
        "plan",
        "design-interface",
        "design-domain",
        "design-criteria",
    ]
    .into_iter()
    .collect();
    let o_only: HashSet<&str> = [
        "plan",
        "design-interface",
        "design-domain",
        "design-criteria",
    ]
    .into_iter()
    .collect();
    let mut stage_body = body.to_string();
    let mut ledger: Option<Vec<Value>> = None;
    if ledger_stages.contains(stage_name) && target.parent_id.is_some() {
        let parent = target.parent_id.clone().unwrap();
        let materialize = || -> Result<String, String> {
            let led = deps.materialize_ledger(&parent)?;
            let mut inp: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
            let filtered = if o_only.contains(stage_name) {
                led.iter()
                    .filter(|e| e.get("badge").and_then(|b| b.as_str()) == Some("o"))
                    .cloned()
                    .collect::<Vec<_>>()
            } else {
                led.clone()
            };
            let args = inp.get_mut("args").and_then(|a| a.as_object_mut());
            if let Some(args) = args {
                args.insert("ledger".into(), json!(filtered));
            } else {
                inp["args"] = json!({ "ledger": filtered });
            }
            if stage_name == "plan" || o_only.contains(stage_name) {
                let facts = deps.materialize_facts(&parent)?;
                let f_filtered = if o_only.contains(stage_name) {
                    facts
                        .iter()
                        .filter(|e| e.get("badge").and_then(|b| b.as_str()) == Some("o"))
                        .cloned()
                        .collect::<Vec<_>>()
                } else {
                    facts.clone()
                };
                inp["args"]["facts"] = json!(f_filtered);
            }
            Ok(inp.to_string())
        };
        // ledger 는 반환에도 실어야 함(classify 검증). materialize 를 한 번 더 부르지 않게 캡처.
        match deps.materialize_ledger(&parent) {
            Ok(led) => {
                ledger = Some(led);
                match materialize() {
                    Ok(sb) => stage_body = sb,
                    Err(e) => {
                        if stage_name != "hunt" {
                            return Err(
                                json!({ "ok": false, "processed": 0, "id": target.id, "code": "INTERNAL", "message": format!("원장 materialize 실패({stage_name}): {e}") }),
                            );
                        }
                    }
                }
            }
            Err(e) => {
                if stage_name != "hunt" {
                    return Err(
                        json!({ "ok": false, "processed": 0, "id": target.id, "code": "INTERNAL", "message": format!("원장 materialize 실패({stage_name}): {e}") }),
                    );
                }
            }
        }
    }
    Ok(StageInput { stage_body, ledger })
}

/// 스테이지 → 섹션 제목 — 프레임을 매달 chunk-밑 섹션. research→Research, design 체인 3스테이지→Design(공유),
/// plan→Plan. Spec 은 apply_draft_doc 소관. 그 밖(generate/audit/body 등)은 섹션 없음.
fn stage_section_title(stage: &str) -> Option<&'static str> {
    match stage {
        "research" => Some("Research"),
        "design-interface" | "design-domain" | "design-criteria" => Some("Design"),
        "plan" => Some("Plan"),
        _ => None,
    }
}

/// 스테이지 섹션 멱등 발행 — chunk 밑 kind=section·동일 title 이 이미 있으면 그 id 재사용(Design 3스테이지가
/// 한 섹션 공유), 없으면 발행. 섹션은 badge 없음(pick_ready 제외) + locked + collapsed(자식 숨김).
/// add_node 실패(None)면 None → 호출부가 프레임을 chunk 직속으로 폴백.
fn ensure_section(deps: &dyn Deps, chunk_id: &str, title: &str) -> Option<String> {
    if let Some(existing) = deps.list_nodes().into_iter().find(|n| {
        n.kind.as_deref() == Some("section")
            && n.parent_id.as_deref() == Some(chunk_id)
            && n.title.as_deref() == Some(title)
    }) {
        return Some(existing.id);
    }
    deps.add_node(json!({
        "title": title,
        "parentId": chunk_id,
        "body": "",
        "blockedBy": [],
        "locked": true,
        "collapsed": true,
        "type": "task",
        "kind": "section",
    }))
}

/// 합의 스테이지 프레임 명세 — 어떤 kind 프레임을, 어느 chunk-밑 섹션에, 어느 scope 로 물질화하나.
/// draft-review→Spec(요건 item), research-audit→Research(기초 fact), design-audit→Design(설계 fact).
struct ConsensusSpec {
    kind: &'static str,
    section: &'static str,
    scope: FrameScope,
    /// 신규(add) 프레임의 최초 badge — 검수 대상(검수전)이냐 태생 확정(o)이냐.
    /// item·research fact 는 검수전(per-item 검증 대상), design fact 는 태생 o(파생 결정, 검증 없음).
    create_badge: &'static str,
}
enum FrameScope {
    Items,
    ResearchFacts,
    DesignFacts,
}

/// 합의 스테이지 판별 — 세 완전성 지점만 changes 프로토콜(apply_changes)을 쓴다. 그 밖은 None.
fn consensus_spec(stage: &str) -> Option<ConsensusSpec> {
    match stage {
        "draft-review" => Some(ConsensusSpec {
            kind: "item",
            section: "Spec",
            scope: FrameScope::Items,
            create_badge: "검수전",
        }),
        "research-audit" => Some(ConsensusSpec {
            kind: "fact",
            section: "Research",
            scope: FrameScope::ResearchFacts,
            create_badge: "검수전",
        }),
        "design-audit" => Some(ConsensusSpec {
            kind: "fact",
            section: "Design",
            scope: FrameScope::DesignFacts,
            create_badge: "o",
        }),
        _ => None,
    }
}

/// scope 필터 — 같은 kind=fact 라도 카테고리로 research(설계 밖)/design(설계 안) 을 가른다.
fn frame_in_scope(scope: &FrameScope, node: &Node) -> bool {
    match scope {
        FrameScope::Items => true,
        FrameScope::ResearchFacts => {
            !DESIGN_FACT_CATS.contains(&node.category.as_deref().unwrap_or(""))
        }
        FrameScope::DesignFacts => {
            DESIGN_FACT_CATS.contains(&node.category.as_deref().unwrap_or(""))
        }
    }
}

/// build_consensus_document — 합의 문서(items with state+history) 구성. reviewer 가 [현재 집합 + 변경
/// 히스토리] 를 보는 단일 채널이며 apply_changes 의 입력이기도 하다. state=badge 매핑(x→"x", 그 밖→"o"),
/// history=node.result JSON 의 history 배열(합의가 누적, 없으면 []). doc 엔진 {{document}}=args.ledger 렌더.
fn build_consensus_document(nodes: &[Node], chunk_id: &str, spec: &ConsensusSpec) -> Vec<Value> {
    let by_id: HashMap<String, &Node> = nodes.iter().map(|n| (n.id.clone(), n)).collect();
    nodes
        .iter()
        .filter(|n| {
            n.kind.as_deref() == Some(spec.kind)
                && descends(&by_id, n, chunk_id)
                && frame_in_scope(&spec.scope, n)
        })
        .map(|n| {
            let state = if n.badge_str() == "x" { "x" } else { "o" };
            let history = n
                .result
                .as_deref()
                .and_then(|r| serde_json::from_str::<Value>(r).ok())
                .and_then(|v| v.get("history").cloned())
                .unwrap_or_else(|| json!([]));
            json!({
                "id": n.id,
                "state": state,
                "title": n.title,
                "description": n.description,
                "category": n.category,
                "history": history,
            })
        })
        .collect()
}

/// read_round — task body 의 args.round(reconcile 소유 카운터). 미지정이면 1(최초 라운드).
fn read_round(body: &str) -> u32 {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.pointer("/args/round").cloned())
        .and_then(|r| match r {
            Value::Number(n) => n.as_u64(),
            Value::String(s) => s.parse().ok(),
            _ => None,
        })
        .map(|n| n as u32)
        .unwrap_or(1)
}

/// inject_round — 자기재발행(-again) task 의 body args.round 에 다음 라운드 번호를 싣는다. doc 엔진은 산술을
/// 못 하므로 round 증분은 reconcile 소유. 이 주입으로 {{round}} 플레이스홀더가 라운드마다 누적한다.
fn inject_round(params: &mut Value, round: u32) {
    let Some(body_str) = params.get("body").and_then(|b| b.as_str()) else {
        return;
    };
    let mut body: Value = match serde_json::from_str(body_str) {
        Ok(v) => v,
        Err(_) => return,
    };
    if let Some(args) = body.get_mut("args").and_then(|a| a.as_object_mut()) {
        args.insert("round".into(), json!(round));
    } else {
        body["args"] = json!({ "round": round });
    }
    if let Some(obj) = params.as_object_mut() {
        obj.insert("body".into(), json!(body.to_string()));
    }
}

/// 형제 프레임의 정규화 verify body 를 복제하되 vars.title/description 를 신규 프레임 값으로 교체한다.
/// 신규(add) 프레임도 형제와 동일한 검증 배선(promptHash+refs+schema)을 얻어 per-item 검증 대상이 된다.
/// 비-정규화(빈·plain) body(예: 태생-o design fact)면 그대로 반환.
fn clone_verify_body(sibling_body: &str, title: &str, description: &str) -> String {
    let mut v: Value = match serde_json::from_str(sibling_body) {
        Ok(v) => v,
        Err(_) => return String::new(),
    };
    if v.get("promptHash").is_none() {
        return sibling_body.to_string();
    }
    if let Some(vars) = v.get_mut("vars").and_then(|x| x.as_object_mut()) {
        vars.insert("title".into(), json!(title));
        vars.insert("description".into(), json!(description));
    }
    v.to_string()
}

/// 합의 add → 신규 프레임 node.add 파라미터. 섹션-밑 locked 프레임 + 형제 body 복제(검증 배선 상속) +
/// history 를 result JSON 으로 실어 다음 라운드가 읽는다. section_id None 이면 chunk 직속 폴백.
fn build_consensus_create(
    create: &Value,
    chunk_id: &str,
    section_id: Option<&str>,
    template: Option<&Node>,
    spec: &ConsensusSpec,
) -> Value {
    let title = create
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let description = create
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let history = create.get("history").cloned().unwrap_or_else(|| json!([]));
    let parent = section_id.unwrap_or(chunk_id);
    let body = template
        .map(|t| clone_verify_body(t.body_str(), &title, &description))
        .unwrap_or_default();
    json!({
        "title": title,
        "parentId": parent,
        "description": description,
        "body": body,
        "blockedBy": [],
        "locked": true,
        "collapsed": false,
        "type": "task",
        "kind": spec.kind,
        "badge": spec.create_badge,
        "origin": "agent",
        "result": json!({ "history": history }).to_string(),
    })
}

// stage 산출 소비 — draftDoc 검증·발행 또는 자식 발행 + classify/audit 처리.
fn consume_stage_output(
    deps: &dyn Deps,
    target: &Node,
    body: &str,
    stage_name: &str,
    ledger: Option<Vec<Value>>,
    staged: StageOut,
) -> Value {
    // childCtx — 자식 task 에 전파할 workflowRef/skeleton+directive.
    let child_ctx: Option<Value> = serde_json::from_str::<Value>(body).ok().and_then(|inp| {
        if let Some(w) = inp.get("workflow").and_then(|v| v.as_str()) {
            Some(json!({ "workflowRef": w, "directive": inp.pointer("/args/directive").cloned().unwrap_or(Value::Null) }))
        } else if inp.get("skeleton").is_some() {
            Some(json!({ "skeleton": inp.get("skeleton"), "directive": inp.pointer("/args/directive").cloned().unwrap_or(Value::Null) }))
        } else {
            None
        }
    });

    match staged {
        StageOut::DraftDoc(draft) => {
            // draft_doc 검증 재사용(골든 태그). build 는 이미 됐고 여기선 재검증.
            let doc: crate::draft_doc::DraftDoc = match serde_json::from_value(draft.clone()) {
                Ok(d) => d,
                Err(e) => {
                    return json!({ "ok": false, "processed": 0, "id": target.id, "code": "VALIDATION_FAILED", "message": format!("DraftDoc 역직렬화 실패: {e}") })
                }
            };
            if let Err(violations) = crate::draft_doc::validate(&doc) {
                return json!({ "ok": false, "processed": 0, "id": target.id, "code": "VALIDATION_FAILED", "message": format!("DraftDoc 검증 실패({}건): {}", violations.len(), violations[0]) });
            }
            let published = match apply_draft_doc(
                deps,
                &draft,
                target.parent_id.as_deref(),
                child_ctx.as_ref(),
            ) {
                Ok(p) => p,
                Err(e) => {
                    return json!({ "ok": false, "processed": 0, "id": target.id, "code": "INTERNAL", "message": e })
                }
            };
            deps.edit_node(&target.id, json!({ "status": "done" }));
            deps.poke();
            json!({ "ok": true, "processed": 1, "id": target.id, "stage": true, "published": published })
        }
        StageOut::Children { children, result } => {
            let mut key_of: HashMap<String, String> = HashMap::new();
            let mut role_to_hash: HashMap<String, String> = HashMap::new();
            // 보드 모델 — 스테이지 프레임(fact/plan-unit)은 chunk 직속이 아니라 chunk 밑 스테이지 섹션 밑에
            // 매단다(Spec 은 apply_draft_doc 소관). 섹션은 프레임 처음 만날 때 멱등 발행(Design 3스테이지가
            // 한 섹션 공유). doc 엔진은 board 상태를 못 읽어 멱등 find-or-create 를 못 하니 이 주입은 reconcile 몫.
            // task/code 자식은 섹션이 아니라 chunk 직속으로 남는다(pick_ready 실행 대상).
            let section_title = stage_section_title(stage_name);
            let mut section_id: Option<String> = None;
            // 합의 라운드 카운터(reconcile 소유) — 이 스테이지가 합의 스테이지면 body args.round 에서 읽는다.
            let is_consensus = consensus_spec(stage_name).is_some();
            let round = read_round(body);
            // 상한 도달로 자기재발행을 봉인했는가 — 봉인 시 chunk 를 badge=f 로 확정하고 루프를 멈춘다.
            let mut sealed = false;
            for ev in &children {
                if let Some(reg) = ev
                    .get("register_prompts")
                    .or_else(|| ev.get("registerPrompts"))
                {
                    for (role, hash) in register_prompt_templates(reg, deps) {
                        role_to_hash.insert(role, hash);
                    }
                }
                // 합의 자기재발행(-again) — 같은 합의 스테이지 task. round+1 을 실어 라운드가 누적한다.
                // 상한 도달이면 재발행하지 않고 봉인(무한 루프 차단) — doc 엔진이 changes 로 발행한 -again 을 억제.
                let is_republish = is_consensus
                    && ev.get("kind").and_then(|v| v.as_str()) == Some("task")
                    && ev.get("stage").and_then(|v| v.as_str()) == Some(stage_name);
                if is_republish {
                    if round >= CONSENSUS_ROUND_MAX {
                        sealed = true;
                        continue;
                    }
                }
                let mut parent_id = ev
                    .get("parent")
                    .and_then(|v| v.as_str())
                    .map(|p| key_of.get(p).cloned().unwrap_or_else(|| p.to_string()));
                // 프레임이면 스테이지 섹션 밑으로 재부모화(id·badge·blockedBy 불변 — 부모만 바뀜). descends()
                // 는 부모 체인으로 chunk 까지 올라가니 섹션-밑 프레임도 여전히 chunk 자손(원장/materialize 유지).
                let is_frame = matches!(
                    ev.get("kind").and_then(|v| v.as_str()),
                    Some("fact") | Some("plan-unit")
                );
                if is_frame {
                    if let (Some(title), Some(chunk)) = (section_title, target.parent_id.as_deref())
                    {
                        if section_id.is_none() {
                            section_id = ensure_section(deps, chunk, title);
                        }
                        if let Some(sid) = &section_id {
                            parent_id = Some(sid.clone());
                        }
                    }
                }
                let blocked_by: Vec<String> = ev
                    .get("blocked_by")
                    .or_else(|| ev.get("blockedBy"))
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|id| id.as_str())
                            .map(|id| key_of.get(id).cloned().unwrap_or_else(|| id.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                let mut params = build_add_params(
                    ev,
                    parent_id.as_deref(),
                    &blocked_by,
                    child_ctx.as_ref(),
                    &role_to_hash,
                );
                if is_republish {
                    inject_round(&mut params, round + 1);
                }
                if let Some(node_id) = deps.add_node(params) {
                    if let Some(ev_id) = ev.get("id").and_then(|v| v.as_str()) {
                        key_of.insert(ev_id.to_string(), node_id);
                    }
                }
            }
            let res = &result;
            let mut assigned = 0;
            if stage_name == "classify" {
                let assignments = res.get("assignments").and_then(|v| v.as_array());
                let Some(assignments) = assignments else {
                    return json!({ "ok": false, "processed": 0, "id": target.id, "code": "INVALID_RESULT", "message": "classify 결과에 assignments 배열 없음" });
                };
                let Some(ledger) = ledger.as_ref() else {
                    return json!({ "ok": false, "processed": 0, "id": target.id, "code": "INTERNAL", "message": "classify 원장 materialize 실패 — 배정 검증 불가" });
                };
                let ledger_ids: HashSet<String> = ledger
                    .iter()
                    .filter_map(|e| e.get("id").and_then(|v| v.as_str()).map(String::from))
                    .collect();
                let mut seen: HashSet<String> = HashSet::new();
                let mut errs: Vec<String> = Vec::new();
                for a in assignments {
                    let id = a.get("id").and_then(|v| v.as_str());
                    let cat = a.get("category").and_then(|v| v.as_str());
                    match (id, cat) {
                        (Some(id), Some(cat)) if !cat.is_empty() => {
                            if !ledger_ids.contains(id) {
                                errs.push(format!("원장 밖 id: {id}"));
                            } else if seen.contains(id) {
                                errs.push(format!("중복 배정: {id}"));
                            }
                            seen.insert(id.to_string());
                        }
                        _ => errs.push(format!("형식 위반: {a}")),
                    }
                }
                for id in &ledger_ids {
                    if !seen.contains(id) {
                        errs.push(format!("미배정: {id}"));
                    }
                }
                if !errs.is_empty() {
                    return json!({ "ok": false, "processed": 0, "id": target.id, "code": "VALIDATION_FAILED", "message": format!("classify 배정 검증 실패({}건): {}", errs.len(), errs[0]) });
                }
                for a in assignments {
                    let id = a.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    let cat = a.get("category").cloned().unwrap_or(Value::Null);
                    let er = deps.edit_node(id, json!({ "category": cat }));
                    if !er.ok {
                        return json!({ "ok": false, "processed": 0, "id": target.id, "code": "INTERNAL", "message": format!("category 기록 실패({id}): {}", er.message.unwrap_or_default()) });
                    }
                    assigned += 1;
                }
            }
            if stage_name == "audit" && !res.is_object() {
                return json!({ "ok": false, "processed": 0, "id": target.id, "code": "INVALID_RESULT", "message": "audit 결과 없음(verdict/complete 미반환)" });
            }
            // 합의 changes 물질화 — reviewer changes[{op,id?,title?,description?,reason}] 를 현재 프레임에 적용한다.
            // add→검수전(또는 태생-o) 프레임 신규(올바른 섹션 밑), remove:o→x, reraise:x→o + history 누적(result JSON).
            // 옛 apply_review(additions/removals)는 changes 없는 audit/classify/generate 반환값 전용(아래 else).
            if let (Some(changes), Some(spec), Some(chunk)) = (
                res.get("changes").and_then(|c| c.as_array()),
                consensus_spec(stage_name),
                target.parent_id.as_deref(),
            ) {
                let all = deps.list_nodes();
                let doc = build_consensus_document(&all, chunk, &spec);
                let cs = crate::consensus::apply_changes(&doc, changes, round);
                if !cs.creates.is_empty() {
                    let section_id = ensure_section(deps, chunk, spec.section);
                    let by_id: HashMap<String, &Node> =
                        all.iter().map(|n| (n.id.clone(), n)).collect();
                    // 형제 프레임 하나 — 신규 add 프레임이 복제할 검증 배선(정규화 body) 원본.
                    let template = all.iter().find(|n| {
                        n.kind.as_deref() == Some(spec.kind)
                            && descends(&by_id, n, chunk)
                            && frame_in_scope(&spec.scope, n)
                    });
                    for create in &cs.creates {
                        let params = build_consensus_create(
                            create,
                            chunk,
                            section_id.as_deref(),
                            template,
                            &spec,
                        );
                        deps.add_node(params);
                    }
                }
                for e in &cs.edits {
                    // remove:o→x, reraise:x→o. history 는 result JSON 으로 누적 — build_consensus_document 가
                    // 읽어 다음 라운드 document 로 렌더(진동 차단). reason = 이 라운드 변경 사유(history 마지막).
                    let badge = if e.state == "x" { "x" } else { "o" };
                    let last_reason = e
                        .history
                        .last()
                        .and_then(|h| h.get("reason").cloned())
                        .unwrap_or(Value::Null);
                    let result = json!({ "reason": last_reason, "history": e.history }).to_string();
                    deps.edit_node(&e.id, json!({ "badge": badge, "result": result }));
                }
            } else if res.is_object() {
                // 합의 루프의 remove 연산 — 어느 audit(draft·research·design·plan)든 result.removals[{id,reason}]
                // 로 현재 항목을 badge→x(반박·중복·범위밖 자기교정). 이 한 경로로 네 완전성 지점이 같은 remove 를 재사용.
                // 삭제 아님 — x 항목은 사유와 함께 ledger 에 남아 다음 라운드 reviewer 가 "이미 뺀 것"을 본다(보드=히스토리→진동 차단).
                let review = crate::consensus::apply_review(res, 1);
                for (id, reason) in &review.badge_edits {
                    deps.edit_node(id, json!({ "badge": "x", "result": reason }));
                }
                if let Some(parent_id) = &target.parent_id {
                    let mut chunk_edit = serde_json::Map::new();
                    if let Some(t) = res.get("chunkTitle").and_then(|v| v.as_str()) {
                        if !t.is_empty() {
                            chunk_edit.insert("title".into(), json!(t));
                        }
                    }
                    if let Some(v) = res.get("verdict").and_then(|v| v.as_str()) {
                        if !v.is_empty() {
                            chunk_edit.insert("result".into(), json!(v));
                        }
                    }
                    if stage_name == "classify" {
                        if let Some(d) = res.get("dimension").and_then(|v| v.as_str()) {
                            if !d.is_empty() {
                                chunk_edit.insert("result".into(), json!(d));
                            }
                        }
                    }
                    if stage_name == "audit" {
                        let f_count = ledger
                            .as_ref()
                            .map(|l| {
                                l.iter()
                                    .filter(|e| {
                                        e.get("badge").and_then(|b| b.as_str()) == Some("f")
                                    })
                                    .count() as i64
                            })
                            .unwrap_or(-1);
                        let complete = res.get("complete").and_then(|v| v.as_bool()) == Some(true);
                        chunk_edit.insert(
                            "badge".into(),
                            json!(if complete && f_count == 0 { "o" } else { "f" }),
                        );
                    }
                    if !chunk_edit.is_empty() {
                        deps.edit_node(parent_id, Value::Object(chunk_edit));
                    }
                }
            }
            // 봉인 — round 상한 도달로 -again 을 억제했으면 chunk 를 badge=f 로 확정(합의 미수렴 종결).
            // 게이트(research_gate 등)는 badge=o 를 요구하므로 여기서 멈추고 사람이 개입한다.
            if sealed {
                if let Some(chunk) = &target.parent_id {
                    deps.edit_node(
                        chunk,
                        json!({ "badge": "f", "result": format!("합의 미수렴 — round 상한 {CONSENSUS_ROUND_MAX} 도달, 봉인") }),
                    );
                }
            }
            deps.edit_node(&target.id, json!({ "status": "done" }));
            deps.poke();
            json!({ "ok": true, "processed": 1, "id": target.id, "stage": true, "published": children.len(), "assigned": assigned })
        }
    }
}

// stage 작업 실행 — 멱등 마커 → buildStageInput → execStage → consume.
fn reconcile_stage(deps: &dyn Deps, target: &Node, body: &str, nodes: &[Node]) -> Value {
    let stage_name = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get("stage").and_then(|s| s.as_str()).map(String::from))
        .unwrap_or_default();
    if stage_published_marker(target, body, &stage_name, nodes) {
        deps.edit_node(&target.id, json!({ "status": "done" }));
        deps.poke();
        return json!({ "ok": true, "processed": 0, "id": target.id, "stage": true, "published": 0, "idempotent": true });
    }
    let built = match build_stage_input(deps, target, body, &stage_name) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let stage_body = with_routing(built.stage_body, target);
    let staged = match deps.exec_stage(&stage_body) {
        Ok(s) => s,
        Err(e) => {
            return json!({ "ok": false, "processed": 0, "id": target.id, "code": "INTERNAL", "message": e })
        }
    };
    consume_stage_output(deps, target, body, &stage_name, built.ledger, staged)
}

/// reconcile 한 틱 — ready 1개 처리. task→exec-stage, 항목→exec-one(검증→배지).
/// 진척(배지 확정) 시 poke. exec 실패는 ok:false(노드 미변경=멱등).
pub fn reconcile_tick(deps: &dyn Deps, state: &mut ReconcileState, now_ms: u64) -> Value {
    let nodes = deps.list_nodes();
    let ready: Vec<Node> = pick_ready(&nodes)
        .into_iter()
        .filter(|n| !lease_active(state, &n.id, now_ms))
        .collect();
    if ready.is_empty() {
        return json!({ "ok": true, "processed": 0 });
    }
    // 기아 방지: 연속 실패 최소 ready 선택.
    let mut target = ready[0].clone();
    if !state.fails.is_empty() {
        let mut best = u32::MAX;
        for n in &ready {
            let f = state.fails.get(&n.id).copied().unwrap_or(0);
            if f < best {
                best = f;
                target = n.clone();
            }
        }
    }
    let node = deps.get_node(&target.id).unwrap_or_default();
    let body = node.body_str().to_string();
    if target.kind.as_deref() == Some("task") {
        let res = reconcile_stage(deps, &target, &body, &nodes);
        if res.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
            state.fails.remove(&target.id);
        } else {
            *state.fails.entry(target.id.clone()).or_insert(0) += 1;
        }
        return res;
    }
    let title = node.title.clone().unwrap_or_else(|| target.id.clone());
    deps.progress("reconcile", &title.chars().take(120).collect::<String>());
    let mut field_vars = serde_json::Map::new();
    if let Some(t) = &node.title {
        field_vars.insert("title".into(), json!(t));
    }
    if let Some(d) = &node.description {
        field_vars.insert("description".into(), json!(d));
    }
    if let Some(c) = &node.category {
        field_vars.insert("category".into(), json!(c));
    }
    let exec_body = with_routing(resolve_body(&body, deps, &Value::Object(field_vars)), &node);
    let exec_out = match deps.exec_one(&exec_body) {
        Ok(o) => o,
        Err(e) => {
            *state.fails.entry(target.id.clone()).or_insert(0) += 1;
            return json!({ "ok": false, "processed": 0, "id": target.id, "code": "INTERNAL", "message": e });
        }
    };
    state.fails.remove(&target.id);
    let mut edit = exec_result_to_edit(&exec_out);
    let has_badge = edit.get("badge").is_some();
    if !has_badge {
        let n = state.no_verdict.get(&target.id).copied().unwrap_or(0) + 1;
        if n >= NO_VERDICT_MAX {
            let last = edit
                .get("result")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let obj = edit.as_object_mut().unwrap();
            obj.insert("badge".into(), json!("f"));
            obj.insert("result".into(), json!(format!("무판정 {n}회 소진(출력에 oxf 없음 — 스키마 부재/모델 이탈) → 자동 f. 마지막 출력: {last}")));
            state.no_verdict.remove(&target.id);
        } else {
            state.no_verdict.insert(target.id.clone(), n);
        }
    } else {
        state.no_verdict.remove(&target.id);
    }
    deps.edit_node(&target.id, edit.clone());
    let final_badge = edit.get("badge").and_then(|v| v.as_str()).map(String::from);
    if let Some(badge) = &final_badge {
        // 보드 모델 — 프레임 badge 확정 시 부모 chunk 진척을 롤업(변화무쌍한 진행중). done chunk(Step 3
        // 이슈라이즈 게이트가 status=done 설정)는 안 건드린다. badge 축(audit 인증)은 안 건드린다 — status +
        // description 만. 같은 값 재-edit 은 무해(멱등). 방금 확정한 프레임은 스냅샷 override 로 계수 반영.
        let by_id: HashMap<String, &Node> = nodes.iter().map(|n| (n.id.clone(), n)).collect();
        if let Some(chunk) = ancestor_chunk(&by_id, &target) {
            if chunk.status.as_deref() != Some("done") {
                if let Some(line) = chunk_progress_line(&nodes, &chunk.id, &target.id, badge) {
                    deps.edit_node(
                        &chunk.id,
                        json!({ "status": "inprogress", "description": line }),
                    );
                }
            }
        }
        deps.poke();
    }
    json!({ "ok": true, "processed": 1, "id": target.id, "badge": final_badge })
}

/// pull v2: 다음 실행 노드 조립 — 검증 노드 우선, 없으면 stage task(assemble).
/// lease 를 잡아 스케줄러 spawn 과의 경합을 막는다. chunk=스코프(팬아웃).
pub fn next_tick(
    deps: &dyn Deps,
    state: &mut ReconcileState,
    chunk: Option<&str>,
    now_ms: u64,
) -> Value {
    let nodes = deps.list_nodes();
    let by_id: HashMap<String, &Node> = nodes.iter().map(|n| (n.id.clone(), n)).collect();
    let in_scope = |n: &Node| -> bool {
        let Some(scope) = chunk else { return true };
        if n.id == scope {
            return false; // 스코프 자신은 실행 대상 아님(자손만).
        }
        let mut p = n.parent_id.clone();
        let mut guard = 0;
        while let Some(pid) = p {
            if guard >= 100 {
                break;
            }
            guard += 1;
            if pid == scope {
                return true;
            }
            p = by_id.get(&pid).and_then(|m| m.parent_id.clone());
        }
        false
    };
    let ready_all: Vec<Node> = pick_ready(&nodes)
        .into_iter()
        .filter(|n| !lease_active(state, &n.id, now_ms) && in_scope(n))
        .collect();
    let ready: Vec<Node> = ready_all
        .iter()
        .filter(|n| n.kind.as_deref() != Some("task") && n.badge.as_deref() == Some("검수전"))
        .cloned()
        .collect();

    // 검증 노드 없고 assemble 배선됐으면 stage task pull.
    if ready.is_empty() && deps.has_assemble_stage() {
        for t in ready_all
            .iter()
            .filter(|n| n.kind.as_deref() == Some("task") && n.status.as_deref() != Some("done"))
        {
            let tn = match deps.get_node(&t.id) {
                Some(n) => n,
                None => continue,
            };
            let stage_name = match serde_json::from_str::<Value>(tn.body_str())
                .ok()
                .and_then(|v| v.get("stage").and_then(|s| s.as_str()).map(String::from))
            {
                Some(s) if !s.is_empty() => s,
                _ => continue,
            };
            // 병합 타겟 — 전체 노드의 parentId/blockedBy.
            let target = tn.clone();
            if stage_published_marker(&target, tn.body_str(), &stage_name, &nodes) {
                deps.edit_node(&t.id, json!({ "status": "done" }));
                deps.poke();
                continue;
            }
            let built = match build_stage_input(deps, &target, tn.body_str(), &stage_name) {
                Ok(b) => b,
                Err(e) => {
                    return json!({ "ok": false, "code": e.get("code").cloned().unwrap_or(json!("INTERNAL")), "message": e.get("message").cloned().unwrap_or(Value::Null) })
                }
            };
            let asm = match deps.assemble_stage(&built.stage_body) {
                Ok(a) => a,
                Err(e) => {
                    return json!({ "ok": false, "code": "INTERNAL", "message": format!("stage 패키지 조립 실패({}): {e}", t.id) })
                }
            };
            let pkg = asm.get("assembled");
            let prompt = pkg.and_then(|p| p.get("prompt")).filter(|v| !v.is_null());
            if prompt.is_none() {
                return json!({ "ok": false, "code": "INTERNAL", "message": format!("stage 패키지에 prompt 없음({})", t.id) });
            }
            state.leases.insert(t.id.clone(), now_ms + NEXT_LEASE_MS);
            state.stage_ctx.insert(
                t.id.clone(),
                StageCtx {
                    stage_body: built.stage_body.clone(),
                    stage_name: stage_name.clone(),
                    ledger: built.ledger.clone(),
                    body: tn.body_str().to_string(),
                },
            );
            return json!({
                "ok": true,
                "node": { "id": t.id, "kind": "task", "stage": stage_name, "title": tn.title.clone().unwrap_or_default() },
                "prompt": prompt,
                "schema": pkg.and_then(|p| p.get("schema")),
                "leaseMs": NEXT_LEASE_MS,
            });
        }
        return json!({ "ok": true, "node": null, "message": "실행할 준비된 노드가 없습니다" });
    }
    if ready.is_empty() {
        return json!({ "ok": true, "node": null, "message": "실행할 준비된 검증 노드가 없습니다" });
    }
    let target = &ready[0];
    let node = deps.get_node(&target.id).unwrap_or_default();
    let mut field_vars = serde_json::Map::new();
    if let Some(t) = &node.title {
        field_vars.insert("title".into(), json!(t));
    }
    if let Some(d) = &node.description {
        field_vars.insert("description".into(), json!(d));
    }
    if let Some(c) = &node.category {
        field_vars.insert("category".into(), json!(c));
    }
    let resolved = resolve_body(node.body_str(), deps, &Value::Object(field_vars));
    let pkg: Value = match serde_json::from_str(&resolved) {
        Ok(v) => v,
        Err(_) => {
            return json!({ "ok": false, "code": "INTERNAL", "message": format!("실행 패키지 조립 실패({}) — 프롬프트 미해석", target.id) })
        }
    };
    let prompt = pkg.get("prompt").filter(|v| !v.is_null());
    if prompt.is_none() {
        return json!({ "ok": false, "code": "INTERNAL", "message": format!("실행 패키지에 prompt 없음({})", target.id) });
    }
    state
        .leases
        .insert(target.id.clone(), now_ms + NEXT_LEASE_MS);
    json!({
        "ok": true,
        "node": { "id": target.id, "kind": target.kind, "title": node.title.clone().unwrap_or_default() },
        "prompt": prompt,
        "schema": pkg.get("schema"),
        "leaseMs": NEXT_LEASE_MS,
    })
}

/// pull v2: CLI 실행자 산출 제출 — spawn과 동일한 파이프. 멱등·무판정 거부.
pub fn submit_tick(
    deps: &dyn Deps,
    state: &mut ReconcileState,
    node_id: &str,
    output: &Value,
) -> Value {
    if node_id.is_empty() {
        return json!({ "ok": false, "code": "INVALID_INPUT", "message": "node(노드 id) 필수" });
    }
    let node = match deps.get_node(node_id) {
        Some(n) => n,
        None => {
            return json!({ "ok": false, "code": "NOT_FOUND", "message": format!("노드 미존재: {node_id}") })
        }
    };
    if node.kind.as_deref() == Some("task") {
        if node.status.as_deref() == Some("done") {
            return json!({ "ok": false, "code": "ALREADY_DONE", "message": "이미 완료된 stage — 멱등 거부" });
        }
        if !deps.has_exec_stage_with_output() {
            return json!({ "ok": false, "code": "INTERNAL", "message": "execStageWithOutput 미배선" });
        }
        if !output.is_object() {
            return json!({ "ok": false, "code": "INVALID_INPUT", "message": "stage 산출(output JSON) 필수" });
        }
        let ctx = match state.stage_ctx.get(node_id).cloned() {
            Some(c) => c,
            None => {
                let stage_name = match serde_json::from_str::<Value>(node.body_str())
                    .ok()
                    .and_then(|v| v.get("stage").and_then(|s| s.as_str()).map(String::from))
                {
                    Some(s) if !s.is_empty() => s,
                    _ => {
                        return json!({ "ok": false, "code": "INVALID_INPUT", "message": "stage task 아님(body 에 stage 없음)" })
                    }
                };
                match build_stage_input(deps, &node, node.body_str(), &stage_name) {
                    Ok(b) => StageCtx {
                        stage_body: b.stage_body,
                        stage_name,
                        ledger: b.ledger,
                        body: node.body_str().to_string(),
                    },
                    Err(e) => return e,
                }
            }
        };
        let staged = match deps.exec_stage_with_output(&ctx.stage_body, output.clone()) {
            Ok(s) => s,
            Err(e) => {
                return json!({ "ok": false, "code": "INTERNAL", "message": format!("stage 산출 재생 실패: {e}") })
            }
        };
        let consumed =
            consume_stage_output(deps, &node, &ctx.body, &ctx.stage_name, ctx.ledger, staged);
        if consumed
            .get("ok")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            state.leases.remove(node_id);
            state.stage_ctx.remove(node_id);
        }
        return consumed;
    }
    let badge = node.badge_str();
    if badge == "o" || badge == "x" || badge == "f" {
        return json!({ "ok": false, "code": "ALREADY_DONE", "message": format!("이미 확정된 노드(badge={badge}) — 멱등 거부") });
    }
    let oxf = match crate::exec_one::extract_oxf(output) {
        Some(o) => o,
        None => {
            return json!({ "ok": false, "code": "INVALID_INPUT", "message": "산출에 oxf(o/x/f) 판정 없음 — 무판정 제출 거부" })
        }
    };
    let result = match output {
        Value::String(s) => s.clone(),
        v => v.to_string(),
    };
    deps.edit_node(node_id, json!({ "badge": oxf, "result": result }));
    state.leases.remove(node_id);
    deps.poke();
    json!({ "ok": true, "node": node_id, "badge": oxf })
}

/// 확정 code 노드 실파일화 — o 확정 code만, PROOF 블록 제외. 게이트: code≥1·전부 확정.
pub fn export_tick(deps: &dyn Deps, chunk_id: &str, dir: &str) -> Value {
    let nodes = deps.list_nodes();
    let by_id: HashMap<String, &Node> = nodes.iter().map(|n| (n.id.clone(), n)).collect();
    let codes: Vec<&Node> = nodes
        .iter()
        .filter(|n| n.kind.as_deref() == Some("code") && descends(&by_id, n, chunk_id))
        .collect();
    if codes.is_empty() {
        return json!({ "ok": false, "code": "GATE_REQUIRED", "message": "확정할 code 노드 없음 — issuerize→실코드화 후에 export" });
    }
    let pending = codes
        .iter()
        .filter(|n| !matches!(n.badge_str(), "o" | "x" | "f"))
        .count();
    if pending > 0 {
        return json!({ "ok": false, "code": "GATE_REQUIRED", "message": format!("미확정 code {pending}건(검수전) — 검증 완료 후 export") });
    }
    let mut files: Vec<String> = Vec::new();
    for c in codes.iter().filter(|n| n.badge_str() == "o") {
        let rel = c.title.clone().unwrap_or_default().trim().to_string();
        if rel.is_empty() || rel.starts_with('/') || rel.split('/').any(|seg| seg == "..") {
            return json!({ "ok": false, "code": "INVALID_INPUT", "message": format!("부적합 파일 경로({}) — 상대경로만, '..' 금지", json!(rel)) });
        }
        let node = deps.get_node(&c.id).unwrap_or_default();
        let desc = node
            .description
            .clone()
            .or_else(|| c.description.clone())
            .unwrap_or_default();
        let content = format!(
            "{}\n",
            desc.split("---- PROOF ----")
                .next()
                .unwrap_or("")
                .trim_end()
        );
        deps.write_file(&rel, &content);
        files.push(rel);
    }
    json!({ "ok": true, "files": files, "dir": dir })
}

/// 이슈라이즈 — 인증 덩어리의 plan-unit(o)을 파일별 실코드화 body task로 승격.
/// 게이트: 덩어리 o·fact≥1 전부 확정·plan-unit≥1 전부 확정. 멱등: 커버된 파일 제외.
pub fn issuerize_tick(deps: &dyn Deps, chunk_id: &str) -> Value {
    let nodes = deps.list_nodes();
    let by_id: HashMap<String, &Node> = nodes.iter().map(|n| (n.id.clone(), n)).collect();
    let chunk = match by_id.get(chunk_id) {
        Some(c) => *c,
        None => {
            return json!({ "ok": false, "code": "NOT_FOUND", "message": format!("덩어리 미존재: {chunk_id}") })
        }
    };
    if chunk.badge_str() != "o" {
        return json!({ "ok": false, "code": "GATE_REQUIRED", "message": format!("덩어리 미인증(badge={}) — audit 인증(badge='o') 후에만 이슈라이즈", json!(chunk.badge)) });
    }
    let confirmed = |n: &Node| matches!(n.badge_str(), "o" | "x" | "f");
    let body_stage = |n: &Node| {
        serde_json::from_str::<Value>(n.body_str())
            .ok()
            .and_then(|v| v.get("stage").and_then(|s| s.as_str()).map(String::from))
    };
    let body_tasks: Vec<&Node> = nodes
        .iter()
        .filter(|n| {
            n.kind.as_deref() == Some("task")
                && descends(&by_id, n, chunk_id)
                && body_stage(n).as_deref() == Some("body")
        })
        .collect();
    let codes: Vec<&Node> = nodes
        .iter()
        .filter(|n| n.kind.as_deref() == Some("code") && descends(&by_id, n, chunk_id))
        .collect();
    let covered_file = |file: &str| -> bool {
        codes
            .iter()
            .any(|c| c.category.as_deref() == Some(file) && c.badge_str() == "o")
            || body_tasks.iter().any(|t| {
                serde_json::from_str::<Value>(t.body_str())
                    .ok()
                    .and_then(|v| {
                        v.get("args")
                            .and_then(|a| a.get("file_path"))
                            .and_then(|f| f.as_str())
                            .map(String::from)
                    })
                    .as_deref()
                    == Some(file)
                    && t.status.as_deref() != Some("done")
            })
            || codes
                .iter()
                .any(|c| c.category.as_deref() == Some(file) && !confirmed(c))
    };
    let facts: Vec<&Node> = nodes
        .iter()
        .filter(|n| n.kind.as_deref() == Some("fact") && descends(&by_id, n, chunk_id))
        .collect();
    if facts.is_empty() {
        return json!({ "ok": false, "code": "GATE_REQUIRED", "message": "research 미경유(기초지식 fact 없음) — research 워크플로 후에만 이슈라이즈" });
    }
    let unverified_facts = facts.iter().filter(|n| !confirmed(n)).count();
    if unverified_facts > 0 {
        return json!({ "ok": false, "code": "GATE_REQUIRED", "message": format!("기초지식 미검증 {unverified_facts}건(검수전) — 검증 완료 후 이슈라이즈") });
    }
    let units: Vec<&Node> = nodes
        .iter()
        .filter(|n| n.kind.as_deref() == Some("plan-unit") && descends(&by_id, n, chunk_id))
        .collect();
    if units.is_empty() {
        return json!({ "ok": false, "code": "GATE_REQUIRED", "message": "plan 미경유(plan-unit 없음) — plan(한턴 슈도코드화) 후에만 이슈라이즈" });
    }
    let unverified_units = units.iter().filter(|n| !confirmed(n)).count();
    if unverified_units > 0 {
        return json!({ "ok": false, "code": "GATE_REQUIRED", "message": format!("유닛 미검증 {unverified_units}건(검수전) — plan 검증 완료 후 이슈라이즈") });
    }
    // 게이트 통과 = 플랜 완결(fact·plan-unit 전부 확정). 완성 스펙은 에픽 헤더가 되니 Draft chunk 를 done 으로.
    // badge 축(검증) 아닌 status 축(완료) — chunk badge='o' 는 그대로. 이미 done 이면 재-edit 금지(멱등).
    if chunk.status.as_deref() != Some("done") {
        deps.edit_node(chunk_id, json!({ "status": "done" }));
    }
    let directive = chunk.description.clone().unwrap_or_default();
    let pending: Vec<&Node> = units
        .iter()
        .filter(|n| n.badge_str() == "o" && !covered_file(n.category.as_deref().unwrap_or("")))
        .copied()
        .collect();
    if pending.is_empty() {
        return json!({ "ok": false, "code": "ALREADY_DONE", "message": "이미 이슈라이즈된 덩어리(전 유닛 실코드화 진행/완료) — 멱등 거부" });
    }
    let mut issued = 0;
    for u in &pending {
        let file = u.category.clone().unwrap_or_default();
        let rejected: Vec<&&Node> = codes
            .iter()
            .filter(|c| {
                c.category.as_deref() == Some(file.as_str()) && matches!(c.badge_str(), "f" | "x")
            })
            .collect();
        let rework: Vec<String> = rejected
            .iter()
            .filter_map(|c| {
                serde_json::from_str::<Value>(c.result.as_deref().unwrap_or("{}"))
                    .ok()
                    .and_then(|v| v.get("reason").and_then(|r| r.as_str()).map(String::from))
            })
            .filter(|s| !s.is_empty())
            .collect();
        let pseudo = if rework.is_empty() {
            u.description.clone().unwrap_or_default()
        } else {
            format!("{}\n\nPRIOR ATTEMPT REJECTED — the verifier's findings, every one of which THIS attempt must fix:\n- {}", u.description.clone().unwrap_or_default(), rework.join("\n- "))
        };
        let body = json!({
            "workflow": "research",
            "stage": "body",
            "args": { "title": u.title, "file_path": file, "pseudocode": pseudo, "chunkRef": chunk_id, "directive": directive },
        })
        .to_string();
        // 팬아웃 작업 task 는 스펙 프레임(locked, 분리불가)과 대비되는 분리·성장 가능한 정상 노드 — unlocked.
        // 부모 Draft chunk 도 unlocked 라 isLockedTree 주입 가드 통과. status 는 미설정(기본 todo, 개별 생애주기).
        let params = json!({
            "title": format!("실코드화: {}", if file.is_empty() { u.title.clone().unwrap_or_default() } else { file.clone() }),
            "parentId": chunk_id,
            "body": body,
            "blockedBy": [],
            "locked": false,
            "type": "task",
            "kind": "task",
        });
        if deps.add_node(params).is_none() {
            return json!({ "ok": false, "code": "INTERNAL", "message": format!("body task 발행 실패({}) — 부분 승격 중단(발행 {issued}건)", u.id), "issued": issued });
        }
        issued += 1;
    }
    json!({ "ok": true, "issued": issued, "chunk": chunk_id })
}

/// research 진입 게이트 — 덩어리 o·description 비어있지 않음·멱등(fact/research task 부재).
pub fn research_gate(deps: &dyn Deps, chunk_id: &str) -> Value {
    let nodes = deps.list_nodes();
    let by_id: HashMap<String, &Node> = nodes.iter().map(|n| (n.id.clone(), n)).collect();
    let chunk = match nodes.iter().find(|n| n.id == chunk_id) {
        Some(c) => c,
        None => {
            return json!({ "ok": false, "code": "NOT_FOUND", "message": format!("덩어리 미존재: {chunk_id}") })
        }
    };
    if chunk.badge_str() != "o" {
        return json!({ "ok": false, "code": "GATE_REQUIRED", "message": format!("덩어리 미인증(badge={}) — audit 인증(badge='o') 후에만 research", json!(chunk.badge)) });
    }
    if chunk
        .description
        .as_deref()
        .map(|d| d.trim().is_empty())
        .unwrap_or(true)
    {
        return json!({ "ok": false, "code": "INVALID_INPUT", "message": "덩어리 description(정련 directive) 비어있음 — 검증 기준 없이 research 불가" });
    }
    if nodes
        .iter()
        .any(|n| n.kind.as_deref() == Some("fact") && descends(&by_id, n, chunk_id))
    {
        return json!({ "ok": false, "code": "ALREADY_DONE", "message": "이미 research 진행/완료(fact 존재) — 멱등 거부" });
    }
    for t in nodes
        .iter()
        .filter(|n| n.kind.as_deref() == Some("task") && n.parent_id.as_deref() == Some(chunk_id))
    {
        let full = deps.get_node(&t.id).unwrap_or_default();
        if let Ok(b) = serde_json::from_str::<Value>(full.body_str()) {
            if b.get("workflow").and_then(|w| w.as_str()) == Some("research") {
                return json!({ "ok": false, "code": "ALREADY_DONE", "message": "이미 research task 발행됨 — 멱등 거부" });
            }
        }
    }
    json!({ "ok": true, "directive": chunk.description })
}

// applyDraftDoc·registerPromptTemplates 는 chunk 5(draft 발행). 여기선 시그니처만 전방 선언용 — chunk 5 에서 구현.
fn apply_draft_doc(
    deps: &dyn Deps,
    doc: &Value,
    chunk_id: Option<&str>,
    task_ctx: Option<&Value>,
) -> Result<usize, String> {
    crate::reconcile::draft::apply_draft_doc(deps, doc, chunk_id, task_ctx)
}
fn register_prompt_templates(register_prompts: &Value, deps: &dyn Deps) -> Vec<(String, String)> {
    crate::reconcile::draft::register_prompt_templates(register_prompts, deps)
}

pub mod draft;

#[cfg(test)]
#[path = "reconcile_tests.rs"]
mod tests;
