//! 인메모리 보드 Deps + DRAFT 합의 루프 드라이버.
//!
//! 앱/보드 없이 전체 DRAFT 흐름을 in-process 로 끝까지(변경 0 수렴) 돌린다. DRAFT 는 단 하나의
//! 자기-루프 op(draft-review) — round-1 은 빈 집합이라 그 라운드가 곧 최초 생성이고, 이후 라운드가
//! add/remove 를 누적하다 변경 0 에서 수렴(=인증)한다. reconcile 로직은 재구현하지 않는다 —
//! `reconcile_tick`/`consume_stage_output` 을 그대로 재사용하고, Deps 만 인메모리 상태로 채운다.
//!
//! exec seam(exec_one/exec_stage)만 갈아끼운다: CLI 는 실 provider(claude -p), 테스트는 결정적 stub.
//! 그래서 이 모듈은 exec 백엔드에 대해 제네릭이고 LLM 을 직접 알지 못한다.

use crate::reconcile::{
    build_ledger, reconcile_tick, Deps, EditResult, Node, ReconcileState, StageOut,
};
use serde_json::{json, Map, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

/// exec 경계 — reconcile 이 부르는 노드/스테이지 실행. CLI 는 실 in-process provider 를,
/// 테스트는 결정적 stub 를 배선한다. Err=throw(멱등: 노드 미변경).
pub trait Exec {
    fn exec_one(&self, body: &str) -> Result<Value, String>;
    fn exec_stage(&self, body: &str) -> Result<StageOut, String>;
}

/// 보드 상태 — kanban 이 소유하던 것을 HashMap/Vec 로 든다.
#[derive(Default)]
struct Inner {
    nodes: Vec<Node>,
    /// node.add id 배정 카운터(kanban genId 대응).
    seq: u64,
    /// 콘텐츠 주소 prompt 저장(put_prompt/get_prompt/resolve_prompt).
    prompts: HashMap<String, Value>,
    /// poke 횟수(진행 신호 — 스케줄러 없음, 계수만).
    pokes: u32,
}

/// 인메모리 보드 — Deps 를 HashMap 상태 + 주입된 exec 로 구현한다.
pub struct MemBoard<E: Exec> {
    inner: RefCell<Inner>,
    exec: E,
}

impl<E: Exec> MemBoard<E> {
    pub fn new(exec: E) -> Self {
        MemBoard {
            inner: RefCell::new(Inner::default()),
            exec,
        }
    }

    /// 현재 노드 스냅샷.
    pub fn nodes(&self) -> Vec<Node> {
        self.inner.borrow().nodes.clone()
    }

    /// id 로 노드 스냅샷.
    pub fn node(&self, id: &str) -> Option<Node> {
        self.inner
            .borrow()
            .nodes
            .iter()
            .find(|n| n.id == id)
            .cloned()
    }

    pub fn pokes(&self) -> u32 {
        self.inner.borrow().pokes
    }

    /// 주입된 exec — 테스트가 호출 관측(격리검증 0회 단언)에 쓴다.
    pub fn exec(&self) -> &E {
        &self.exec
    }
}

/// canon 문자열의 콘텐츠 주소 — 결정적 hex 다이제스트. sha2 의존 없이 put↔get 일관만 보장한다(동일
/// 콘텐츠 = 동일 주소로 dedup). 프로세스 내 안정: DefaultHasher 는 고정 키(0,0)라 재현 가능하다.
fn content_address(canon: &str) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    canon.hash(&mut h);
    format!("mem-{:016x}", h.finish())
}

/// {{key}} 마커를 vars 로 치환(kanban prompt.resolve bindVars 대응). 미정의 키는 마커 보존.
/// \w = ascii [A-Za-z0-9_] 만 키로 인정한다.
fn bind_vars(tmpl: &str, vars: &Map<String, Value>) -> String {
    let mut out = String::with_capacity(tmpl.len());
    let mut rest = tmpl;
    while let Some(pos) = rest.find("{{") {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 2..];
        if let Some(close) = after.find("}}") {
            let key = &after[..close];
            if !key.is_empty() && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                match vars.get(key) {
                    Some(Value::String(s)) => out.push_str(s),
                    Some(v) => out.push_str(&v.to_string()),
                    None => {
                        out.push_str("{{");
                        out.push_str(key);
                        out.push_str("}}");
                    }
                }
                rest = &after[close + 2..];
                continue;
            }
        }
        // 닫힘 없음/비-키 — "{{" 를 리터럴로 흘리고 진행한다.
        out.push_str("{{");
        rest = after;
    }
    out.push_str(rest);
    out
}

