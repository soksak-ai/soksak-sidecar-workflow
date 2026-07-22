//! spec_set — DRAFT 전용 전체집합(whole-set) 산출의 델타 계산(순수).
//!
//! **왜 델타가 아니라 전체집합인가.** 모델에게 `changes[]` 를 요구하면 편집 모드(최소 개입)로 들어가
//! 구조적으로 인색해진다(실측: round 1 이 요건 1개만 냄). 출력이 집합 전체면 조금씩 흘릴 수가 없다.
//! 수렴 판정도 행동에서 구조로 바뀐다 — "빈 배열"(게을러졌는지 의견이 없는지 구분 불가) 대신
//! **새로 생성한 전체 ≡ 기존 전체** 라는 집합 동일성으로 판정하므로 모델의 성실성에 기대지 않는다.
//!
//! **연산은 정확히 셋: add / change / remove.** 그 밖은 없다. 제거된 항목을 다시 넣는 것도 `add` 다
//! (계보: r1 add → r3 remove → r5 add).
//!
//! **history 가 진실, state 는 투영.** history 는 append-only 이며 어떤 항목도 덮어쓰지 않는다.
//! 현재 title/description/badge 는 그 history 를 지금 시점으로 접은 결과일 뿐 기록 자체가 아니다.
//! 그래서 [`fold`] 로 되접으면 현재 상태가 정확히 재구성되어야 한다 — 재구성이 안 되면 어딘가에서
//! 기록 없이 상태를 만진 것이고, 그것이 곧 무성 증발이다.
//!
//! **근거가 검증 기제 전부다.** 격리 per-item 검증을 없애며 잃은 "프레임별 검증 코멘트"를 add 근거가
//! 대체한다 — 조각만 보고 사후에 붙이던 코멘트가, 전체 맥락에서 그 요건을 넣기로 한 이유로 바뀐다.
//! 그래서 근거 누락은 기본값으로 메우지 않고 fail-loud 로 막는다.
//!
//! consensus.rs(공유 코어)는 research/design 이 쓰므로 건드리지 않는다 — 이 경로가 독립으로 같은 규율을 만든다.

use serde_json::{json, Map, Value};
use std::collections::HashSet;

/// 기존 프레임 1건의 편집 — 상태 전이와 누적된 전체 history.
#[derive(Debug, Clone, PartialEq)]
pub struct SpecEdit {
    pub id: String,
    /// 전이 후 상태. "o"=집합에 있음, "x"=제거되어 잔존(삭제 아님 — 계보 보존).
    pub state: String,
    pub title: Option<String>,
    pub description: Option<String>,
    /// 기존 이력 + 이번 라운드 결정 1건(append-only).
    pub history: Vec<Value>,
}

/// 전체집합 산출 → 시스템이 계산한 델타.
#[derive(Debug, Default)]
pub struct SpecSetPlan {
    /// 신규 발행 프레임 — {title, description, origin?, history:[add 엔트리]}.
    pub creates: Vec<Value>,
    pub edits: Vec<SpecEdit>,
    /// add 0 ∧ change 0 ∧ remove 0 — 집합이 그대로면 수렴.
    pub converged: bool,
    /// fail-loud 사유. 비어 있지 않으면 호출부는 **아무것도 변형하지 말고** 에러를 올린다.
    pub violations: Vec<String>,
}

/// history 를 접은 현재 상태 — state 는 이것과 반드시 일치해야 한다.
#[derive(Debug, Clone, PartialEq)]
pub struct Folded {
    pub title: String,
    pub description: String,
    /// "o"=집합에 있음, "x"=제거됨.
    pub badge: String,
}

