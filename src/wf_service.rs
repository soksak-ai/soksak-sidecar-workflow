//! workflow 상주 서비스 핸들러. 보드/스케줄러 호출은 Emit::call로 중개하고,
//! exec는 provider/doc_exec를 프로세스 안에서 사용한다. 상태 변경은 서비스 하니스의 단일
//! 뮤텍스로 직렬화한다.
//!
//! 보드는 계약으로 발견한다(consumes) — 구현체 이름은 이 파일 어디에도 없다.

use serde_json::{json, Map, Value};
use soksak_spec_service::{serve_stdio, Emit, ErrCode, OpCtx, Outcome, ServiceHandler};
use std::collections::HashMap;
use std::sync::Mutex;

use crate::doc_exec;
use crate::draft_doc;
use crate::emit_host::NodeEvent;
use crate::exec_one;
use crate::host::build_prompt_with_schema;
use crate::lang::Language;
use crate::provider::{run_agent, run_agent_text, AgentRequest};
use crate::reconcile::draft::register_prompt_templates;
use crate::reconcile::{
    build_add_params, build_ledger, export_tick, issuerize_tick, next_tick, reconcile_tick,
    research_gate, resolve_directive, submit_tick, Deps, EditResult, Node, ReconcileState,
    StageOut,
};

const DEFAULT_MODEL: &str = "opus";

// 소비 계약만 선언하며 구현체 id는 런타임에 발견한다.
const BOARD_CONTRACT: &str = "soksak-spec-plugin-issue-board@0.0.1";
const PROMPT_CONTRACT: &str = "soksak-spec-plugin-prompt-store@0.0.1";
/// 보드 변경 신호 — 토픽 이름은 보드 계약이 정한다(서비스는 버스 축에서 `bus:` 접두로 구독).
const BOARD_CHANGED: &str = "issue-board:changed";