// node.edit 필드 병합 — 존재하는 필드만 덮어쓴다(kanban node.edit 의미론).
fn apply_str(field: &mut Option<String>, fields: &Value, key: &str) {
    if let Some(s) = fields.get(key).and_then(|v| v.as_str()) {
        *field = Some(s.to_string());
    }
}

impl<E: Exec> Deps for MemBoard<E> {
    fn list_nodes(&self) -> Vec<Node> {
        self.inner.borrow().nodes.clone()
    }

    fn get_node(&self, id: &str) -> Option<Node> {
        self.inner
            .borrow()
            .nodes
            .iter()
            .find(|n| n.id == id)
            .cloned()
    }

    fn edit_node(&self, id: &str, fields: Value) -> EditResult {
        let mut inner = self.inner.borrow_mut();
        let Some(node) = inner.nodes.iter_mut().find(|n| n.id == id) else {
            return EditResult::err(format!("node.edit: 미발견 {id}"));
        };
        apply_str(&mut node.title, &fields, "title");
        apply_str(&mut node.description, &fields, "description");
        apply_str(&mut node.body, &fields, "body");
        apply_str(&mut node.status, &fields, "status");
        apply_str(&mut node.badge, &fields, "badge");
        apply_str(&mut node.result, &fields, "result");
        apply_str(&mut node.category, &fields, "category");
        apply_str(&mut node.kind, &fields, "kind");
        if let Some(arr) = fields.get("blockedBy").and_then(|v| v.as_array()) {
            node.blocked_by = arr
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
        }
        EditResult::ok()
    }

    fn add_node(&self, params: Value) -> Option<String> {
        let mut inner = self.inner.borrow_mut();
        inner.seq += 1;
        let id = format!("k-{}", inner.seq);
        // params(camelCase parentId/blockedBy)에 id 를 스탬핑해 Node 로 역직렬화한다. Node 는
        // rename/alias 로 parentId/blockedBy 를 받고, 모르는 필드(type/locked/collapsed/isDraft…)는 버린다.
        let mut v = params;
        if let Some(obj) = v.as_object_mut() {
            obj.insert("id".into(), json!(id));
        } else {
            return None;
        }
        match serde_json::from_value::<Node>(v) {
            Ok(node) => {
                inner.nodes.push(node);
                Some(id)
            }
            Err(_) => None,
        }
    }

    fn poke(&self) {
        self.inner.borrow_mut().pokes += 1;
    }

    fn exec_one(&self, body: &str) -> Result<Value, String> {
        self.exec.exec_one(body)
    }

    fn exec_stage(&self, body: &str) -> Result<StageOut, String> {
        self.exec.exec_stage(body)
    }

    fn materialize_ledger(&self, chunk_id: &str) -> Result<Vec<Value>, String> {
        Ok(build_ledger(&self.list_nodes(), chunk_id, "item"))
    }

    fn materialize_facts(&self, chunk_id: &str) -> Result<Vec<Value>, String> {
        Ok(build_ledger(&self.list_nodes(), chunk_id, "fact"))
    }

    fn put_prompt(&self, value: Value) -> Option<String> {
        // canon = 문자열은 그대로, 그 밖은 JSON 직렬화(kanban sha256 canon 대응).
        let canon = match &value {
            Value::String(s) => s.clone(),
            v => v.to_string(),
        };
        if canon.is_empty() {
            return None;
        }
        let hash = content_address(&canon);
        self.inner.borrow_mut().prompts.insert(hash.clone(), value);
        Some(hash)
    }

