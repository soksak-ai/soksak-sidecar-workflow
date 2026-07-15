//! doc_exec — workflow-doc@0.0.1(언어중립 JSON 워크플로 문서) 실행기.
//!
//! 저작 LLM은 선언형 문서를 만들고 실행기는 검증된 stage만 수행한다. publish는 NodeEvent,
//! agent는 주입된 러너, generate 산출은 draft_doc build/validate 경계로 흐른다.
//!
//! 문서 형태:
//!   { "spec": "workflow-doc@0.0.1", "meta": {name, description},
//!     "args":    { name: {"from": ["directive","DIRECTIVE",…], "default": any} },   // 실행 인자 해석(우선순위)
//!     "values":  { name: any },            // 상수(프롬프트 조각·스키마·템플릿) — **렌더하지 않음**(verbatim)
//!     "prompts": { name: "…{{ref}}…" },    // agent 실행 시 렌더. ref = values.* | args.* | ledger(빌트인)
//!     "stages":  { ""|"generate"|…: [op…] } }  // "" = skeleton(--emit, agent 금지)
//! op 5종:
//!   {"op":"agent","prompt":p,"schema"?:valueName,"label"?:s,"bind":var}
//!   {"op":"forEach","in":path,"when"?:path,"collect"?:var,"do":[op…]}   // item/index 바인딩
//!   {"op":"publish","node":{…},"when"?:path,"unless"?:path}  // 값=리터럴|{"$":path,"or"?:def}|id={"auto":prefix}; when=truthy·unless=falsy 일 때만 발행(빈 배열=falsy)
//!   {"op":"return","value":{k: expr}}
//! path = "root.seg.seg" — root 는 locals(bind/item/index/collect) → "args" → "values".

use crate::emit_host::NodeEvent;
use serde_json::{Map, Value as Json};

pub const SPEC: &str = "workflow-doc@0.0.1";

/// is_doc — 입력 JSON이 workflow-doc@0.0.1 문서인가.
pub fn is_doc(v: &Json) -> bool {
    v.get("spec").and_then(|s| s.as_str()) == Some(SPEC)
}

/// agent 러너 — (렌더된 prompt, schema, label) → 결과 JSON. 실행 컨텍스트가 주입(claude/stub).
pub type AgentFn<'a> = dyn FnMut(&str, Option<&Json>, &str) -> Result<Json, String> + 'a;

// ── values 조성 ──────────────────────────────────────────────

/// resolved_values — values 로드-시 조성: `{"concat":[문자열|{"$":"values.X"}…]}` 를 문자열로 접는다.
/// 참조 대상은 **plain 문자열 값**만(1단 — concat-of-concat 금지, fail-loud).
/// 용도: VERIFY_TMPL 이 COMMON 을 단일 원천으로 포함(문서 내 중복 0)하면서도 등록(registerPrompts) 시점엔
/// 완성 텍스트로 나간다 — {{COMMON}} 렌더 마커를 값에 남기면 소비 시점(kanban resolve 는 vars/refs 만 앎)에
/// 치환되지 않은 채 프롬프트에 새는 사고가 나므로, 조성은 실행기 로드 시점에 끝낸다.
pub fn resolved_values(doc: &Json) -> Result<Map<String, Json>, String> {
    let Some(m) = doc.get("values").and_then(|v| v.as_object()) else {
        return Ok(Map::new());
    };
    let mut out = Map::new();
    for (k, v) in m {
        if !(v.is_object() && v.get("concat").is_some()) {
            out.insert(k.clone(), v.clone());
        }
    }
    for (k, v) in m {
        if let Some(parts) = v.get("concat").and_then(|c| c.as_array()) {
            let mut s = String::new();
            for p in parts {
                match p {
                    Json::String(lit) => s.push_str(lit),
                    Json::Object(o) => {
                        let path = o.get("$").and_then(|x| x.as_str()).ok_or_else(|| {
                            format!("values.{k} concat 원소 — 문자열 또는 {{\"$\":\"values.X\"}}")
                        })?;
                        let name = path.strip_prefix("values.").ok_or_else(|| {
                            format!("values.{k} concat 은 values.* 만 참조({path:?})")
                        })?;
                        match out.get(name) {
                            Some(Json::String(t)) => s.push_str(t),
                            _ => {
                                return Err(format!(
                                    "values.{k} concat 참조 {name:?} — plain 문자열 값 아님"
                                ))
                            }
                        }
                    }
                    _ => return Err(format!("values.{k} concat 원소는 문자열|{{\"$\"}} 만")),
                }
            }
            out.insert(k.clone(), Json::String(s));
        }
    }
    Ok(out)
}

/// inject_refinement — 번들 정본 골격(draft template)에 정련 산출을 주입해 실행 doc 을 조립(순수).
/// LLM 은 정련({directive, description})만 하고 상수(COMMON·스키마·프롬프트·stages)는 여기서 결정적으로
/// 합쳐진다 — 19KB verbatim 재타이핑(문자 1개 누락=전체 파손)을 구조적으로 제거(PRINCIPLES §7).
pub fn inject_refinement(template: &Json, directive: &str, description: &str) -> Json {
    let mut doc = template.clone();
    if let Some(d) = doc.pointer_mut("/args/directive/default") {
        *d = Json::String(directive.to_string());
    }
    if !description.is_empty() {
        if let Some(m) = doc.pointer_mut("/meta/description") {
            *m = Json::String(description.to_string());
        }
    }
    doc
}

// ── 검증(fail-loud) ──────────────────────────────────────────

/// validate — 문서 정합 인증. 위반 목록 반환(빈 목록 = 통과). 저작 게이트(generate-skeleton)와
/// 실행 진입(--emit/exec-stage) 양단에서 강제한다.
pub fn validate(doc: &Json) -> Result<(), Vec<String>> {
    let mut v: Vec<String> = vec![];
    if !is_doc(doc) {
        return Err(vec![format!("[spec] spec ≠ {SPEC:?}")]);
    }
    let name = doc
        .pointer("/meta/name")
        .and_then(|n| n.as_str())
        .unwrap_or("");
    if name.trim().is_empty() {
        v.push("[meta] meta.name 비어있음".to_string());
    }
    // values 는 조성(concat) 해석 후 기준으로 검사 — 조성 실패 자체도 위반.
    let resolved: Map<String, Json>;
    let values = match resolved_values(doc) {
        Ok(m) => {
            resolved = m;
            Some(&resolved)
        }
        Err(e) => {
            v.push(format!("[values] {e}"));
            None
        }
    };
    let args_decl = doc.get("args").and_then(|x| x.as_object());
    let prompts = doc.get("prompts").and_then(|x| x.as_object());
    let stages = match doc.get("stages").and_then(|x| x.as_object()) {
        Some(s) if !s.is_empty() => s,
        _ => {
            v.push("[stages] stages 비어있음".to_string());
            return Err(v);
        }
    };

    // prompts 플레이스홀더 해석 가능성 — {{name}} ∈ values ∪ 선언 args ∪ {ledger}.
    if let Some(ps) = prompts {
        for (pname, tmpl) in ps {
            let Some(t) = tmpl.as_str() else {
                v.push(format!("[prompts] {pname:?} 문자열 아님"));
                continue;
            };
            for ph in placeholders(t) {
                let known = values.is_some_and(|m| m.contains_key(&ph))
                    || args_decl.is_some_and(|m| m.contains_key(&ph))
                    || ph == "ledger"
                    || ph == "facts"
                    || ph == "round"
                    || ph == "document";
                if !known {
                    v.push(format!("[prompts] {pname:?} 플레이스홀더 {{{{{ph}}}}} 미해석(values/args/ledger 아님)"));
                }
            }
        }
    }

    // stage op 재귀 검증.
    for (sname, ops) in stages {
        let Some(list) = ops.as_array() else {
            v.push(format!("[stages] {sname:?} 가 op 배열 아님"));
            continue;
        };
        let mut literal_ids: std::collections::BTreeSet<String> = Default::default();
        validate_ops(sname, list, prompts, values, &mut literal_ids, &mut v);
        // skeleton stage("") 는 agent 금지 — --emit 은 LLM 미호출 계약.
        if sname.is_empty() && ops_contain_agent(list) {
            v.push(
                "[stages] skeleton stage(\"\") 에 agent op — 발행(--emit)은 LLM 미호출 계약"
                    .to_string(),
            );
        }
    }

    if v.is_empty() {
        Ok(())
    } else {
        Err(v)
    }
}