/// fold — history(진실)를 지금 시점으로 접어 state(투영)를 재구성한다.
/// add=값 설정+o, change=바뀐 필드 덮기, remove=x. 빈 history 는 재구성 불가(None).
pub fn fold(history: &[Value]) -> Option<Folded> {
    let mut f: Option<Folded> = None;
    for e in history {
        let op = e.get("op").and_then(|v| v.as_str()).unwrap_or("");
        let t = e.get("title").and_then(|v| v.as_str());
        let d = e.get("description").and_then(|v| v.as_str());
        match op {
            "add" => {
                let cur = f.take();
                f = Some(Folded {
                    title: t
                        .map(String::from)
                        .or_else(|| cur.as_ref().map(|c| c.title.clone()))
                        .unwrap_or_default(),
                    description: d
                        .map(String::from)
                        .or_else(|| cur.as_ref().map(|c| c.description.clone()))
                        .unwrap_or_default(),
                    badge: "o".into(),
                });
            }
            "change" => {
                if let Some(cur) = f.as_mut() {
                    if let Some(t) = t {
                        cur.title = t.to_string();
                    }
                    if let Some(d) = d {
                        cur.description = d.to_string();
                    }
                }
            }
            "remove" => {
                if let Some(cur) = f.as_mut() {
                    cur.badge = "x".into();
                }
            }
            _ => {}
        }
    }
    f
}