/// 두 계약을 **모두** 구현한 활성 플러그인. 노드는 자기가 실행할 프롬프트의 주소를 지니고, 한 저장소가
/// 발급한 주소는 다른 저장소에서 아무 뜻이 없다 — 그래서 선택은 교집합이지 "먼저 답한 보드" 가 아니다.
/// 순수: 두 발견 결과만 주면 앱 없이 판정된다.
pub fn pick_implementer(boards: &Value, stores: &Value) -> Option<String> {
    let enabled = |v: &Value| -> Vec<String> {
        v.get("implementers")
            .and_then(|i| i.as_array())
            .map(|xs| {
                xs.iter()
                    .filter(|i| i.get("status").and_then(|s| s.as_str()) == Some("enabled"))
                    .filter_map(|i| i.get("id").and_then(|s| s.as_str()).map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };
    let holds_prompts: std::collections::HashSet<String> = enabled(stores).into_iter().collect();
    enabled(boards)
        .into_iter()
        .find(|id| holds_prompts.contains(id))
}

/// 구현체 해소 — op 마다 다시 발견한다. 구현체가 꺼지거나 갈아끼워져도 다음 op 는 새 사실을 보므로
/// 캐시 무효화 추측이 필요 없다(비용 = op 당 발견 왕복 2회, 그 안의 수십 노드 호출은 이 id 를 공유).
fn resolve_implementer(emit: &Emit) -> Result<String, String> {
    let ask = |contract: &str| -> Value {
        let env = emit.call("plugin.implementers", json!({ "contract": contract }), None);
        if env.get("ok").and_then(|v| v.as_bool()) == Some(true) {
            env.get("data").cloned().unwrap_or(Value::Null)
        } else {
            Value::Null
        }
    };
    let boards = ask(BOARD_CONTRACT);
    let stores = ask(PROMPT_CONTRACT);
    pick_implementer(&boards, &stores).ok_or_else(|| {
        format!("{BOARD_CONTRACT} 와 {PROMPT_CONTRACT} 를 모두 구현한 플러그인 없음 — 노드가 지닌 프롬프트 주소를 읽으려면 카드를 든 보드가 곧 그 텍스트를 든 저장소여야 한다")
    })
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// auth env 수집 — ANTHROPIC_* + 토큰/OAuth 우선순위. 스폰 env 경계에서만 사용한다.
fn auth_env() -> Result<Vec<(String, String)>, String> {
    let all: Vec<(String, String)> = std::env::vars()
        .filter(|(k, _)| {
            k.starts_with("ANTHROPIC_")
                || k == "CLAUDE_ACCOUNT_NAME"
                || k == "CLAUDE_CODE_OAUTH_TOKEN"
        })
        .collect();
    let has_token = all.iter().any(|(k, _)| k == "ANTHROPIC_AUTH_TOKEN");
    let has_oauth = all.iter().any(|(k, _)| k == "CLAUDE_CODE_OAUTH_TOKEN");
    let is_codex = std::env::var("SOKSAK_WORKFLOW_PROVIDER").ok().as_deref() == Some("codex");
    if !has_token && !has_oauth && !is_codex {
        return Err(
            "프로필 인증 토큰 미설정 — ANTHROPIC_AUTH_TOKEN 또는 CLAUDE_CODE_OAUTH_TOKEN"
                .to_string(),
        );
    }
    if has_token {
        Ok(all
            .into_iter()
            .filter(|(k, _)| k.starts_with("ANTHROPIC_") || k == "CLAUDE_ACCOUNT_NAME")
            .collect())
    } else {
        Ok(all)
    }
}

// ── in-process execution ─────────────────────────────────────

/// exec-one — {prompt, schema?, model?} 한 노드 실행 → {oxf, result}.
fn exec_one_inprocess(body: &str) -> Result<Value, String> {
    let input = exec_one::parse_input(body)?;
    let model = input
        .model
        .clone()
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());
    // effort = 노드가 실은 tier, 미지정이면 최고(품질우선 — under-fund 방지).
    let effort = input
        .effort
        .clone()
        .unwrap_or_else(|| crate::provider::DEFAULT_EFFORT.to_string());
    let env = auth_env()?;
    let full = build_prompt_with_schema(&input.prompt, None, Some(&Language::parse("ko")));
    let has_schema = input.schema.is_some();
    let req = AgentRequest {
        prompt: full,
        model: &model,
        allowed_tools: vec![],
        timeout_secs: 7200,
        system_prompt: None,
        schema: input.schema,
        effort,
        text_only: false,
    };
    let result = if has_schema {
        run_agent(&req, &env)?
    } else {
        Value::String(run_agent_text(&req, &env)?)
    };
    Ok(exec_one::build_output(result))
}

// stage 실행 모드 — 정본 LLM / --assemble(패키지만) / --with-output(재생).
enum StageMode {
    Normal,
    Assemble,
    WithOutput(Value),
}

/// exec-stage doc 실행(main.rs run_exec_stage_doc) — {workflow|skeleton, stage, args} → 산출.
fn exec_stage_inprocess(body: &str, mode: StageMode) -> Result<Value, String> {
    let input: Value =
        serde_json::from_str(body).map_err(|e| format!("exec-stage 입력 파싱: {e}"))?;
    let stage = input
        .get("stage")
        .and_then(|s| s.as_str())
        .ok_or("stage 필드 필수")?
        .to_string();
    // doc 결정: workflow(번들 이름) 또는 skeleton(임베드).
    let doc: Value = if let Some(name) = input.get("workflow").and_then(|w| w.as_str()) {
        serde_json::from_str(crate::paths::bundled_workflow(name)?)
            .map_err(|e| format!("parse {name}: {e}"))?
    } else if let Some(sk) = input.get("skeleton") {
        sk.clone()
    } else {
        return Err("exec-stage: workflow 또는 skeleton 필수".to_string());
    };
    let lang = Language::parse("ko");
    let mut args_obj = match input.get("args") {
        Some(Value::Object(m)) => m.clone(),
        _ => Map::new(),
    };
    args_obj.insert("stage".to_string(), Value::String(stage.clone()));
    args_obj.insert("lang".to_string(), Value::String(lang.code.clone()));
    let args_json = Value::Object(args_obj);

    match mode {
        StageMode::Assemble => {
            // agent 턴의 {prompt, schema} 패키지만 포획(LLM 0).
            let mut captured: Option<Value> = None;
            let mut cap_fn = |prompt: &str,
                              schema: Option<&Value>,
                              label: &str|
             -> Result<Value, String> {
                captured = Some(
                    json!({ "prompt": build_prompt_with_schema(prompt, None, Some(&lang)), "schema": schema, "label": label }),
                );
                Err("__assemble_capture__".to_string())
            };
            let _ = doc_exec::run(&doc, &stage, &args_json, &mut cap_fn);
            match captured {
                Some(pkg) => Ok(json!({ "assembled": pkg })),
                None => Err("assemble: agent 턴 없음(stage 미도달)".to_string()),
            }
        }
        StageMode::WithOutput(out) => {
            // 외부 실행자 산출을 agent 위치에 주입(LLM 0).
            let mut used = false;
            let mut inj_fn = |_p: &str, _s: Option<&Value>, _l: &str| -> Result<Value, String> {
                used = true;
                Ok(out.clone())
            };
            let (events, result) = doc_exec::run(&doc, &stage, &args_json, &mut inj_fn)?;
            let _ = used;
            shape_stage_output(&stage, events, result)
        }
        StageMode::Normal => {
            let model = input
                .get("model")
                .and_then(|m| m.as_str())
                .map(String::from)
                .unwrap_or_else(|| DEFAULT_MODEL.to_string());
            // effort = stage 입력의 tier(저작 LLM 난이도), 미지정이면 최고(품질우선).
            let effort = input
                .get("effort")
                .and_then(|m| m.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from)
                .unwrap_or_else(|| crate::provider::DEFAULT_EFFORT.to_string());
            let env = auth_env()?;
            let mut agent_fn =
                |prompt: &str, schema: Option<&Value>, label: &str| -> Result<Value, String> {
                    let full = build_prompt_with_schema(prompt, None, Some(&lang));
                    let req = AgentRequest {
                        prompt: full,
                        model: &model,
                        allowed_tools: vec![],
                        timeout_secs: 3600,
                        system_prompt: None,
                        schema: schema.cloned(),
                        effort: effort.clone(),
                        text_only: false,
                    };
                    if schema.is_some() {
                        run_agent(&req, &env).map_err(|e| format!("agent {label:?} 실패: {e}"))
                    } else {
                        run_agent_text(&req, &env)
                            .map(Value::String)
                            .map_err(|e| format!("agent {label:?} 실패: {e}"))
                    }
                };
            let (events, result) = doc_exec::run(&doc, &stage, &args_json, &mut agent_fn)?;
            shape_stage_output(&stage, events, result)
        }
    }
}

// stage 산출을 StageOut 로(main.rs emit_stage_output 의 in-process 판). generate→DraftDoc, 그 외→Children.
fn shape_stage_output(stage: &str, events: Vec<NodeEvent>, result: Value) -> Result<Value, String> {
    if stage == "generate" {
        let mut ddoc = draft_doc::build(&events)?;
        if let Some(Value::String(t)) = result.get("chunkTitle") {
            if !t.is_empty() {
                ddoc.chunk_title = Some(t.clone());
            }
        }
        if let Err(violations) = draft_doc::validate(&ddoc) {
            return Err(format!(
                "generate DraftDoc 검증 실패({}건) — 발행 거부",
                violations.len()
            ));
        }
        Ok(json!({ "draftDoc": serde_json::to_value(&ddoc).map_err(|e| e.to_string())? }))
    } else {
        let children: Vec<Value> = events
            .iter()
            .filter_map(|ev| serde_json::to_value(ev).ok())
            .collect();
        Ok(json!({ "children": children, "result": result }))
    }
}

// shape_stage_output 결과 Value → StageOut(reconcile 소비형).
fn to_stage_out(v: Value) -> Result<StageOut, String> {
    if let Some(d) = v.get("draftDoc") {
        Ok(StageOut::DraftDoc(d.clone()))
    } else {
        let children = v
            .get("children")
            .and_then(|c| c.as_array())
            .cloned()
            .unwrap_or_default();
        let result = v.get("result").cloned().unwrap_or(Value::Null);
        Ok(StageOut::Children { children, result })
    }
}

// ── production Deps — board/scheduler는 중개 cmd, exec는 in-process ──
struct ProdDeps<'a> {
    emit: &'a Emit,
    poke_schedule: String,
    /// 계약 교집합으로 해소된 구현체 id(resolve_implementer). 이 필드가 유일한 구현체 지식이고,
    /// 매니페스트·소스 어디에도 그 이름은 적혀 있지 않다.
    board: String,
}

impl ProdDeps<'_> {
    // 발견한 구현체의 명령 이름. 구현체 id 는 여기 한 곳에서만 문자열로 합쳐진다.
    fn cmd(&self, name: &str) -> String {
        format!("plugin.{}.{}", self.board, name)
    }

    // 중개 호출 봉투를 해석한다. ok:false는 None이다.
    fn call_data(&self, method: &str, params: Value) -> Option<Value> {
        let env = self.emit.call(method, params, None);
        if env.get("ok").and_then(|v| v.as_bool()) == Some(true) {
            Some(env.get("data").cloned().unwrap_or(Value::Null))
        } else {
            None
        }
    }
}

/// node.edit 와이어 파라미터 조립 — 변경 필드는 top-level 로 편다("fields" 중첩 아님). 보드 계약
/// (node.edit)은 평문 { node, title?, description?, status?, badge?, result? … } 를 읽으므로 감싸면
/// 보드가 top-level 만 보고 변경을 조용히 드롭한다. 순수: emit 없이 와이어 shape 를 결정적으로 검증한다.
fn node_edit_params(id: &str, fields: Value) -> Value {
    // json! 은 spread 가 없다 — fields 객체를 base 로 취하고 그 위에 top-level 엔트리를 편다.
    let mut map = match fields {
        Value::Object(entries) => entries,
        _ => Map::new(),
    };
    // node 는 마지막에 스탬핑한다 — 대상 id 는 인자이지 fields blob 이 덮을 값이 아니다.
    map.insert("node".to_string(), Value::String(id.to_string()));
    Value::Object(map)
}

impl Deps for ProdDeps<'_> {
    fn list_nodes(&self) -> Vec<Node> {
        self.call_data(&self.cmd("node.list"), json!({ "limit": 100000 }))
            .and_then(|d| d.get("nodes").cloned())
            .and_then(|n| serde_json::from_value(n).ok())
            .unwrap_or_default()
    }
    fn get_node(&self, id: &str) -> Option<Node> {
        self.call_data(&self.cmd("node.get"), json!({ "node": id }))
            .and_then(|d| d.get("node").cloned())
            .and_then(|n| serde_json::from_value(n).ok())
    }
    fn edit_node(&self, id: &str, fields: Value) -> EditResult {
        let env = self
            .emit
            .call(&self.cmd("node.edit"), node_edit_params(id, fields), None);
        if env.get("ok").and_then(|v| v.as_bool()) == Some(true) {
            EditResult::ok()
        } else {
            EditResult::err(
                env.get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("")
                    .to_string(),
            )
        }
    }
    fn add_node(&self, params: Value) -> Option<String> {
        self.call_data(&self.cmd("node.add"), params)
            .and_then(|d| d.get("nodeId").and_then(|v| v.as_str()).map(String::from))
    }
    fn poke(&self) {
        let _ = self
            .emit
            .call("schedule.poke", json!({ "id": self.poke_schedule }), None);
    }
    fn progress(&self, cmd: &str, delta: &str) {
        self.emit.progress(cmd, json!(delta));
    }
    fn exec_one(&self, body: &str) -> Result<Value, String> {
        exec_one_inprocess(body)
    }
    fn exec_stage(&self, body: &str) -> Result<StageOut, String> {
        to_stage_out(exec_stage_inprocess(body, StageMode::Normal)?)
    }
    fn materialize_ledger(&self, chunk_id: &str) -> Result<Vec<Value>, String> {
        Ok(build_ledger(&self.list_nodes(), chunk_id, "item"))
    }
    fn materialize_facts(&self, chunk_id: &str) -> Result<Vec<Value>, String> {
        Ok(build_ledger(&self.list_nodes(), chunk_id, "fact"))
    }
    fn put_prompt(&self, value: Value) -> Option<String> {
        self.call_data(&self.cmd("prompt.put"), json!({ "value": value }))
            .and_then(|d| d.get("hash").and_then(|v| v.as_str()).map(String::from))
    }
    fn resolve_prompt(&self, hash: &str, vars: Value, refs: Value) -> Option<Value> {
        self.call_data(
            &self.cmd("prompt.resolve"),
            json!({ "hash": hash, "vars": vars, "refs": refs }),
        )
    }
    fn get_prompt(&self, hash: &str) -> Option<Value> {
        self.call_data(&self.cmd("prompt.get"), json!({ "hash": hash }))
    }
    fn has_assemble_stage(&self) -> bool {
        true
    }
    fn assemble_stage(&self, body: &str) -> Result<Value, String> {
        exec_stage_inprocess(body, StageMode::Assemble)
    }
    fn has_exec_stage_with_output(&self) -> bool {
        true
    }
    fn exec_stage_with_output(&self, body: &str, out: Value) -> Result<StageOut, String> {
        to_stage_out(exec_stage_inprocess(body, StageMode::WithOutput(out))?)
    }
    fn write_file(&self, rel: &str, content: &str) {
        // export — 코어 fs 쓰기는 서비스 권한 밖. 파일 쓰기는 딕렉토리 상대라 os fs 로 직접(상대경로는
        // export 핸들러가 dir 와 합쳐 절대화). 여기선 rel 만 받으므로 핸들러가 dir 합성 후 절대경로를 준다.
        if let Some(parent) = std::path::Path::new(rel).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(rel, content);
    }
}