fn validate_ops(
    stage: &str,
    ops: &[Json],
    prompts: Option<&Map<String, Json>>,
    values: Option<&Map<String, Json>>,
    literal_ids: &mut std::collections::BTreeSet<String>,
    v: &mut Vec<String>,
) {
    for op in ops {
        match op.get("op").and_then(|o| o.as_str()) {
            Some("agent") => {
                let p = op.get("prompt").and_then(|x| x.as_str()).unwrap_or("");
                if !prompts.is_some_and(|m| m.contains_key(p)) {
                    v.push(format!("[{stage}] agent.prompt {p:?} ∉ prompts"));
                }
                if let Some(s) = op.get("schema").and_then(|x| x.as_str()) {
                    let ok = values.and_then(|m| m.get(s)).is_some_and(|x| x.is_object());
                    if !ok {
                        v.push(format!("[{stage}] agent.schema {s:?} ∉ values(객체)"));
                    }
                }
                if op
                    .get("bind")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .is_empty()
                {
                    v.push(format!("[{stage}] agent.bind 누락"));
                }
            }
            Some("forEach") => {
                if op
                    .get("in")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .is_empty()
                {
                    v.push(format!("[{stage}] forEach.in 누락"));
                }
                match op.get("do").and_then(|x| x.as_array()) {
                    Some(inner) if !inner.is_empty() => {
                        validate_ops(stage, inner, prompts, values, literal_ids, v)
                    }
                    _ => v.push(format!("[{stage}] forEach.do 비어있음")),
                }
            }
            Some("publish") => {
                let Some(node) = op.get("node").and_then(|x| x.as_object()) else {
                    v.push(format!("[{stage}] publish.node 누락"));
                    continue;
                };
                if node
                    .get("kind")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .is_empty()
                {
                    v.push(format!("[{stage}] publish.node.kind 누락"));
                }
                match node.get("id") {
                    Some(Json::String(id)) => {
                        if !literal_ids.insert(id.clone()) {
                            v.push(format!("[{stage}] publish id {id:?} 중복"));
                        }
                    }
                    Some(Json::Object(m))
                        if m.get("auto")
                            .is_some_and(|a| a.as_str().is_some_and(|s| !s.is_empty())) => {}
                    _ => v.push(format!(
                        "[{stage}] publish.node.id — 리터럴 문자열 또는 {{\"auto\":prefix}} 필요"
                    )),
                }
                // schema 필드가 문자열이면 values 참조여야 한다(오타 fail-loud).
                if let Some(Json::String(s)) = node.get("schema") {
                    if !values.and_then(|m| m.get(s)).is_some_and(|x| x.is_object()) {
                        v.push(format!(
                            "[{stage}] publish.node.schema {s:?} ∉ values(객체)"
                        ));
                    }
                }
                // 등록 템플릿 소비 계약 — registerPromptsOnce 로 등록되는 템플릿(문자열 값)의 {{ph}}는
                // 소비 시점(exec-one, kanban prompt.resolve)에 vars(노드 필드 title/description/category
                // ∪ publish vars 키) ∪ varRefs 키로만 치환된다. 그 밖(ledger/facts 등 stage 전용 빌트인)은
                // 검증 프롬프트에 영구 미치환으로 샌다 — C4 실측이 잡은 결함의 재발 방지 lint.
                {
                    let mut allowed: std::collections::BTreeSet<String> =
                        ["title", "description", "category"]
                            .iter()
                            .map(|s| s.to_string())
                            .collect();
                    if let Some(vars) = node.get("vars").and_then(|x| x.as_object()) {
                        allowed.extend(vars.keys().cloned());
                    }
                    if let Some(refs) = node.get("varRefs").and_then(|x| x.as_object()) {
                        allowed.extend(refs.keys().cloned());
                    }
                    if let Some(reg) = node.get("registerPromptsOnce").and_then(|x| x.as_object()) {
                        for (role, tv) in reg {
                            // 템플릿 값: 리터럴 문자열 | {"$":"values.X"} 참조(조성 완료본 조회).
                            let text: Option<String> = match tv {
                                Json::String(t) => Some(t.clone()),
                                Json::Object(m) => m
                                    .get("$")
                                    .and_then(|p| p.as_str())
                                    .and_then(|p| p.strip_prefix("values."))
                                    .and_then(|name| values.and_then(|vm| vm.get(name)))
                                    .and_then(|x| x.as_str().map(String::from)),
                                _ => None,
                            };
                            if let Some(t) = text {
                                for ph in placeholders(&t) {
                                    if !allowed.contains(&ph) {
                                        v.push(format!(
                                            "[{stage}] registerPromptsOnce.{role} 템플릿 {{{{{ph}}}}} — 소비 시점 치환 불가(vars/varRefs/노드 필드 밖: 허용 {allowed:?})"
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Some("return") => {
                if !op.get("value").is_some_and(|x| x.is_object()) {
                    v.push(format!("[{stage}] return.value 객체 필요"));
                }
            }
            other => v.push(format!("[{stage}] 미지 op {other:?}")),
        }
    }
}

fn ops_contain_agent(ops: &[Json]) -> bool {
    ops.iter().any(|op| {
        op.get("op").and_then(|o| o.as_str()) == Some("agent")
            || op
                .get("do")
                .and_then(|d| d.as_array())
                .is_some_and(|inner| ops_contain_agent(inner))
    })
}

/// placeholders — "{{name}}" 마커 이름 수집.
fn placeholders(t: &str) -> Vec<String> {
    let mut out = vec![];
    let mut rest = t;
    while let Some(open) = rest.find("{{") {
        let after = &rest[open + 2..];
        if let Some(close) = after.find("}}") {
            out.push(after[..close].trim().to_string());
            rest = &after[close + 2..];
        } else {
            break;
        }
    }
    out
}

// ── 실행 ─────────────────────────────────────────────────────

struct Scope<'a> {
    args: Json,
    values: &'a Json,
    locals: Vec<(String, Json)>,
}

impl Scope<'_> {
    /// path("tree.title") 해석 — 첫 세그먼트: locals → "args" → "values" 루트. 미해석 = None.
    fn lookup(&self, path: &str) -> Option<Json> {
        let mut segs = path.split('.');
        let root = segs.next()?;
        let mut cur: Json = if root == "args" {
            self.args.clone()
        } else if root == "values" {
            self.values.clone()
        } else if let Some((_, v)) = self.locals.iter().rev().find(|(k, _)| k == root) {
            v.clone()
        } else {
            return None;
        };
        for s in segs {
            cur = cur.get(s)?.clone();
        }
        Some(cur)
    }

    /// 필드 표현식 평가 — 리터럴 그대로 | {"$":path,"or"?:default} 참조(미해석/null → or, 없으면 Null)
    /// | {"concat":[원소…]} 문자열 조성(원소 = 리터럴|{"$"} — 비문자열 값은 JSON 문자열화).
    /// concat 은 body stage 의 code 노드 표면(코드 전문+PROOF 블록) 실측 RED 가 증명한 확장(§8) —
    /// 결정적 조성을 LLM 에 맡기지 않는다(§7/§11).
    fn eval(&self, v: &Json) -> Json {
        if let Some(m) = v.as_object() {
            if let Some(Json::String(path)) = m.get("$") {
                return match self.lookup(path) {
                    Some(x) if !x.is_null() => x,
                    _ => m.get("or").cloned().unwrap_or(Json::Null),
                };
            }
            if let Some(Json::Array(parts)) = m.get("concat") {
                let mut out = String::new();
                for p in parts {
                    match self.eval(p) {
                        Json::String(s) => out.push_str(&s),
                        Json::Null => {}
                        other => out.push_str(&other.to_string()),
                    }
                }
                return Json::String(out);
            }
        }
        v.clone()
    }

    fn eval_str(&self, v: &Json) -> Option<String> {
        match self.eval(v) {
            Json::String(s) => Some(s),
            Json::Null => None,
            other => Some(other.to_string()),
        }
    }

    /// 프롬프트 템플릿 렌더 — {{name}} → values/args/locals + {{ledger}} 빌트인(원장 렌더).
    /// 미해석 플레이스홀더는 Err(fail-loud — 조용한 빈 프롬프트 금지).
    fn render(&self, tmpl: &str) -> Result<String, String> {
        let mut out = tmpl.to_string();
        for ph in placeholders(tmpl) {
            let rendered = if ph == "ledger" {
                Some(ledger_view(&self.args, "ledger"))
            } else if ph == "facts" {
                Some(ledger_view(&self.args, "facts"))
            } else if ph == "round" {
                Some(
                    self.args
                        .get("round")
                        .map(|v| match v {
                            Json::String(s) => s.clone(),
                            o => o.to_string(),
                        })
                        .unwrap_or_else(|| "1".into()),
                )
            } else if ph == "document" {
                // 확정 규약 — 항목 문서(items with history)를 JSON 정본 그대로. 렌더 없음.
                let empty = Json::Array(vec![]);
                Some(
                    serde_json::to_string_pretty(self.args.get("ledger").unwrap_or(&empty))
                        .unwrap_or_default(),
                )
            } else {
                self.lookup(&ph)
                    .or_else(|| self.lookup(&format!("args.{ph}")))
                    .or_else(|| self.lookup(&format!("values.{ph}")))
                    .map(|x| match x {
                        Json::String(s) => s,
                        other => other.to_string(),
                    })
            };
            match rendered {
                Some(r) => out = out.replace(&format!("{{{{{ph}}}}}"), &r),
                None => return Err(format!("프롬프트 플레이스홀더 {{{{{ph}}}}} 미해석")),
            }
        }
        Ok(out)
    }
}

/// ledger_view — 원장 렌더 빌트인({{ledger}}=args.ledger 요건 원장, {{facts}}=args.facts 기초지식 원장).
/// draft 계약 줄 형식: `- [id] [badge] (category?) title | 근거: verified_value?`.
fn ledger_view(args: &Json, key: &str) -> String {
    let Some(items) = args.get(key).and_then(|l| l.as_array()) else {
        return String::new();
    };
    items
        .iter()
        .map(|t| {
            let g = |k: &str| t.get(k).and_then(|x| x.as_str()).unwrap_or("");
            let badge = {
                let b = g("badge");
                if b.is_empty() {
                    "검수전"
                } else {
                    b
                }
            };
            let cat = g("category");
            let vv = g("verified_value");
            let desc = g("description");
            format!(
                "- [{}] [{}]{} {}{}{}",
                g("id"),
                badge,
                if cat.is_empty() {
                    String::new()
                } else {
                    format!(" ({cat})")
                },
                g("title"),
                if desc.is_empty() {
                    String::new()
                } else {
                    format!(" — {desc}")
                },
                if vv.is_empty() {
                    String::new()
                } else {
                    format!(" | 근거: {vv}")
                }
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// resolve_args — 문서 args 선언({"from":[…],"default"}) 을 런타임 입력으로 해석.
/// from 경로는 입력 args 객체 안 키(우선순위 순회). "$args" 는 입력 전체가 문자열일 때(호환 — 미지원, 객체만).
fn resolve_args(doc: &Json, input: &Json) -> Json {
    let mut out = Map::new();
    // 입력 객체의 키를 전부 통과(ledger/stage/chunkRef/lang 등 런타임 주입 유지).
    if let Some(m) = input.as_object() {
        for (k, v) in m {
            out.insert(k.clone(), v.clone());
        }
    }
    if let Some(decl) = doc.get("args").and_then(|a| a.as_object()) {
        for (name, spec) in decl {
            if out.contains_key(name) {
                continue;
            }
            let mut got: Option<Json> = None;
            if let Some(from) = spec.get("from").and_then(|f| f.as_array()) {
                for cand in from {
                    if let Some(key) = cand.as_str() {
                        if let Some(v) = input.get(key) {
                            if !v.is_null() {
                                got = Some(v.clone());
                                break;
                            }
                        }
                    }
                }
            }
            let v = got
                .or_else(|| spec.get("default").cloned())
                .unwrap_or(Json::Null);
            out.insert(name.clone(), v);
        }
    }
    Json::Object(out)
}

/// run — stage 실행. (발행 NodeEvent 목록, return 값) 반환. stage 미존재 = Err(fail-loud).
pub fn run(
    doc: &Json,
    stage: &str,
    input_args: &Json,
    agent_fn: &mut AgentFn,
) -> Result<(Vec<NodeEvent>, Json), String> {
    validate(doc)
        .map_err(|v| format!("workflow-doc 검증 실패({}건): {}", v.len(), v.join(" / ")))?;
    let ops = doc
        .pointer(&format!("/stages/{}", stage.replace('/', "~1")))
        .and_then(|x| x.as_array())
        .ok_or_else(|| format!("stage {stage:?} 미정의(stages 키 확인)"))?
        .clone();
    let values = Json::Object(resolved_values(doc)?);
    let mut scope = Scope {
        args: resolve_args(doc, input_args),
        values: &values,
        locals: vec![],
    };
    let mut st = RunState {
        events: vec![],
        registered: false,
        result: Json::Null,
    };
    exec_ops(doc, &ops, &mut scope, &mut st, agent_fn, None)?;
    Ok((st.events, st.result))
}

struct RunState {
    events: Vec<NodeEvent>,
    registered: bool, // registerPromptsOnce — 한 run에서 첫 항목에만 부착한다.
    result: Json,
}

/// exec_ops — op 순차 실행. return 을 만나면 true(중단 신호).
fn exec_ops(
    doc: &Json,
    ops: &[Json],
    scope: &mut Scope,
    st: &mut RunState,
    agent_fn: &mut AgentFn,
    for_index: Option<usize>,
) -> Result<bool, String> {
    for op in ops {
        match op.get("op").and_then(|o| o.as_str()) {
            Some("agent") => {
                let pname = op.get("prompt").and_then(|x| x.as_str()).unwrap_or("");
                let tmpl = doc
                    .pointer(&format!("/prompts/{pname}"))
                    .and_then(|x| x.as_str())
                    .ok_or_else(|| format!("prompts.{pname} 미정의"))?;
                let prompt = scope.render(tmpl)?;
                let schema = op
                    .get("schema")
                    .and_then(|x| x.as_str())
                    .and_then(|s| scope.values.get(s))
                    .cloned();
                let label = op.get("label").and_then(|x| x.as_str()).unwrap_or(pname);
                let out = agent_fn(&prompt, schema.as_ref(), label)?;
                let bind = op
                    .get("bind")
                    .and_then(|x| x.as_str())
                    .unwrap_or("_")
                    .to_string();
                scope.locals.push((bind, out));
            }
            Some("forEach") => {
                let path = op.get("in").and_then(|x| x.as_str()).unwrap_or("");
                let items = match scope.lookup(path) {
                    Some(Json::Array(a)) => a,
                    Some(Json::Null) | None => vec![],
                    Some(other) => return Err(format!("forEach.in {path:?} 배열 아님: {other}")),
                };
                let inner = op
                    .get("do")
                    .and_then(|x| x.as_array())
                    .cloned()
                    .unwrap_or_default();
                let collect_name = op.get("collect").and_then(|x| x.as_str());
                let mut collected: Vec<Json> = vec![];
                for (idx, item) in items.into_iter().enumerate() {
                    // when 게이트로 항목을 필터링한다.
                    scope.locals.push(("item".to_string(), item));
                    scope.locals.push(("index".to_string(), Json::from(idx)));
                    let pass = match op.get("when").and_then(|x| x.as_str()) {
                        Some(w) => truthy(&scope.lookup(w).unwrap_or(Json::Null)),
                        None => true,
                    };
                    if pass {
                        let before = st.events.len();
                        let ret = exec_ops(doc, &inner, scope, st, agent_fn, Some(idx))?;
                        for ev in &st.events[before..] {
                            let NodeEvent::Add { id, .. } = ev;
                            collected.push(Json::String(id.clone()));
                        }
                        if ret {
                            scope.locals.pop();
                            scope.locals.pop();
                            return Ok(true);
                        }
                    }
                    scope.locals.pop();
                    scope.locals.pop();
                }
                if let Some(name) = collect_name {
                    scope
                        .locals
                        .push((name.to_string(), Json::Array(collected)));
                }
            }
            Some("publish") => {
                // when/unless 조건부 발행 — 수렴 루프 게이트. 값 = 경로 문자열 | 경로 배열(배열 = OR: 하나라도
                // truthy 면 truthy). when: truthy 면 발행(변경 있으면 재감사). unless: 전부 falsy 면 발행(add·remove
                // 둘 다 0 = 이견 없음 → 다음 스테이지). 둘 다 없으면 무조건. 빈 배열/누락 = falsy.
                let any_truthy = |cond: &Json| -> bool {
                    match cond {
                        Json::Array(ps) => ps.iter().any(|p| {
                            p.as_str()
                                .is_some_and(|s| truthy(&scope.lookup(s).unwrap_or(Json::Null)))
                        }),
                        Json::String(s) => truthy(&scope.lookup(s).unwrap_or(Json::Null)),
                        _ => false,
                    }
                };
                let gated = if let Some(w) = op.get("when") {
                    any_truthy(w)
                } else if let Some(u) = op.get("unless") {
                    !any_truthy(u)
                } else {
                    true
                };
                if gated {
                    let node = op
                        .get("node")
                        .and_then(|x| x.as_object())
                        .ok_or("publish.node 누락")?;
                    let ev = build_event(node, scope, st, for_index)?;
                    st.events.push(ev);
                }
            }
            Some("return") => {
                let spec = op
                    .get("value")
                    .and_then(|x| x.as_object())
                    .ok_or("return.value 누락")?;
                let mut out = Map::new();
                for (k, v) in spec {
                    out.insert(k.clone(), scope.eval(v));
                }
                st.result = Json::Object(out);
                return Ok(true);
            }
            other => return Err(format!("미지 op {other:?}")),
        }
    }
    Ok(false)
}

fn truthy(v: &Json) -> bool {
    match v {
        Json::Null => false,
        Json::Bool(b) => *b,
        Json::String(s) => !s.is_empty(),
        Json::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        // 빈 배열/객체 = falsy(수렴 게이트의 핵심: additions 가 비면 재감사 안 함). 비지 않으면 truthy.
        Json::Array(a) => !a.is_empty(),
        Json::Object(m) => !m.is_empty(),
    }
}

/// build_event — publish.node 스펙 → NodeEvent::Add. 필드 표현식 평가 + id auto(prefix+index) +
/// registerPromptsOnce(이 run 1회 — 첫 부착 시점) 처리. wire 는 interp 경로와 동일(NodeEvent serde).
fn build_event(
    node: &Map<String, Json>,
    scope: &Scope,
    st: &mut RunState,
    for_index: Option<usize>,
) -> Result<NodeEvent, String> {
    let id = match node.get("id") {
        Some(Json::String(s)) => s.clone(),
        Some(Json::Object(m)) => {
            let prefix = m
                .get("auto")
                .and_then(|a| a.as_str())
                .ok_or("id.auto prefix 필요")?;
            let idx = for_index.ok_or("id {\"auto\"} 는 forEach 안에서만")?;
            format!("{prefix}{idx}")
        }
        _ => return Err("publish.node.id 필요".to_string()),
    };
    let s = |k: &str| {
        node.get(k)
            .and_then(|v| scope.eval_str(v))
            .filter(|x| !x.is_empty())
    };
    let kind = s("kind").ok_or("publish.node.kind 필요")?;
    // blockedBy — 원소: 리터럴 문자열 | {"$":path}(배열이면 spread).
    let mut blocked_by: Vec<String> = vec![];
    if let Some(Json::Array(arr)) = node.get("blockedBy") {
        for el in arr {
            match scope.eval(el) {
                Json::String(one) => blocked_by.push(one),
                Json::Array(many) => {
                    for m in many {
                        if let Json::String(x) = m {
                            blocked_by.push(x);
                        }
                    }
                }
                Json::Null => {}
                other => return Err(format!("blockedBy 원소 해석 불가: {other}")),
            }
        }
    }
    // vars — {k: expr} 평가(작은 값만 — 정규화 계약).
    let vars = node.get("vars").and_then(|v| v.as_object()).map(|m| {
        let mut out = Map::new();
        for (k, v) in m {
            out.insert(k.clone(), scope.eval(v));
        }
        Json::Object(out)
    });
    // registerPromptsOnce — 이 run 첫 부착에서만 register_prompts 로 emit(sha dedup 은 kanban 몫).
    let register_prompts = if !st.registered {
        node.get("registerPromptsOnce")
            .and_then(|v| v.as_object())
            .map(|m| {
                st.registered = true;
                let mut out = Map::new();
                for (k, v) in m {
                    out.insert(k.clone(), scope.eval(v));
                }
                Json::Object(out)
            })
    } else {
        None
    };
    // schema — 문자열이면 values 참조(검증 완료), 객체면 인라인.
    let schema = match node.get("schema") {
        Some(Json::String(key)) => scope.values.get(key).cloned(),
        Some(obj @ Json::Object(_)) => Some(obj.clone()),
        _ => None,
    };
    Ok(NodeEvent::Add {
        id,
        parent: s("parent"),
        kind,
        title: s("title").unwrap_or_default(),
        description: s("description").unwrap_or_default(),
        prompt: s("prompt").unwrap_or_default(),
        stage: s("stage"),
        schema,
        category: s("category"),
        origin: s("origin"),
        prompt_role: s("promptRole"),
        vars,
        register_prompts,
        var_refs: node.get("varRefs").cloned().filter(|v| v.is_object()),
        schema_ref: s("schemaRef"),
        blocked_by,
        badge: s("badge"),
        is_draft: node
            .get("isDraft")
            .map(|v| scope.eval(v) == Json::Bool(true))
            .unwrap_or(false),
        parent_draft_id: s("parentDraftId"),
        // 라우팅 tier — s() 가 빈 문자열("or":"" 폴백)을 None 으로 걸러 미emit=기본 최고 보존.
        effort: s("effort"),
        model: s("model"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 최소 draft 형태 doc — 실제 draft 계약의 축소판(스키마·프롬프트 축약).
    fn mini_doc() -> Json {
        json!({
            "spec": SPEC,
            "meta": { "name": "draft", "description": "테스트" },
            "args": {
                "directive": { "from": ["directive", "DIRECTIVE", "IDEA"], "default": "" },
                "parentDraftId": { "from": ["parentDraftId"], "default": null }
            },
            "values": {
                "PENDING": "검수전",
                "COMMON": "SHARED CONCEPTS",
                "VERIFY_TMPL": { "concat": [ { "$": "values.COMMON" }, " verify {{title}} — {{directive}}" ] },
                "GEN_SCHEMA": { "type": "object", "required": ["title", "requirements"] },
                "VERIFY_SCHEMA": { "type": "object", "required": ["oxf", "origin"] },
                "HUNT_SCHEMA": { "type": "object", "required": ["additions"] },
                "AUDIT_SCHEMA": { "type": "object", "required": ["complete", "verdict"] }
            },
            "prompts": {
                "gen": "{{COMMON}}\nGENERATOR\nDirective: \"{{directive}}\"",
                "hunt": "{{COMMON}}\nHUNT\n{{ledger}}\nDirective: \"{{directive}}\"",
                "audit": "{{COMMON}}\nAUDIT\n{{ledger}}"
            },
            "stages": {
                "": [
                    { "op": "publish", "node": { "id": "chunk", "kind": "chunk", "isDraft": true,
                        "title": { "$": "args.title", "or": "구체화 덩어리" }, "description": { "$": "args.directive" },
                        "parentDraftId": { "$": "args.parentDraftId", "or": "" } } },
                    { "op": "publish", "node": { "id": "gen", "kind": "task", "stage": "generate", "parent": "chunk", "title": "요건 도출" } }
                ],
                "generate": [
                    { "op": "agent", "prompt": "gen", "schema": "GEN_SCHEMA", "label": "요건 도출", "bind": "tree" },
                    { "op": "forEach", "in": "tree.requirements", "when": "item.title", "collect": "itemIds", "do": [
                        { "op": "publish", "node": { "id": { "auto": "i" }, "kind": "item", "parent": { "$": "args.chunkRef", "or": "chunk" },
                            "title": { "$": "item.title" }, "description": { "$": "item.description", "or": "" },
                            "origin": { "$": "item.origin" }, "badge": { "$": "values.PENDING" },
                            "effort": { "$": "item.effort", "or": "" }, "model": { "$": "item.model", "or": "" },
                            "schema": "VERIFY_SCHEMA", "promptRole": "verify",
                            "vars": { "title": { "$": "item.title" }, "description": { "$": "item.description", "or": "" } },
                            "varRefs": { "directive": "directive" },
                            "registerPromptsOnce": { "verify": { "$": "values.VERIFY_TMPL" }, "directive": { "$": "args.directive" } } } }
                    ] },
                    { "op": "publish", "node": { "id": "hunt", "kind": "task", "stage": "hunt", "parent": { "$": "args.chunkRef", "or": "chunk" },
                        "title": "누락 탐색", "blockedBy": [ { "$": "itemIds" } ] } },
                    { "op": "return", "value": { "chunkTitle": { "$": "tree.title", "or": "" }, "titleOrigin": { "$": "tree.titleOrigin", "or": "agent" } } }
                ],
                "audit": [
                    { "op": "agent", "prompt": "audit", "schema": "AUDIT_SCHEMA", "bind": "r" },
                    { "op": "return", "value": { "verdict": { "$": "r.verdict", "or": "(감사 결과 없음)" }, "complete": { "$": "r.complete", "or": false } } }
                ]
            }
        })
    }

    fn no_agent(_p: &str, _s: Option<&Json>, _l: &str) -> Result<Json, String> {
        Err("agent 호출 없어야 함".into())
    }

    #[test]
    fn is_doc_detects_spec() {
        assert!(is_doc(&mini_doc()));
        assert!(
            !is_doc(&json!({ "program": {} })),
            "skeleton(AST) 은 doc 아님"
        );
    }

    #[test]
    fn validate_accepts_mini_doc() {
        assert_eq!(validate(&mini_doc()), Ok(()));
    }

    #[test]
    fn validate_rejects_unknown_prompt_and_schema_refs() {
        let mut d = mini_doc();
        d["stages"]["generate"][0]["prompt"] = json!("nope");
        d["stages"]["generate"][0]["schema"] = json!("NOPE_SCHEMA");
        let errs = validate(&d).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.contains("agent.prompt") && e.contains("nope")),
            "{errs:?}"
        );
        assert!(errs.iter().any(|e| e.contains("agent.schema")), "{errs:?}");
    }

    #[test]
    fn validate_rejects_unresolvable_placeholder() {
        let mut d = mini_doc();
        d["prompts"]["gen"] = json!("{{MISSING_VALUE}}");
        let errs = validate(&d).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("MISSING_VALUE")), "{errs:?}");
    }

    #[test]
    fn validate_rejects_agent_in_skeleton_stage() {
        let mut d = mini_doc();
        d["stages"][""] = json!([{ "op": "agent", "prompt": "gen", "bind": "x" }]);
        let errs = validate(&d).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.contains("skeleton") && e.contains("agent")),
            "{errs:?}"
        );
    }

    #[test]
    fn validate_rejects_duplicate_literal_ids() {
        let mut d = mini_doc();
        d["stages"][""][1]["node"]["id"] = json!("chunk"); // gen id 를 chunk 로 중복
        let errs = validate(&d).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("중복")), "{errs:?}");
    }

    #[test]
    fn conditional_publish_gates_convergence_loop() {
        // when/unless = 수렴 게이트. 감사가 additions 를 내면 재감사 발행(when), 비면 다음 스테이지(unless).
        // 이 게이트가 없으면 감사가 매번 진행만 해 편차를 못 죽인다 — 완전성 루프의 종료 조건.
        let doc = json!({
            "spec": SPEC,
            "meta": { "name": "x", "description": "t" },
            "values": { "AUDIT_SCHEMA": { "type": "object", "required": ["additions"] } },
            "prompts": { "audit": "AUDIT" },
            "stages": { "audit": [
                { "op": "agent", "prompt": "audit", "schema": "AUDIT_SCHEMA", "bind": "a" },
                { "op": "publish", "when": "a.additions",
                  "node": { "id": "reaudit", "kind": "task", "stage": "audit", "title": "재감사" } },
                { "op": "publish", "unless": "a.additions",
                  "node": { "id": "proceed", "kind": "task", "stage": "design", "title": "진행" } }
            ] }
        });
        let ids = |ev: &[NodeEvent]| {
            ev.iter()
                .map(|NodeEvent::Add { id, .. }| id.clone())
                .collect::<Vec<_>>()
        };
        // 갭 있음 → 재감사만(진행 게이트 차단).
        let mut with_gap = |_p: &str, _s: Option<&Json>, _l: &str| {
            Ok(json!({ "additions": [ { "title": "누락된 큐 전략" } ] }))
        };
        let (ev, _) = run(&doc, "audit", &json!({}), &mut with_gap).expect("run(gap)");
        assert_eq!(
            ids(&ev),
            vec!["reaudit"],
            "갭 있으면 재감사만 발행, 진행 안 함"
        );
        // 갭 0(빈 배열=falsy) → 진행만(수렴·종료).
        let mut no_gap = |_p: &str, _s: Option<&Json>, _l: &str| Ok(json!({ "additions": [] }));
        let (ev2, _) = run(&doc, "audit", &json!({}), &mut no_gap).expect("run(no gap)");
        assert_eq!(
            ids(&ev2),
            vec!["proceed"],
            "갭 0 이면 진행만 발행, 재감사 안 함(무한루프 종료)"
        );
    }

    #[test]
    fn skeleton_stage_publishes_chunk_and_task_without_agent() {
        let (events, result) = run(
            &mini_doc(),
            "",
            &json!({ "directive": "약국 재고" }),
            &mut no_agent,
        )
        .unwrap();
        assert_eq!(events.len(), 2);
        let NodeEvent::Add {
            id,
            kind,
            is_draft,
            description,
            ..
        } = &events[0];
        assert_eq!(
            (id.as_str(), kind.as_str(), *is_draft),
            ("chunk", "chunk", true)
        );
        assert_eq!(description, "약국 재고", "args.directive → description");
        let NodeEvent::Add {
            id,
            kind,
            stage,
            parent,
            ..
        } = &events[1];
        assert_eq!((id.as_str(), kind.as_str()), ("gen", "task"));
        assert_eq!(stage.as_deref(), Some("generate"));
        assert_eq!(parent.as_deref(), Some("chunk"));
        assert_eq!(result, Json::Null, "skeleton 은 return 없음");
    }

    #[test]
    fn args_from_priority_and_default() {
        // DIRECTIVE(대문자)로 넣어도 args.directive 로 해석(from 우선순위), title 미지정 → or 기본.
        let (events, _) = run(
            &mini_doc(),
            "",
            &json!({ "DIRECTIVE": "대문자 지시" }),
            &mut no_agent,
        )
        .unwrap();
        let NodeEvent::Add {
            title, description, ..
        } = &events[0];
        assert_eq!(description, "대문자 지시");
        assert_eq!(title, "구체화 덩어리", "or 기본값");
    }

    #[test]
    fn generate_publishes_items_tasks_and_returns() {
        let mut agent = |prompt: &str, schema: Option<&Json>, _l: &str| -> Result<Json, String> {
            assert!(prompt.contains("SHARED CONCEPTS"), "{{{{COMMON}}}} 렌더");
            assert!(
                prompt.contains("Directive: \"약국\""),
                "{{{{directive}}}} 렌더"
            );
            assert!(
                schema.is_some_and(|s| s["required"][0] == "title"),
                "GEN_SCHEMA 전달"
            );
            Ok(
                json!({ "title": "약국 재고 SaaS", "titleOrigin": "agent", "requirements": [
                { "title": "재고 차감", "description": "판매 시", "origin": "user" },
                { "title": "", "description": "제목 없음 — when 필터", "origin": "agent" },
                { "title": "유통기한 경고", "origin": "agent" }
            ] }),
            )
        };
        let (events, result) = run(
            &mini_doc(),
            "generate",
            &json!({ "directive": "약국", "chunkRef": "k-7" }),
            &mut agent,
        )
        .unwrap();
        // 항목 2(빈 title 필터) + hunt task 1.
        assert_eq!(events.len(), 3);
        let NodeEvent::Add {
            id,
            parent,
            badge,
            register_prompts,
            vars,
            schema,
            prompt_role,
            ..
        } = &events[0];
        assert_eq!(id, "i0", "auto id = prefix+index");
        assert_eq!(parent.as_deref(), Some("k-7"), "args.chunkRef");
        assert_eq!(badge.as_deref(), Some("검수전"), "values.PENDING");
        assert_eq!(prompt_role.as_deref(), Some("verify"));
        assert!(schema.is_some(), "VERIFY_SCHEMA 인라인");
        let reg = register_prompts
            .as_ref()
            .expect("첫 항목에 registerPromptsOnce");
        assert_eq!(reg["directive"], "약국");
        assert!(
            reg["verify"].as_str().unwrap().contains("{{title}}"),
            "VERIFY_TMPL 은 렌더하지 않음(소비 시점 치환)"
        );
        assert_eq!(vars.as_ref().unwrap()["title"], "재고 차감");
        let NodeEvent::Add {
            id,
            register_prompts,
            ..
        } = &events[1];
        assert_eq!(id, "i2", "필터된 index 도 auto 에 반영(원본 순번 유지)");
        assert!(register_prompts.is_none(), "registerPromptsOnce 는 1회만");
        let NodeEvent::Add { id, blocked_by, .. } = &events[2];
        assert_eq!(id, "hunt");
        assert_eq!(
            blocked_by,
            &vec!["i0".to_string(), "i2".to_string()],
            "collect(itemIds) spread"
        );
        assert_eq!(result["chunkTitle"], "약국 재고 SaaS");
        assert_eq!(result["titleOrigin"], "agent");
    }

    #[test]
    fn audit_renders_ledger_builtin_and_returns() {
        let mut agent = |prompt: &str, _s: Option<&Json>, _l: &str| -> Result<Json, String> {
            assert!(
                prompt.contains("- [i0] [o] 재고 차감"),
                "ledger 렌더: {prompt}"
            );
            assert!(
                prompt.contains("- [i1] [검수전] (재고) 창고 연결 | 근거: 근거텍스트"),
                "badge 폴백·category·근거: {prompt}"
            );
            Ok(json!({ "complete": true, "verdict": "완결" }))
        };
        let args = json!({ "directive": "d", "ledger": [
            { "id": "i0", "title": "재고 차감", "badge": "o" },
            { "id": "i1", "title": "창고 연결", "category": "재고", "verified_value": "근거텍스트" }
        ] });
        let (events, result) = run(&mini_doc(), "audit", &args, &mut agent).unwrap();
        assert!(events.is_empty(), "audit 발행 0");
        assert_eq!(result["verdict"], "완결");
        assert_eq!(result["complete"], true);
    }

    #[test]
    fn agent_failure_propagates() {
        let mut agent = |_p: &str, _s: Option<&Json>, _l: &str| -> Result<Json, String> {
            Err("529 소진".into())
        };
        let err = run(
            &mini_doc(),
            "generate",
            &json!({ "directive": "d" }),
            &mut agent,
        )
        .unwrap_err();
        assert!(
            err.contains("529"),
            "agent 실패 전파(빈-성공 침묵 금지): {err}"
        );
    }

    #[test]
    fn unknown_stage_is_loud() {
        let err = run(&mini_doc(), "classify", &json!({}), &mut no_agent).unwrap_err();
        assert!(err.contains("stage") && err.contains("classify"), "{err}");
    }

    #[test]
    fn foreach_carries_routing_tier_to_wire() {
        // 저작이 실은 난이도 tier(effort/model)가 NodeEvent wire 까지 관통해야 한다 —
        // reconcile 이 그 wire 를 읽어 exec 에 honor. 여기서 끊기면 자기선택 라우팅이 무음 no-op.
        let mut agent = |_p: &str, _s: Option<&Json>, _l: &str| -> Result<Json, String> {
            Ok(
                json!({ "title": "T", "titleOrigin": "agent", "requirements": [
                { "title": "auth 경계", "description": "d", "origin": "agent", "effort": "max", "model": "gpt-5.6-sol" },
                { "title": "날짜 포맷", "description": "d", "origin": "user" }
            ] }),
            )
        };
        let (events, _) = run(
            &mini_doc(),
            "generate",
            &json!({ "directive": "d", "chunkRef": "k" }),
            &mut agent,
        )
        .unwrap();
        let w0 = serde_json::to_string(&events[0]).unwrap();
        assert!(
            w0.contains(r#""effort":"max""#),
            "tier 가 wire 까지 관통: {w0}"
        );
        assert!(
            w0.contains(r#""model":"gpt-5.6-sol""#),
            "model tier 관통: {w0}"
        );
        // tier 미emit 항목 = wire 에 effort/model 키 없음(기본 최고 보존, 군더더기 0).
        let w1 = serde_json::to_string(&events[1]).unwrap();
        assert!(
            !w1.contains("\"effort\""),
            "미지정 = wire 에 effort 생략: {w1}"
        );
        assert!(
            !w1.contains("\"model\""),
            "미지정 = wire 에 model 생략: {w1}"
        );
    }

    /// [계약 스냅샷] gen.pharmacy.doc.json — draft fixture의 wire 계약을 고정한다.
    /// 이 단언이 깨지는 변경은 relay/board와의 wire 계약 변경이다.
    #[test]
    fn fixture_doc_wire_contract_snapshot() {
        let doc: Json =
            serde_json::from_str(include_str!("../fixtures/gen.pharmacy.doc.json")).unwrap();
        assert_eq!(validate(&doc), Ok(()), "fixture doc 은 스키마 검증 통과");
        let values = resolved_values(&doc).unwrap();

        // ── skeleton stage("") — chunk + generate task, 정확한 직렬화 라인(wire) 고정.
        let mut no_agent = |_p: &str, _s: Option<&Json>, _l: &str| -> Result<Json, String> {
            Err("no agent".into())
        };
        let em_args = json!({ "directive": "테스트 지시" });
        let (skel, _) = run(&doc, "", &em_args, &mut no_agent).expect("doc skeleton");
        let lines: Vec<String> = skel
            .iter()
            .map(|e| serde_json::to_string(e).unwrap())
            .collect();
        assert_eq!(
            lines[0],
            r#"{"ev":"add","id":"chunk","parent":null,"kind":"chunk","title":"구체화 덩어리","description":"테스트 지시","is_draft":true}"#,
            "chunk wire"
        );
        assert_eq!(
            lines[1],
            r#"{"ev":"add","id":"gen","parent":"chunk","kind":"task","title":"요건 도출","description":"","stage":"generate"}"#,
            "generate task wire"
        );

        // ── generate stage — stub 산출로 항목/task 발행 계약 고정.
        let stub_json = json!({ "title": "테스트 덩어리", "titleOrigin": "agent", "requirements": [
            { "title": "항목1", "description": "설명1", "origin": "user" },
            { "title": "항목2", "description": "설명2", "origin": "agent" }
        ] });
        let mut prompt_cap = String::new();
        let mut agent = |p: &str, _s: Option<&Json>, _l: &str| -> Result<Json, String> {
            prompt_cap = p.to_string();
            Ok(stub_json.clone())
        };
        let args = json!({ "stage": "generate", "directive": "테스트 지시", "chunkRef": "chunk" });
        let (events, result) = run(&doc, "generate", &args, &mut agent).expect("doc generate");
        // gen 프롬프트 = 렌더된 COMMON + 역할 본문 + directive(정확 조성 — 빈 마커 잔존 0).
        let common = values.get("COMMON").and_then(|v| v.as_str()).unwrap();
        assert!(
            prompt_cap.starts_with(common),
            "genPrompt 는 COMMON 으로 시작(렌더)"
        );
        assert!(
            prompt_cap.contains("Directive: \"테스트 지시\""),
            "directive 렌더"
        );
        assert!(!prompt_cap.contains("{{"), "미해석 마커 잔존 0");
        // 이벤트: 항목 2 + hunt/classify/audit task 3.
        assert_eq!(events.len(), 5);
        let NodeEvent::Add {
            id,
            kind,
            parent,
            badge,
            schema,
            prompt_role,
            vars,
            var_refs,
            register_prompts,
            ..
        } = &events[0];
        assert_eq!(
            (id.as_str(), kind.as_str(), parent.as_deref()),
            ("i0", "item", Some("chunk"))
        );
        assert_eq!(badge.as_deref(), Some("검수전"));
        assert_eq!(
            schema.as_ref(),
            values.get("VERIFY_SCHEMA"),
            "schema = 값 참조(전역 1행)"
        );
        assert_eq!(prompt_role.as_deref(), Some("verify"));
        assert_eq!(
            vars.as_ref().unwrap(),
            &json!({ "description": "설명1", "title": "항목1" }),
            "vars 는 작은 값만"
        );
        assert_eq!(
            var_refs.as_ref().unwrap(),
            &json!({ "directive": "directive" })
        );
        let reg = register_prompts
            .as_ref()
            .expect("첫 항목에 registerPrompts");
        assert_eq!(
            reg.get("verify"),
            values.get("VERIFY_TMPL"),
            "등록 템플릿 = 조성된 VERIFY_TMPL(COMMON 단일 원천)"
        );
        assert_eq!(reg.get("directive"), Some(&json!("테스트 지시")));
        let NodeEvent::Add {
            register_prompts, ..
        } = &events[1];
        assert!(register_prompts.is_none(), "registerPrompts 는 run 당 1회");
        let expected_tasks = [
            ("hunt", vec!["i0", "i1"]),
            ("classify", vec!["i0", "i1", "hunt"]),
            ("audit", vec!["i0", "i1", "hunt", "classify"]),
        ];
        for (idx, (tid, blocked)) in expected_tasks.iter().enumerate() {
            let NodeEvent::Add {
                id,
                kind,
                stage,
                blocked_by,
                ..
            } = &events[2 + idx];
            assert_eq!(
                (id.as_str(), kind.as_str(), stage.as_deref()),
                (*tid, "task", Some(*tid))
            );
            assert_eq!(
                blocked_by,
                &blocked.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                "{tid} blockedBy 사슬"
            );
        }
        assert_eq!(
            result,
            json!({ "chunkTitle": "테스트 덩어리", "titleOrigin": "agent" }),
            "return 계약"
        );
    }

    /// [번들 정본] workflows/research.doc.json — research/plan stage 가 계약대로 실행되는지(stub agent).
    #[test]
    fn bundled_research_doc_validates_and_runs_research_and_plan() {
        let doc: Json =
            serde_json::from_str(include_str!("../workflows/research.doc.json")).unwrap();
        assert_eq!(
            validate(&doc),
            Ok(()),
            "번들 research doc 은 스키마 검증 통과"
        );

        // research stage — fact 발행(정규화·registerPromptsOnce 1회·area→category) + plan task(blockedBy=factIds).
        let mut agent = |prompt: &str, schema: Option<&Json>, _l: &str| -> Result<Json, String> {
            assert!(prompt.contains("RESEARCHER"), "research 역할 프롬프트");
            assert!(
                prompt.contains("- [i0] [o] 요건A"),
                "{{{{ledger}}}} 렌더(인증 원장)"
            );
            assert!(
                prompt.contains("Directive: \"정련 지시\""),
                "{{{{directive}}}} 렌더"
            );
            assert!(
                schema.is_some_and(|s| s["required"][0] == "facts"),
                "RESEARCH_SCHEMA 전달"
            );
            Ok(json!({ "facts": [
                { "title": "저장소: SQLite 채택", "description": "동시성 요건이 단일 노드 — [i0] 근거", "origin": "agent", "area": "framework" },
                { "title": "마약류 보고 기한 준수", "description": "재고 불일치 시 기한 내 보고", "origin": "search", "area": "directive" }
            ] }))
        };
        let args = json!({ "stage": "research", "directive": "정련 지시", "chunkRef": "K-7",
            "ledger": [{ "id": "i0", "title": "요건A", "badge": "o" }] });
        let (events, _r) = run(&doc, "research", &args, &mut agent).expect("research 실행");
        assert_eq!(events.len(), 3, "fact 2 + plan task 1");
        let NodeEvent::Add {
            id,
            kind,
            parent,
            badge,
            category,
            prompt_role,
            register_prompts,
            var_refs,
            ..
        } = &events[0];
        assert_eq!((id.as_str(), kind.as_str()), ("fact0", "fact"));
        assert_eq!(
            parent.as_deref(),
            Some("K-7"),
            "args.chunkRef(기존 칸반 id) 직속"
        );
        assert_eq!(
            badge.as_deref(),
            Some("검수전"),
            "fact 는 draft 항목과 같은 검증 파이프"
        );
        assert_eq!(category.as_deref(), Some("framework"), "area → category");
        assert_eq!(prompt_role.as_deref(), Some("fact-verify"));
        assert!(
            register_prompts.is_some(),
            "첫 fact 에 registerPromptsOnce(fact-verify+directive)"
        );
        assert!(var_refs.is_some(), "directive 콘텐츠 주소 참조");
        let NodeEvent::Add {
            register_prompts, ..
        } = &events[1];
        assert!(register_prompts.is_none(), "registerPromptsOnce 는 1회만");
        let NodeEvent::Add {
            id,
            kind,
            stage,
            blocked_by,
            ..
        } = &events[2];
        assert_eq!(
            (id.as_str(), kind.as_str()),
            ("research-audit", "task"),
            "research 다음 = 완전성 감사(누락 ground 게이트)"
        );
        assert_eq!(
            stage.as_deref(),
            Some("research-audit"),
            "design 앞에 완전성 감사 — 누락 ground 를 사냥·완결 후 design 진입"
        );
        assert_eq!(
            blocked_by,
            &vec!["fact0".to_string(), "fact1".to_string()],
            "완전성 감사는 fact 전부 검증 후"
        );

        // design 체인(M-B, 대회 실측 채택) — 각 스테이지: fact 발행 + 다음 task(blockedBy=자기 factIds).
        // 뒤 스테이지는 앞 산출을 {{facts}} 원장으로 계승(칸반 materializeFacts 가 전 fact 를 주입).
        let mut design_agent = |prompt: &str,
                                schema: Option<&Json>,
                                _l: &str|
         -> Result<Json, String> {
            assert!(
                prompt.contains("SHARED CONCEPTS (design)"),
                "DESIGN_COMMON 렌더(정본 concat)"
            );
            assert!(prompt.contains("- [i0] [o] 요건A"), "{{{{ledger}}}} 렌더");
            assert!(
                prompt.contains("- [fact0] [o] (framework) 저장소: SQLite 채택"),
                "{{{{facts}}}} 렌더(고정 ground)"
            );
            assert!(!prompt.contains("{{"), "미해석 마커 잔존 0");
            assert!(
                schema.is_some_and(|s| s["required"][0] == "facts"),
                "DESIGN_SCHEMA 전달"
            );
            Ok(json!({ "facts": [
                { "title": "재고 차감 API 계약", "description": "```ts\ninterface Deduct { ... }\n``` — realizes [i0]", "origin": "agent", "category": "interface" }
            ] }))
        };
        let design_args = json!({ "stage": "design-interface", "directive": "정련 지시", "chunkRef": "K-7",
            "ledger": [{ "id": "i0", "title": "요건A", "badge": "o" }],
            "facts": [{ "id": "fact0", "title": "저장소: SQLite 채택", "badge": "o", "category": "framework" }] });
        let (dev, _dr) = run(&doc, "design-interface", &design_args, &mut design_agent)
            .expect("design-interface 실행");
        assert_eq!(dev.len(), 2, "interface fact 1 + design-domain task 1");
        let NodeEvent::Add {
            id,
            kind,
            badge,
            category,
            prompt_role,
            register_prompts,
            ..
        } = &dev[0];
        assert_eq!(kind.as_str(), "fact");
        assert!(id.starts_with("design-interface"), "auto id prefix: {id}");
        // 적재적소: design fact 는 검증된 ground(research fact)의 투영(파생물) — 개별 반박 생략, set-레벨
        // design-audit 가 완전성·정합 담당. badge=o(수용), verify 미부착. 원본 주장(research fact)만 개별 반박.
        assert_eq!(
            badge.as_deref(),
            Some("o"),
            "design fact = 파생물 → 개별 반박 생략(badge o), design-audit 가 커버"
        );
        assert_eq!(category.as_deref(), Some("interface"));
        assert!(
            prompt_role.is_none(),
            "design fact 는 개별 verify 안 함(파생물)"
        );
        assert!(
            register_prompts.is_none(),
            "design fact 는 verify 템플릿 미등록"
        );
        let NodeEvent::Add {
            id,
            kind,
            stage,
            blocked_by,
            ..
        } = &dev[1];
        assert_eq!(
            (id.as_str(), kind.as_str(), stage.as_deref()),
            ("design-domain", "task", Some("design-domain"))
        );
        assert_eq!(blocked_by.len(), 1, "domain 은 interface fact 전부 검증 후");

        // 체인 말미(criteria) — plan task 로 이어진다 + 처방(1:1 커버리지) 문구가 프롬프트에 실재.
        let mut crit_agent = |prompt: &str, _s: Option<&Json>, _l: &str| -> Result<Json, String> {
            assert!(
                prompt.contains("COVERAGE IS A CONTRACT"),
                "커버리지 처방(대회 실측 교정) 렌더"
            );
            Ok(json!({ "facts": [
                { "title": "차감 판정", "description": "관측: 차감 후 잔량 일치 — proves [i0]. SURFACE pure-logic, PROOF unit", "origin": "agent", "category": "criterion" }
            ] }))
        };
        let (cev, _cr) = run(&doc, "design-criteria", &design_args, &mut crit_agent)
            .expect("design-criteria 실행");
        assert_eq!(cev.len(), 2, "criterion fact 1 + design-audit task 1");
        let NodeEvent::Add { id, stage, .. } = &cev[1];
        assert_eq!(
            (id.as_str(), stage.as_deref()),
            ("design-audit", Some("design-audit")),
            "design 뒤 보완/감사(수렴 시 → plan)"
        );

        // plan stage — 요건 원장+fact 원장 렌더 → plan-unit 발행.
        let mut plan_agent = |prompt: &str,
                              schema: Option<&Json>,
                              _l: &str|
         -> Result<Json, String> {
            assert!(prompt.contains("PLANNER"), "plan 역할 프롬프트");
            assert!(prompt.contains("- [i0] [o] 요건A"), "{{{{ledger}}}} 렌더");
            assert!(
                prompt.contains("- [fact0] [o] (framework) 저장소: SQLite 채택"),
                "{{{{facts}}}} 렌더: {prompt}"
            );
            assert!(
                schema.is_some_and(|s| s["required"][0] == "units"),
                "PLAN_SCHEMA 전달"
            );
            Ok(
                json!({ "units": [ { "title": "재고 차감 구현", "file_path": "src/inventory/deduct.ts", "pseudocode": "impl deduct([i0], [fact0])\nacceptance: 차감 후 잔량 일치", "implements": ["i0"] } ] }),
            )
        };
        let plan_args = json!({ "stage": "plan", "directive": "정련 지시", "chunkRef": "K-7",
            "ledger": [{ "id": "i0", "title": "요건A", "badge": "o" }],
            "facts": [{ "id": "fact0", "title": "저장소: SQLite 채택", "badge": "o", "category": "framework" }] });
        let (pev, _pr) = run(&doc, "plan", &plan_args, &mut plan_agent).expect("plan 실행");
        assert_eq!(pev.len(), 1);
        let NodeEvent::Add {
            id,
            kind,
            parent,
            title,
            description,
            badge,
            category,
            prompt_role,
            ..
        } = &pev[0];
        assert_eq!((id.as_str(), kind.as_str()), ("unit0", "plan-unit"));
        assert_eq!(parent.as_deref(), Some("K-7"));
        assert_eq!(title, "재고 차감 구현");
        assert!(
            description.contains("acceptance"),
            "슈도코드 전문(검증 방법 포함) = description"
        );
        // 적재적소: plan-unit 은 검증된 요건+fact 의 구현 투영(파생물) — 개별 반박 생략, set-레벨 plan-audit 가
        // 파일셋 완전성·정합 담당. badge=o, verify 미부착.
        assert_eq!(
            badge.as_deref(),
            Some("o"),
            "plan-unit = 파생물 → 개별 반박 생략(badge o), plan-audit 가 커버"
        );
        assert_eq!(
            category.as_deref(),
            Some("src/inventory/deduct.ts"),
            "category = file_path(원장에 파일경로 렌더)"
        );
        assert!(
            prompt_role.is_none(),
            "plan-unit 은 개별 verify 안 함(파생물)"
        );

        // body stage — 유닛 1개 실코드화 → code 노드(코드 전문+PROOF 블록 한 표면, badge 검증 파이프).
        let mut body_agent = |prompt: &str,
                              schema: Option<&Json>,
                              _l: &str|
         -> Result<Json, String> {
            assert!(
                prompt.contains("File: src/inventory/deduct.ts")
                    || prompt.contains("src/inventory/deduct.ts"),
                "file_path 주입: {prompt}"
            );
            assert!(prompt.contains("impl deduct"), "pseudocode 주입");
            assert!(!prompt.contains("{{"), "미해석 마커 잔존 0");
            assert!(
                schema.is_some_and(|s| s.get("required").is_some()),
                "BODY_SCHEMA 전달"
            );
            Ok(json!({
                "status": "ok",
                "source": { "content": "export function deduct(q: number) { return q - 1; }" },
                "proof": { "commands": ["npx tsc --noEmit", "node --test deduct.test.ts"], "pass_condition": "exit 0" },
                "implements": ["i0"]
            }))
        };
        let body_args = json!({ "stage": "body", "directive": "정련 지시", "chunkRef": "K-7",
            "title": "재고 차감 구현", "file_path": "src/inventory/deduct.ts",
            "pseudocode": "impl deduct([i0], [fact0])\nacceptance: 차감 후 잔량 일치" });
        let (bev, br) = run(&doc, "body", &body_args, &mut body_agent).expect("body 실행");
        assert_eq!(bev.len(), 1);
        let NodeEvent::Add {
            id,
            kind,
            title,
            description,
            badge,
            category,
            prompt_role,
            ..
        } = &bev[0];
        assert_eq!((id.as_str(), kind.as_str()), ("code", "code"));
        assert_eq!(
            title, "src/inventory/deduct.ts",
            "code 노드 title = 파일경로"
        );
        assert!(
            description.contains("export function deduct"),
            "코드 전문: {description}"
        );
        assert!(description.contains("---- PROOF ----"), "PROOF 블록 병기");
        assert!(
            description.contains("npx tsc --noEmit"),
            "proof commands 렌더"
        );
        assert_eq!(
            badge.as_deref(),
            Some("검수전"),
            "code 도 badge 검증 파이프(body-verify)"
        );
        assert_eq!(category.as_deref(), Some("src/inventory/deduct.ts"));
        assert_eq!(prompt_role.as_deref(), Some("body-verify"));
        assert_eq!(br, json!({ "file": "src/inventory/deduct.ts" }));
    }

    /// [번들 정본] 확정 문서 규약 — review 는 changes[] 를 낸다. changes 비면 다음 스테이지(수렴), 있으면
    /// 자기재발행. {{document}} 로 items(history 포함) JSON 을 읽는다. draft-review·research-audit·design-audit 동형.
    #[test]
    fn bundled_review_loop_changes_model() {
        let stages = |ev: &[NodeEvent]| {
            ev.iter()
                .filter_map(|NodeEvent::Add { stage, .. }| stage.clone())
                .collect::<Vec<_>>()
        };
        let research: Json =
            serde_json::from_str(include_str!("../workflows/research.doc.json")).unwrap();
        let draft: Json =
            serde_json::from_str(include_str!("../workflows/draft.doc.json")).unwrap();
        for (doc, stage, onconv) in [
            (&research, "research-audit", "design-interface"),
            (&research, "design-audit", "plan"),
            (&draft, "draft-review", "classify"),
        ] {
            let args = json!({ "directive": "d", "chunkRef": "chunk",
                "ledger": [{ "id": "x0", "state": "o", "title": "t", "description": "", "history": [] }] });
            // changes 비면 → onConverge 수렴, 자기재발행 없음. {{document}} 가 items JSON 렌더.
            let mut empty = |p: &str, _s: Option<&Json>, _l: &str| {
                assert!(
                    p.contains("CONSENSUS REVIEWER"),
                    "{stage}: reviewer 프롬프트"
                );
                assert!(
                    p.contains("\"x0\""),
                    "{stage}: document 가 items JSON 렌더: {p}"
                );
                Ok(json!({ "changes": [] }))
            };
            let (e, ret) = run(doc, stage, &args, &mut empty).unwrap();
            assert!(
                stages(&e).iter().any(|x| x == onconv),
                "{stage}: changes 0 → {onconv} 수렴: {:?}",
                stages(&e)
            );
            assert!(
                !stages(&e).iter().any(|x| x == stage),
                "{stage}: 수렴이면 자기재발행 없음"
            );
            assert!(
                ret.get("changes").and_then(|c| c.as_array()).is_some(),
                "{stage}: changes return"
            );
            // changes 있으면 → 자기재발행, 진행 금지.
            let mut ch = |_p: &str, _s: Option<&Json>, _l: &str| {
                Ok(json!({ "changes": [{ "op": "add", "title": "g", "reason": "r" }] }))
            };
            let (e2, _) = run(doc, stage, &args, &mut ch).unwrap();
            assert!(
                stages(&e2).iter().any(|x| x == stage),
                "{stage}: changes 있으면 자기재발행: {:?}",
                stages(&e2)
            );
            assert!(
                !stages(&e2).iter().any(|x| x == onconv),
                "{stage}: 이견 있으면 진행 금지"
            );
        }
    }

    /// [번들 정본] 번들 스키마가 Ajv strict(claude --json-schema)를 통과 — 스키마 객체 최상위에 미지 키가
    /// 없어야 한다. properties 밖에 프로퍼티를 두면(예: removals 오배치) 사이드카 검증은 통과하나 claude 가
    /// 런타임에 거부(exit 1). "테스트 그린≠런타임 정확" 방어.
    #[test]
    fn bundled_schemas_have_no_stray_top_level_keys() {
        use std::collections::HashSet;
        let known: HashSet<&str> = [
            "type",
            "properties",
            "required",
            "items",
            "enum",
            "additionalProperties",
            "description",
            "title",
            "anyOf",
            "oneOf",
            "allOf",
            "not",
            "format",
            "minimum",
            "maximum",
            "minItems",
            "maxItems",
            "minLength",
            "maxLength",
            "pattern",
            "default",
            "const",
            "$schema",
        ]
        .into_iter()
        .collect();
        fn check(s: &Json, name: &str, known: &HashSet<&str>) {
            let Some(obj) = s.as_object() else { return };
            for (k, v) in obj {
                match k.as_str() {
                    "properties" => {
                        if let Some(p) = v.as_object() {
                            for (_pk, pv) in p {
                                check(pv, name, known);
                            }
                        }
                    }
                    "items" => check(v, name, known),
                    "anyOf" | "oneOf" | "allOf" => {
                        if let Some(a) = v.as_array() {
                            for e in a {
                                check(e, name, known);
                            }
                        }
                    }
                    other => assert!(known.contains(other), "{name}: 스키마 최상위 미지 키 {other:?} — Ajv strict 거부(properties 밖 프로퍼티?)"),
                }
            }
        }
        for (label, src) in [
            ("research", include_str!("../workflows/research.doc.json")),
            ("draft", include_str!("../workflows/draft.doc.json")),
        ] {
            let doc: Json = serde_json::from_str(src).unwrap();
            if let Some(values) = doc.get("values").and_then(|v| v.as_object()) {
                for (vk, vv) in values {
                    if vv.get("type").and_then(|t| t.as_str()) == Some("object")
                        || vv.get("properties").is_some()
                    {
                        check(vv, &format!("{label}/{vk}"), &known);
                    }
                }
            }
        }
    }

    /// [번들 정본] plan-audit 도 remove 통과(4번째 완전성 지점). return {complete,gaps,verdict} 였던 것에
    /// removals 추가 — reconcile 이 대상 unit badge→x.
    #[test]
    fn bundled_plan_audit_returns_removals() {
        let doc: Json =
            serde_json::from_str(include_str!("../workflows/research.doc.json")).unwrap();
        let args = json!({ "directive": "d", "chunkRef": "K-7", "ledger": [{ "id": "u1", "title": "t", "badge": "o" }] });
        let mut agent = |_p: &str, _s: Option<&Json>, _l: &str| {
            Ok(json!({
                "complete": true, "verdict": "1 유닛 제거", "gaps": [],
                "removals": [{ "id": "u1", "reason": "지시서 범위밖 유닛 — 자기교정" }]
            }))
        };
        let (_ev, ret) = run(&doc, "plan-audit", &args, &mut agent).expect("plan-audit");
        let removals = ret
            .get("removals")
            .and_then(|r| r.as_array())
            .expect("plan-audit removals 통과");
        assert_eq!(removals.len(), 1);
        assert_eq!(removals[0]["id"], "u1");
        assert_eq!(removals[0]["reason"], "지시서 범위밖 유닛 — 자기교정");
    }

    /// [번들 정본] workflows/draft.doc.json — 정련 주입(inject_refinement) 후 유효 doc 이 되는지.
    /// 저작 LLM 은 {directive, description}만 내고 상수는 이 템플릿이 정본(재타이핑 0 — PRINCIPLES §7).
    #[test]
    fn bundled_draft_template_accepts_refinement_injection() {
        let tpl: Json = serde_json::from_str(include_str!("../workflows/draft.doc.json")).unwrap();
        assert_eq!(validate(&tpl), Ok(()), "번들 draft 템플릿 자체가 유효 doc");
        let doc = inject_refinement(&tpl, "정련된 지시어 전문", "약국 재고 SaaS 백로그");
        assert_eq!(
            doc.pointer("/args/directive/default")
                .and_then(|v| v.as_str()),
            Some("정련된 지시어 전문")
        );
        assert_eq!(
            doc.pointer("/meta/description").and_then(|v| v.as_str()),
            Some("약국 재고 SaaS 백로그")
        );
        assert_eq!(validate(&doc), Ok(()), "주입 후에도 유효");
        // 주입된 정련본이 skeleton 발행의 chunk description 으로 흐른다(단일 진실 §1).
        let mut no_agent = |_p: &str, _s: Option<&Json>, _l: &str| -> Result<Json, String> {
            Err("no agent".into())
        };
        let (events, _) = run(&doc, "", &json!({}), &mut no_agent).unwrap();
        let NodeEvent::Add { description, .. } = &events[0];
        assert_eq!(description, "정련된 지시어 전문");
        // description 빈 문자열이면 meta 는 템플릿 기본 유지.
        let doc2 = inject_refinement(&tpl, "d", "");
        assert_eq!(
            doc2.pointer("/meta/description"),
            tpl.pointer("/meta/description")
        );
    }

    #[test]
    fn concat_value_composes_from_plain_values() {
        let d = json!({ "spec": SPEC, "meta": {"name":"x","description":""},
            "values": { "A": "머리", "T": { "concat": [ {"$":"values.A"}, "-꼬리 {{title}}" ] } },
            "stages": { "": [ {"op":"publish","node":{"id":"n","kind":"chunk","title":{"$":"values.T"}}} ] } });
        let mut no_agent =
            |_p: &str, _s: Option<&Json>, _l: &str| -> Result<Json, String> { Err("x".into()) };
        let (events, _) = run(&d, "", &json!({}), &mut no_agent).unwrap();
        let NodeEvent::Add { title, .. } = &events[0];
        assert_eq!(title, "머리-꼬리 {{title}}", "조성 + 소비 시점 마커 보존");
    }

    #[test]
    fn events_serialize_same_wire_as_interp_path() {
        // relay(서비스 relay) 가 파싱하는 wire — {"ev":"add", snake_case 필드}. interp 경로와 동일 serde.
        let (events, _) =
            run(&mini_doc(), "", &json!({ "directive": "d" }), &mut no_agent).unwrap();
        let line = serde_json::to_string(&events[0]).unwrap();
        assert!(line.contains("\"ev\":\"add\""), "{line}");
        assert!(line.contains("\"is_draft\":true"), "{line}");
        assert!(
            !line.contains("\"prompt\""),
            "빈 prompt 직렬화 생략(군더더기 0): {line}"
        );
    }
}