    fn resolve_prompt(&self, hash: &str, vars: Value, refs: Value) -> Option<Value> {
        let inner = self.inner.borrow();
        let tmpl = match inner.prompts.get(hash) {
            Some(Value::String(s)) => s.clone(),
            _ => return None,
        };
        let mut bound: Map<String, Value> = match vars {
            Value::Object(m) => m,
            _ => Map::new(),
        };
        // refs: {{key}} → 콘텐츠 주소 deref(큰 공유값을 소비 시점에 펼친다).
        if let Value::Object(rmap) = refs {
            for (k, h) in rmap {
                let Some(hstr) = h.as_str() else { continue };
                match inner.prompts.get(hstr) {
                    Some(Value::String(t)) => {
                        bound.insert(k, json!(t));
                    }
                    Some(v) => {
                        bound.insert(k, v.clone());
                    }
                    None => return None,
                }
            }
        }
        Some(json!({ "prompt": bind_vars(&tmpl, &bound) }))
    }

    fn get_prompt(&self, hash: &str) -> Option<Value> {
        // resolve_body 는 sr.get("value").unwrap_or(sr) 로 언랩하므로 { value } 봉투로 답한다.
        self.inner
            .borrow()
            .prompts
            .get(hash)
            .map(|v| json!({ "value": v }))
    }
}

// ── 드라이버 ─────────────────────────────────────────────────────

/// 한 드라이브 결과 — 몇 틱, draft-review 몇 라운드, 수렴/봉인 여부, chunk 최종 badge, 틱 로그.
#[derive(Debug, Default)]
pub struct DriveReport {
    /// 처리한(processed≥1 또는 idempotent) 틱 수.
    pub ticks: usize,
    /// draft-review 라운드 수(합의 task 노드 개수 = 재발행 누적).
    pub draft_review_rounds: usize,
    /// 변경 0 수렴으로 chunk badge=o 인증에 도달했는가.
    pub converged: bool,
    /// round 상한 도달로 봉인(chunk badge=f)됐는가.
    pub sealed: bool,
    /// chunk 노드 최종 badge.
    pub final_chunk_badge: Option<String>,
    /// 발행된 item 프레임 수.
    pub items: usize,
    /// 틱별 전이 로그(스테이지·라운드·badge).
    pub log: Vec<String>,
    /// 상한/에러로 중단됐으면 사유.
    pub aborted: Option<String>,
}

impl DriveReport {
    /// 요약 한 줄(+중단 사유). 로그를 라이브로 흘렸으므로 완주 시점엔 이것만 찍는다.
    pub fn summary(&self) -> String {
        let mut s = format!(
            "── 요약: {} 틱, item {}개, draft-review {} 라운드, chunk badge={}, {}\n",
            self.ticks,
            self.items,
            self.draft_review_rounds,
            self.final_chunk_badge.as_deref().unwrap_or("(없음)"),
            if self.sealed {
                "봉인(미수렴)"
            } else if self.converged {
                "수렴(인증)"
            } else {
                "미완"
            },
        );
        if let Some(a) = &self.aborted {
            s.push_str(&format!("── 중단: {a}\n"));
        }
        s
    }

    /// 전체 로그 + 요약(테스트·사후 재구성용). 라이브 관전은 emit 스트림으로 본다.
    pub fn render(&self) -> String {
        let mut s = String::new();
        for line in &self.log {
            s.push_str(line);
            s.push('\n');
        }
        s.push_str(&self.summary());
        s
    }
}

/// 안정 시각 — 드라이버는 lease 만료를 쓰지 않으므로 0 고정(모든 ready 를 즉시 처리).
fn drive_now_ms() -> u64 {
    0
}