// 초기 DAG 발행 — doc emit → NodeEvent → board relay. LLM 0.
fn publish_doc(
    deps: &dyn Deps,
    doc: &Value,
    args: &Value,
    task_ctx: Option<&Value>,
) -> Result<usize, String> {
    if !doc_exec::is_doc(doc) {
        return Err("workflow-doc@0.0.1 필요(spec 필드)".to_string());
    }
    let mut no_agent = |_p: &str, _s: Option<&Value>, _l: &str| -> Result<Value, String> {
        Err("발행(--emit)은 agent 를 호출하지 않는다".to_string())
    };
    let (events, _result) = doc_exec::run(doc, "", args, &mut no_agent)?;
    relay_events(deps, &events, task_ctx)
}

// NodeEvent 스트림을 kanban 으로 릴레이(keyOf 배치 해결) — 발행/자식 공용.
fn relay_events(
    deps: &dyn Deps,
    events: &[NodeEvent],
    task_ctx: Option<&Value>,
) -> Result<usize, String> {
    let mut key_of: HashMap<String, String> = HashMap::new();
    let mut role_to_hash: HashMap<String, String> = HashMap::new();
    let mut published = 0;
    for ev in events {
        let ev_val = serde_json::to_value(ev).map_err(|e| e.to_string())?;
        if let Some(reg) = ev_val
            .get("register_prompts")
            .or_else(|| ev_val.get("registerPrompts"))
        {
            for (r, h) in register_prompt_templates(reg, deps) {
                role_to_hash.insert(r, h);
            }
        }
        let parent = ev_val
            .get("parent")
            .and_then(|v| v.as_str())
            .map(|p| key_of.get(p).cloned().unwrap_or_else(|| p.to_string()));
        let blocked_by: Vec<String> = ev_val
            .get("blocked_by")
            .or_else(|| ev_val.get("blockedBy"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|id| id.as_str())
                    .map(|id| key_of.get(id).cloned().unwrap_or_else(|| id.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let params = build_add_params(
            &ev_val,
            parent.as_deref(),
            &blocked_by,
            task_ctx,
            &role_to_hash,
        );
        match deps.add_node(params) {
            Some(id) => {
                if let Some(evid) = ev_val.get("id").and_then(|v| v.as_str()) {
                    key_of.insert(evid.to_string(), id);
                }
                published += 1;
            }
            None => return Err(format!("노드 발행 실패(relay {published}건)")),
        }
    }
    Ok(published)
}

// ── WorkflowService(serve 핸들러) ────────────────────────────────────────────
#[derive(Default)]
struct Runtime {
    skeleton: Option<Value>,
    directive: Option<String>,
    workflow_ref: Option<String>,
}

pub struct WorkflowService {
    state: Mutex<(ReconcileState, Runtime)>,
    poke_schedule: String,
}

impl WorkflowService {
    fn new() -> Self {
        let plugin = std::env::var("SOKSAK_SERVICE_PLUGIN")
            .unwrap_or_else(|_| "soksak-plugin-workflow".to_string());
        WorkflowService {
            state: Mutex::new((ReconcileState::default(), Runtime::default())),
            poke_schedule: format!("svc:{plugin}:reconcile"),
        }
    }
}

fn ok_outcome(v: Value) -> Outcome {
    let ok = v.get("ok").and_then(|b| b.as_bool()).unwrap_or(false);
    let message = v.get("message").and_then(|m| m.as_str()).map(String::from);
    let code = v.get("code").and_then(|c| c.as_str()).map(String::from);
    let mut data = v.clone();
    if let Some(o) = data.as_object_mut() {
        o.remove("ok");
    }
    if ok {
        Outcome {
            ok: true,
            code,
            message,
            hints: vec![],
            data: Some(data),
        }
    } else {
        Outcome {
            ok: false,
            code: code.or(Some("INTERNAL".into())),
            message: message.or(Some("error".into())),
            hints: vec![],
            data: Some(data),
        }
    }
}

fn validate_request(op: &str, params: &Value) -> Result<(), Outcome> {
    let required_string = |name: &str| {
        params
            .get(name)
            .and_then(Value::as_str)
            .ok_or_else(|| Outcome::err(ErrCode::InvalidParams, &format!("{name} 필수")))
    };

    match op {
        "ping" | "reconcile" | "next" | "submit" => Ok(()),
        "issuerize" | "research" => {
            required_string("chunk")?;
            Ok(())
        }
        "export" => {
            required_string("chunk")?;
            required_string("dir")?;
            Ok(())
        }
        "run" => {
            if let Some(skeleton) = params.get("skeleton").and_then(Value::as_str) {
                let document: Value = serde_json::from_str(skeleton).map_err(|error| {
                    Outcome::err(ErrCode::InvalidParams, &format!("skeleton 파싱: {error}"))
                })?;
                if document.is_null() {
                    return Err(Outcome::err(ErrCode::InvalidParams, "skeleton 파싱 실패"));
                }
                Ok(())
            } else if params.get("idea").and_then(Value::as_str).is_some() {
                Ok(())
            } else {
                Err(Outcome::err(
                    ErrCode::InvalidParams,
                    "idea 또는 skeleton 필수",
                ))
            }
        }
        other => Err(Outcome::err(ErrCode::UnknownOp, other)),
    }
}

impl ServiceHandler for WorkflowService {
    fn ops(&self) -> Vec<String> {
        crate::interface::SERVICE_OPS
            .iter()
            .map(|s| s.to_string())
            .collect()
    }
    fn subscribe(&self) -> Vec<String> {
        // 토픽은 보드 계약이 소유한다 — 구현체 이름을 토픽에 박으면 그게 이름-핀이고, 보드를
        // 갈아끼우는 순간 에러 하나 없이 구독이 끊긴다.
        vec![format!("bus:{BOARD_CHANGED}")]
    }
    fn read_only(&self, op: &str) -> bool {
        op == "ping"
    }
    fn on_push(&self, topic: &str, _seq: u64, _payload: Value) {
        // 보드 변경 → reconcile 재발화(부팅 poke 와 동형). 여기선 poke 만.
        if topic == format!("bus:{BOARD_CHANGED}") {
            // 서비스가 자기 스케줄을 poke — 실제 발화는 코어 스케줄러가 reconcile 커맨드로.
            // push 핸들러는 emit 접근이 없어(하니스 설계) 직접 poke 불가 — 코어 bus 브리지가 이미
            // reconcile 트리거를 소유하므로 no-op(트리거 채널은 코어측). 진행 self-poke 는 deps.poke.
        }
    }
    fn handle(&self, op: &str, params: Value, ctx: &OpCtx, emit: &Emit) -> Outcome {
        // 라이브니스는 상태 락 밖에서 처리한다 — 다른 op 가 스테이지 LLM 으로 락을 오래(최대 캡) 쥐어도
        // ping 은 즉시 응답해야 "서비스 살았나"를 판정할 수 있다. LLM·rate limit 과도 분리(스모크는 {llm:true}).
        if op == "ping" {
            if params.get("llm").and_then(|b| b.as_bool()) == Some(true) {
                return match exec_one_inprocess(
                    r#"{"prompt":"다음 항목을 판정하라: \"1 더하기 1은 2다\". 참이면 oxf=o."}"#,
                ) {
                    Ok(o) => ok_outcome(
                        json!({ "ok": true, "alive": true, "llm": true, "oxf": o.get("oxf") }),
                    ),
                    Err(e) => Outcome::err(ErrCode::Internal, &e),
                };
            }
            return ok_outcome(json!({ "ok": true, "alive": true, "model": DEFAULT_MODEL }));
        }
        if let Err(outcome) = validate_request(op, &params) {
            return outcome;
        }
        // ping 을 뺀 모든 op 는 보드 위에서 돈다 — 구현체를 먼저 해소한다. 교집합이 비면 조용히
        // 아무 일도 안 한 척(빈 노드 목록으로 no-op) 넘기지 않는다: 그건 "할 일이 없었다" 와
        // "실행할 보드가 없었다" 를 같은 침묵으로 답하는 것이고, 후자는 사람이 고쳐야 할 사실이다.
        // 락 밖에서 해소한다 — 발견 왕복 동안 뮤텍스를 쥐고 있을 이유가 없다.
        let board = match resolve_implementer(emit) {
            Ok(id) => id,
            Err(e) => return Outcome::err(ErrCode::Unavailable, &e),
        };
        let mut guard = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let (state, runtime) = &mut *guard;
        let deps = ProdDeps {
            emit,
            poke_schedule: self.poke_schedule.clone(),
            board,
        };
        let _ = ctx;
        match op {
            "reconcile" => ok_outcome(reconcile_tick(&deps, state, now_ms())),
            "next" => {
                let chunk = params.get("chunk").and_then(|c| c.as_str());
                ok_outcome(next_tick(&deps, state, chunk, now_ms()))
            }
            "submit" => {
                let node = params.get("node").and_then(|n| n.as_str()).unwrap_or("");
                let output = params.get("output").cloned().unwrap_or(Value::Null);
                ok_outcome(submit_tick(&deps, state, node, &output))
            }
            "issuerize" => match params.get("chunk").and_then(|c| c.as_str()) {
                Some(chunk) => ok_outcome(issuerize_tick(&deps, chunk)),
                None => Outcome::err(ErrCode::InvalidParams, "chunk(덩어리 id) 필수"),
            },
            "export" => {
                let chunk = params.get("chunk").and_then(|c| c.as_str());
                let dir = params.get("dir").and_then(|d| d.as_str());
                match (chunk, dir) {
                    (Some(chunk), Some(dir)) => {
                        // write_file 은 상대경로 — dir 합성 절대화를 위해 export_tick 의 rel 을 dir 밑으로.
                        let deps_dir = DirDeps {
                            inner: &deps,
                            dir: dir.to_string(),
                        };
                        ok_outcome(export_tick(&deps_dir, chunk, dir))
                    }
                    _ => Outcome::err(ErrCode::InvalidParams, "chunk·dir 필수"),
                }
            }
            "research" => {
                let chunk = match params.get("chunk").and_then(|c| c.as_str()) {
                    Some(c) => c,
                    None => return Outcome::err(ErrCode::InvalidParams, "chunk(덩어리 id) 필수"),
                };
                let gate = research_gate(&deps, chunk);
                if gate.get("ok").and_then(|b| b.as_bool()) != Some(true) {
                    return ok_outcome(gate);
                }
                let directive = gate
                    .get("directive")
                    .and_then(|d| d.as_str())
                    .map(String::from);
                runtime.workflow_ref = Some("research".into());
                runtime.skeleton = None;
                runtime.directive = directive.clone();
                let task_ctx = json!({ "workflowRef": "research", "directive": directive });
                let doc = match load_bundle("research") {
                    Ok(d) => d,
                    Err(e) => return Outcome::err(ErrCode::Internal, &e),
                };
                let args = json!({ "chunkRef": chunk, "directive": directive, "lang": "ko" });
                match publish_doc(&deps, &doc, &args, Some(&task_ctx)) {
                    Ok(published) => {
                        deps.poke();
                        ok_outcome(json!({ "ok": true, "published": published }))
                    }
                    Err(e) => Outcome::err(ErrCode::Internal, &e),
                }
            }
            "run" => run_publish(&deps, runtime, params),
            other => Outcome::err(ErrCode::UnknownOp, other),
        }
    }
}

// 번들 정본 workflow-doc 로드.
fn load_bundle(name: &str) -> Result<Value, String> {
    serde_json::from_str(crate::paths::bundled_workflow(name)?)
        .map_err(|e| format!("parse {name}: {e}"))
}

// run 발행 — idea/skeleton → 초기 DAG 발행(LLM은 skeleton 생성 시에만 사용).
fn run_publish(deps: &dyn Deps, runtime: &mut Runtime, params: Value) -> Outcome {
    let idea = params.get("idea").and_then(|v| v.as_str());
    let skeleton_str = params.get("skeleton").and_then(|v| v.as_str());
    let directive_param = params.get("directive").and_then(|v| v.as_str());

    // skeleton 결정: 명시 skeleton > idea→generate_skeleton(LLM).
    let doc: Value = if let Some(sk) = skeleton_str {
        serde_json::from_str(sk)
            .map_err(|e| format!("skeleton 파싱: {e}"))
            .unwrap_or(Value::Null)
    } else if let Some(idea) = idea {
        match generate_skeleton_inprocess(idea, params.get("model").and_then(|m| m.as_str())) {
            Ok(d) => d,
            Err(e) => return Outcome::err(ErrCode::Internal, &e),
        }
    } else {
        return Outcome::err(ErrCode::InvalidParams, "idea 또는 skeleton 필수");
    };
    if doc.is_null() {
        return Outcome::err(ErrCode::InvalidParams, "skeleton 파싱 실패");
    }
    runtime.skeleton = Some(doc.clone());
    runtime.workflow_ref = None;
    runtime.directive = resolve_directive(directive_param, Some(&doc), idea);
    let task_ctx = json!({ "skeleton": doc, "directive": runtime.directive });
    let args = json!({ "lang": "ko" });
    match publish_doc(deps, &doc, &args, Some(&task_ctx)) {
        Ok(published) => {
            deps.poke();
            ok_outcome(json!({ "ok": true, "published": published }))
        }
        Err(e) => Outcome::err(ErrCode::Internal, &e),
    }
}

// idea → workflow-doc(LLM 저작) — CLI run_generate_skeleton 과 동일 lib 경로(generate_skeleton::generate_doc).
// 정련 지침과 workflow 문서는 바이너리에 포함되어 설치 경로와 무관하다.
fn generate_skeleton_inprocess(idea: &str, model: Option<&str>) -> Result<Value, String> {
    let model = model.unwrap_or(DEFAULT_MODEL);
    let env = auth_env()?;
    let lang = Language::parse("ko");
    crate::generate_skeleton::generate_doc(
        idea,
        model,
        Some(&lang),
        crate::paths::draft_skill(),
        &env,
        None,
    )
}

// export write_file 을 dir 밑으로 절대화하는 얇은 래퍼.
struct DirDeps<'a> {
    inner: &'a dyn Deps,
    dir: String,
}
impl Deps for DirDeps<'_> {
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
        let full = format!("{}/{}", self.dir, rel);
        self.inner.write_file(&full, content);
    }
}