fn s(v: &Value, k: &str) -> String {
    v.get(k)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

fn prior_history(item: &Value) -> Vec<Value> {
    item.get("history")
        .and_then(|h| h.as_array())
        .cloned()
        .unwrap_or_default()
}

/// plan — current(문서: [{id,state,title,description,history}])와 모델의 전체집합 산출을 대조해
/// add/change/remove 와 수렴을 계산한다.
///
/// 근거 규칙: add·change·remove 는 reason 필수(빈 문자열 불가). 유지(내용 동일)만 면제 — 추가 시점
/// 근거가 history 에 남아 id 로 승계되므로 매 라운드 재서술은 스퓨리어스 diff 다.
pub fn plan(current: &[Value], result: &Value, round: u32) -> SpecSetPlan {
    let mut out = SpecSetPlan::default();
    let by_id: Map<String, Value> = current
        .iter()
        .filter_map(|it| {
            it.get("id")
                .and_then(|v| v.as_str())
                .map(|id| (id.to_string(), it.clone()))
        })
        .collect();

    let Some(reqs) = result.get("requirements").and_then(|r| r.as_array()) else {
        out.violations
            .push("전체집합 산출에 requirements 배열이 없다".into());
        return out;
    };

    let mut mentioned: HashSet<String> = HashSet::new();

    for (i, req) in reqs.iter().enumerate() {
        let id = s(req, "id");
        let title = s(req, "title");
        let description = s(req, "description");
        let reason = s(req, "reason");
        if title.is_empty() {
            out.violations
                .push(format!("requirements[{i}]: title 없음"));
            continue;
        }
        if id.is_empty() {
            // 신규 add — 근거 필수. 최초 라운드(∅→전체)도 면제 없다. 오히려 전부 add 라 전부 근거가 필요하다.
            if reason.is_empty() {
                out.violations.push(format!(
                    "신규 요건 \"{title}\": 근거(reason) 없음 — 근거 없는 항목은 아무도 책임지지 않는 주장이다"
                ));
                continue;
            }
            let mut item = json!({
                "title": title,
                "description": description,
                "history": [json!({ "round": round, "op": "add", "reason": reason,
                                    "title": title, "description": description })],
            });
            let origin = s(req, "origin");
            if matches!(origin.as_str(), "user" | "agent") {
                item["origin"] = json!(origin);
            }
            out.creates.push(item);
            continue;
        }
        mentioned.insert(id.clone());
        let Some(cur) = by_id.get(&id) else {
            // 존재하지 않는 id — 조용히 신규로 흘리면 계보가 위조된다.
            out.violations.push(format!(
                "requirements[{i}]: 미지 id \"{id}\" — 기존 집합에 없다(신규면 id 를 비워라)"
            ));
            continue;
        };
        let cur_state = cur.get("state").and_then(|v| v.as_str()).unwrap_or("o");
        let cur_title = s(cur, "title");
        let cur_desc = s(cur, "description");
        if cur_state == "x" {
            // 제거된 것을 다시 넣는 것도 add 다(계보: add → remove → add). 제거 사유를 반박하는 근거 필수.
            if reason.is_empty() {
                out.violations.push(format!(
                    "재추가 \"{title}\"(id {id}): 근거 없음 — 이전 제거 사유를 반박하는 근거가 필요하다"
                ));
                continue;
            }
            let mut history = prior_history(cur);
            history.push(json!({ "round": round, "op": "add", "reason": reason,
                                 "title": title, "description": description }));
            out.edits.push(SpecEdit {
                id,
                state: "o".into(),
                title: (title != cur_title).then_some(title),
                description: (description != cur_desc).then_some(description),
                history,
            });
            continue;
        }
        // 유지 또는 change(문장·맥락 교정 — id·history 를 유지한 채 하는 일급 연산).
        let changed = title != cur_title || description != cur_desc;
        if !changed {
            continue; // 원문 유지 — 근거 재서술 불필요(추가 시점 근거를 id 로 승계).
        }
        if reason.is_empty() {
            out.violations.push(format!(
                "change \"{cur_title}\"(id {id}): 근거 없음 — 근거를 못 대는 재서술은 개정이 아니라 잡음이다"
            ));
            continue;
        }
        let mut entry = json!({ "round": round, "op": "change", "reason": reason });
        if title != cur_title {
            entry["title"] = json!(title);
        }
        if description != cur_desc {
            entry["description"] = json!(description);
        }
        let mut history = prior_history(cur);
        history.push(entry);
        out.edits.push(SpecEdit {
            id,
            state: "o".into(),
            title: (title != cur_title).then_some(title),
            description: (description != cur_desc).then_some(description),
            history,
        });
    }

    // 의도적 remove — 사유 필수.
    if let Some(rem) = result.get("removed").and_then(|r| r.as_array()) {
        for (i, r) in rem.iter().enumerate() {
            let id = s(r, "id");
            let reason = s(r, "reason");
            if id.is_empty() {
                out.violations.push(format!("removed[{i}]: id 없음"));
                continue;
            }
            mentioned.insert(id.clone());
            if reason.is_empty() {
                out.violations.push(format!(
                    "removed[{i}](id {id}): 사유 없음 — 뺀 것인지 흘린 것인지 구분되지 않는다"
                ));
                continue;
            }
            let Some(cur) = by_id.get(&id) else { continue };
            if cur.get("state").and_then(|v| v.as_str()) == Some("x") {
                continue; // 이미 제거됨 — 멱등.
            }
            let mut history = prior_history(cur);
            history.push(json!({ "round": round, "op": "remove", "reason": reason }));
            out.edits.push(SpecEdit {
                id,
                state: "x".into(),
                title: None,
                description: None,
                history,
            });
        }
    }

    // 미언급 기존 항목 — 누락(깜빡)과 의도적 제거를 같게 처리하면 요건이 조용히 증발한다.
    let mut missing: Vec<String> = by_id
        .iter()
        .filter(|(id, it)| {
            it.get("state").and_then(|v| v.as_str()).unwrap_or("o") == "o"
                && !mentioned.contains(*id)
        })
        .map(|(id, _)| id.clone())
        .collect();
    missing.sort();
    if !missing.is_empty() {
        out.violations.push(format!(
            "전체집합 산출에서 기존 요건 {}건 미언급: [{}] — 유지면 그대로 싣고, 뺄 것이면 removed 에 사유와 함께 명시하라",
            missing.len(),
            missing.join(", ")
        ));
    }

    out.converged = out.creates.is_empty() && out.edits.is_empty();
    out
}

#[cfg(test)]
#[path = "spec_set_tests.rs"]
mod tests;