/// 한 라운드의 요건 집합 전문을 사람이 읽는 마크다운으로 렌더(순수). 내용이 산출물이다 — 숫자(add=N)는
/// 지표일 뿐 무엇이·왜·어떻게 바뀌었는지가 관전의 본체다. state(title/badge)는 진실(history)의 투영이므로
/// history 계보를 그대로 펼쳐 "r1 add → r3 change" 처럼 보인다. 제거된 x 항목도 사유와 함께 포함(계보 유지).
fn render_round_dump(
    nodes: &[Node],
    round: u32,
    adds: usize,
    changes: usize,
    removes: usize,
) -> String {
    let items: Vec<&Node> = nodes
        .iter()
        .filter(|n| n.kind.as_deref() == Some("item"))
        .collect();
    fn badge_of(n: &Node) -> &str {
        n.badge.as_deref().unwrap_or("")
    }
    let live = items.iter().filter(|n| badge_of(n) != "x").count();
    let mut out = format!(
        "## round {round} — 총 {live}개 (add={adds} change={changes} remove={removes})\n\n"
    );
    for n in &items {
        let mark = if badge_of(n) == "x" { "x" } else { "o" };
        let title = n.title.as_deref().unwrap_or("(제목 없음)");
        let origin = n.origin.as_deref().unwrap_or("?");
        out.push_str(&format!("- **[{mark}]** ({origin}) {title}\n"));
        if let Some(d) = n.description.as_deref().filter(|d| !d.is_empty()) {
            out.push_str(&format!("  - {d}\n"));
        }
        // result JSON = {reason, history[]}. reason=이 항목의 최신 근거, history=결정 계보.
        if let Some(parsed) = n
            .result
            .as_deref()
            .and_then(|r| serde_json::from_str::<Value>(r).ok())
        {
            if let Some(hist) = parsed.get("history").and_then(|h| h.as_array()) {
                for h in hist {
                    let r = h.get("round").and_then(|v| v.as_u64()).unwrap_or(0);
                    let op = h.get("op").and_then(|v| v.as_str()).unwrap_or("?");
                    let reason = h.get("reason").and_then(|v| v.as_str()).unwrap_or("");
                    out.push_str(&format!("  - r{r} {op}: {reason}\n"));
                }
            }
        }
    }
    out.push('\n');
    out
}

/// 스냅샷들을 최신이 위로 쌓아 파일에 쓴다(실패는 무시 — 관전 보조라 루프를 막지 않는다).
fn write_dump(path: &std::path::Path, snapshots: &[String]) {
    let mut body = String::from("# DRAFT 요건 집합 — 라운드별 스냅샷 (최신이 위)\n\n");
    for s in snapshots.iter().rev() {
        body.push_str(s);
    }
    let _ = std::fs::write(path, body);
}

/// body(JSON)에서 stage·round 를 읽어 로그 라벨을 만든다.
fn stage_round(body: &str) -> (Option<String>, Option<u64>) {
    let v: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return (None, None),
    };
    let stage = v.get("stage").and_then(|s| s.as_str()).map(String::from);
    // round 미지정은 최초 라운드(1) — reconcile 의 read_round 와 같은 의미. 첫 draft-review 틱도 round=1 로 보인다.
    let round = v
        .pointer("/args/round")
        .and_then(|r| match r {
            Value::Number(n) => n.as_u64(),
            Value::String(s) => s.parse().ok(),
            _ => None,
        })
        .or(stage.as_deref().map(|_| 1));
    (stage, round)
}

/// 틱 전이를 **즉시** 관전하도록 stdout 에 흘리고(라인 flush) report.log 에도 적재한다. 버퍼링해 끝에
/// 몰아 찍으면 "살아있고 순차적"이라는 이 워크플로의 존재 이유를 배신한다 — 라운드는 쌓이는 순간 보여야 한다.
fn emit(log: &mut Vec<String>, line: String) {
    use std::io::Write;
    println!("{line}");
    let _ = std::io::stdout().flush();
    log.push(line);
}