/// serve 진입점 — 코어가 `<bin> serve`로 스폰한다.
pub fn run_serve() -> Result<(), String> {
    serve_stdio(WorkflowService::new());
    Ok(())
}

#[cfg(test)]
mod implementer_tests {
    use super::pick_implementer;
    use serde_json::json;

    fn found(xs: &[(&str, &str)]) -> serde_json::Value {
        json!({ "implementers": xs.iter().map(|(id, status)| json!({ "id": id, "status": status })).collect::<Vec<_>>() })
    }

    #[test]
    fn picks_the_plugin_that_implements_both_contracts() {
        // 보드만 구현한 가짜가 먼저 답해도 그것을 고르면 안 된다 — 노드가 지닌 프롬프트 주소를
        // 그 보드는 읽지 못한다. 선택은 교집합이다.
        let boards = found(&[("board-only", "enabled"), ("both", "enabled")]);
        let stores = found(&[("both", "enabled"), ("store-only", "enabled")]);
        assert_eq!(pick_implementer(&boards, &stores).as_deref(), Some("both"));
    }

    #[test]
    fn no_plugin_implements_both_is_no_pick() {
        let boards = found(&[("board-only", "enabled")]);
        let stores = found(&[("store-only", "enabled")]);
        assert_eq!(
            pick_implementer(&boards, &stores),
            None,
            "반쯤 맞는 보드를 고르느니 고르지 않는다"
        );
    }