/// ready 노드가 없어질 때까지 reconcile_tick 을 반복한다. 각 틱의 스테이지·라운드·badge 전이를 **즉시**
/// 흘린다(emit). reconcile 로직은 손대지 않는다 — 이 루프는 스케줄러(poke→reconcile)를 인라인으로 대체할 뿐이다.
///
/// max_ticks: 무한 루프 안전판(상한 봉인이 실패해도 여기서 멈춘다). fail_cap: 같은 노드 연속 실패 상한.
pub fn drive<E: Exec>(
    board: &MemBoard<E>,
    max_ticks: usize,
    fail_cap: u32,
    dump: Option<&std::path::Path>,
) -> DriveReport {
    let mut state = ReconcileState::default();
    let mut report = DriveReport::default();
    let mut fails: HashMap<String, u32> = HashMap::new();
    // 라운드별 요건 집합 전문 스냅샷(오래된→최신). 파일엔 최신을 위로 쓴다.
    let mut snapshots: Vec<String> = Vec::new();

    emit(
        &mut report.log,
        "── 드라이브 시작 (틱마다 즉시 출력) ──".to_string(),
    );
    loop {
        if report.ticks >= max_ticks {
            report.aborted = Some(format!("max_ticks {max_ticks} 도달 — ready 잔존"));
            break;
        }
        let res = reconcile_tick(board, &mut state, drive_now_ms());
        let processed = res.get("processed").and_then(|v| v.as_u64()).unwrap_or(0);
        let id = res.get("id").and_then(|v| v.as_str());

        // ready 없음 — {ok:true, processed:0} 이고 id 필드가 없다(idempotent 무-op 과 구분).
        let Some(id) = id else {
            break;
        };
        let id = id.to_string();
        report.ticks += 1;

        let ok = res.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
        if !ok {
            let n = fails.entry(id.clone()).or_insert(0);
            *n += 1;
            let msg = res
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("(사유 없음)");
            emit(
                &mut report.log,
                format!("tick {}: id={id} 실패({n}) {msg}", report.ticks),
            );
            if *n >= fail_cap {
                report.aborted = Some(format!("노드 {id} 연속 {n}회 실패 — 중단: {msg}"));
                break;
            }
            continue;
        }
        fails.remove(&id);

        // 로그 라벨 — 실행된 노드의 kind/stage/round/badge.
        let node = board.node(&id);
        let kind = node
            .as_ref()
            .and_then(|n| n.kind.clone())
            .unwrap_or_default();
        let label = if kind == "task" {
            let (stage, round) = node
                .as_ref()
                .map(|n| stage_round(n.body.as_deref().unwrap_or("")))
                .unwrap_or((None, None));
            let stage = stage.unwrap_or_else(|| "?".into());
            let mut lbl = match round {
                Some(r) => format!("stage={stage} round={r}"),
                None => format!("stage={stage}"),
            };
            // whole-set 라운드의 델타 3수 — 집합이 얼마나 좁혀졌나를 관전 채널(로그)에 노출한다.
            // 이 숫자가 tool 인자 truncate 로 요건 전문이 안 보이는 상황의 유일한 관전 신호다.
            if let (Some(a), Some(c), Some(rm)) = (
                res.get("adds").and_then(|v| v.as_u64()),
                res.get("changes").and_then(|v| v.as_u64()),
                res.get("removes").and_then(|v| v.as_u64()),
            ) {
                lbl.push_str(&format!(" add={a} change={c} remove={rm}"));
                // 내용 덤프 — 이 라운드 후 보드의 요건 전문을 스냅샷으로. 숫자 옆에 "무엇이·왜"를 남긴다.
                if let Some(path) = dump {
                    if stage == "draft-review" {
                        let r = round.unwrap_or(0) as u32;
                        snapshots.push(render_round_dump(
                            &board.nodes(),
                            r,
                            a as usize,
                            c as usize,
                            rm as usize,
                        ));
                        write_dump(path, &snapshots);
                    }
                }
            }
            lbl
        } else {
            let badge = res
                .get("badge")
                .and_then(|v| v.as_str())
                .or_else(|| node.as_ref().and_then(|n| n.badge.as_deref()))
                .unwrap_or("?");
            format!("{kind} badge={badge}")
        };
        let published = res.get("published").and_then(|v| v.as_u64());
        let mut line = format!(
            "tick {}: id={id} {label} → processed={processed}",
            report.ticks
        );
        if let Some(p) = published {
            line.push_str(&format!(" published={p}"));
        }
        emit(&mut report.log, line);
    }

    // 최종 롤업 — chunk badge, item 수, draft-review 라운드 수(합의 task 개수).
    let nodes = board.nodes();
    report.items = nodes
        .iter()
        .filter(|n| n.kind.as_deref() == Some("item"))
        .count();
    report.draft_review_rounds = nodes
        .iter()
        .filter(|n| {
            n.kind.as_deref() == Some("task")
                && stage_round(n.body.as_deref().unwrap_or("")).0.as_deref() == Some("draft-review")
        })
        .count();
    if let Some(chunk) = nodes.iter().find(|n| n.kind.as_deref() == Some("chunk")) {
        report.final_chunk_badge = chunk.badge.clone();
        match chunk.badge.as_deref() {
            Some("o") => report.converged = true,
            Some("f") => report.sealed = true,
            _ => {}
        }
    }
    report
}

#[cfg(test)]
#[path = "mem_board_tests.rs"]
mod tests;