    #[test]
    fn a_disabled_plugin_satisfies_neither_contract() {
        assert_eq!(
            pick_implementer(&found(&[("b", "disabled")]), &found(&[("b", "enabled")])),
            None
        );
        assert_eq!(
            pick_implementer(&found(&[("b", "enabled")]), &found(&[("b", "disabled")])),
            None
        );
    }

    #[test]
    fn nothing_discovered_is_no_pick_not_a_panic() {
        assert_eq!(pick_implementer(&json!({}), &json!({})), None);
        assert_eq!(pick_implementer(&found(&[]), &found(&[])), None);
    }
}

#[cfg(test)]
mod edit_params_tests {
    use super::node_edit_params;
    use serde_json::json;

    // 보드 계약(node.edit)은 평문 { node, badge?, result?, status? … } 를 읽는다. 변경 필드를
    // "fields" 로 감싸면 보드가 top-level 만 보므로 badge/result/status 쓰기가 조용히 드롭된다.
    #[test]
    fn edit_params_are_flat_never_wrapped_in_fields() {
        let p = node_edit_params(
            "WMP-1",
            json!({ "badge": "o", "result": "완료", "status": "done" }),
        );
        assert_eq!(p.get("node").and_then(|v| v.as_str()), Some("WMP-1"));
        assert!(
            p.get("fields").is_none(),
            "변경 필드를 감싸면 보드가 무시한다 — top-level 로 편다"
        );
        assert_eq!(p.get("badge").and_then(|v| v.as_str()), Some("o"));
        assert_eq!(p.get("result").and_then(|v| v.as_str()), Some("완료"));
        assert_eq!(p.get("status").and_then(|v| v.as_str()), Some("done"));
    }

    // node 는 변경 필드가 덮어쓸 수 없다 — fields 에 node 키가 섞여도 대상 id 가 이긴다.
    #[test]
    fn node_id_is_not_overwritten_by_a_stray_field() {
        let p = node_edit_params("WMP-2", json!({ "node": "WMP-999", "title": "t" }));
        assert_eq!(p.get("node").and_then(|v| v.as_str()), Some("WMP-2"));
        assert_eq!(p.get("title").and_then(|v| v.as_str()), Some("t"));
    }
}
