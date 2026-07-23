//! spec_set — DRAFT 전용 **마크 적용**(순수). 모델은 지속 문서(요건 집합, 안정 id)를 **입력으로 읽고**
//! 출력은 **마크만** 단다: add / change / remove. 시스템이 그 마크를 문서에 적용한다.
//!
//! **왜 전체 재출력이 아니라 마크인가.** 전에는 모델에게 집합 전체를 매 라운드 재출력하게 했는데(옮겨적기),
//! 약한 모델이 기존 40개를 옮기다 흘렸다(실측). 근원은 옮겨적기다. 마크 모델에선 **미언급 기존 id = 그대로
//! keep** — 모델이 다시 안 쓰니 "기존을 흘림"이 **구조적으로 불가능**하다. 이게 이 재설계의 핵심 이득.
//! 출력은 마크지만 *추론*은 여전히 whole-set(저자가 전체를 읽고 이번 라운드에 더할·고칠·뺄 것을 다 마크)
//! 이라 drip(편집자 모드로 인색해짐)은 프롬프트가 막는다.
//!
//! **연산은 정확히 셋: add / change / remove.** 제거된 항목을 다시 넣는 것도 그 id 에 change 를 걸면 된다
//! (x→o, history op=add 로 기록 — 계보: r1 add → r3 remove → r5 add).
//!
//! **history 가 진실, state 는 투영.** history 는 append-only. 현재 title/badge 는 그 history 를 접은
//! 결과일 뿐이라 [`fold`] 로 되접으면 현재 상태가 정확히 재구성되어야 한다 — 마크는 history 에 append 되므로
//! 재구성 가능성이 유지된다.
//!
//! **근거가 검증 기제 전부다.** add·change·remove 는 reason 필수(무근거 잡음 금지 = fail-loud). change/remove
//! 의 id 가 실재하지 않아도 fail-loud. **미언급=keep 이므로 "미언급=누락 위반" 체크는 없다**(그게 흘림 불가를
//! 성립시킨다).
//!
//! consensus.rs(공유 코어)는 research/design 이 쓰므로 건드리지 않는다 — 이 경로가 독립으로 같은 규율을 만든다.

use serde_json::{json, Value};
use std::collections::HashMap;

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

/// 마크 적용 결과 — 시스템이 문서에 반영할 신규/편집 + 관측 카운트.
#[derive(Debug, Default)]
pub struct SpecSetPlan {
    /// 신규 발행 프레임 — {title, description, origin?, history:[add 엔트리]}.
    pub creates: Vec<Value>,
    pub edits: Vec<SpecEdit>,
    /// 마크 0(add·change·remove 전부 없음) — 문서 불변 → 수렴.
    pub converged: bool,
    /// 관측 채널(라이브 뷰) — 이 라운드에 실제 적용된 마크 수. 재추가는 add 로 계수.
    pub adds: usize,
    pub changes: usize,
    pub removes: usize,
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

/// plan — 지속 문서 current([{id,state,title,description,history}])에 모델의 마크(result 의 add/change/
/// remove)를 적용한다. **미언급 기존 id 는 손대지 않는다(=keep)** — 그래서 흘림이 구조적으로 불가능하다.
///
/// fail-loud: add/change/remove 는 reason 필수, change/remove 의 id 는 실재해야 한다. 위반이 하나라도
/// 있으면 호출부가 아무것도 변형하지 않고 에러를 올린다(부분 적용 금지).
pub fn plan(current: &[Value], result: &Value, round: u32) -> SpecSetPlan {
    let mut out = SpecSetPlan::default();
    let by_id: HashMap<String, &Value> = current
        .iter()
        .filter_map(|it| {
            it.get("id")
                .and_then(|v| v.as_str())
                .map(|id| (id.to_string(), it))
        })
        .collect();

    // ── add: 신규 요건. text·reason 필수. origin 있으면 통과(user|agent 만). ──
    for (i, a) in result
        .get("add")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .enumerate()
    {
        let text = s(a, "text");
        let reason = s(a, "reason");
        if text.is_empty() {
            out.violations.push(format!("add[{i}]: text 없음"));
            continue;
        }
        if reason.is_empty() {
            out.violations.push(format!(
                "add[{i}] \"{text}\": 근거(reason) 없음 — 근거 없는 항목은 아무도 책임지지 않는 주장이다"
            ));
            continue;
        }
        let mut item = json!({
            "title": text,
            "description": "",
            "history": [json!({ "round": round, "op": "add", "reason": reason, "title": text })],
        });
        let origin = s(a, "origin");
        if matches!(origin.as_str(), "user" | "agent") {
            item["origin"] = json!(origin);
        }
        out.creates.push(item);
        out.adds += 1;
    }

    // ── change: 기존 id 의 텍스트 교체. id 실재·reason 필수. x 상태 id 면 재추가(x→o, history op=add). ──
    for (i, c) in result
        .get("change")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .enumerate()
    {
        let id = s(c, "id");
        let text = s(c, "text");
        let reason = s(c, "reason");
        if id.is_empty() || text.is_empty() {
            out.violations
                .push(format!("change[{i}]: id 또는 text 없음"));
            continue;
        }
        let Some(cur) = by_id.get(&id) else {
            out.violations.push(format!(
                "change[{i}]: 미지 id \"{id}\" — 지속 문서에 없다(신규면 add 로)"
            ));
            continue;
        };
        if reason.is_empty() {
            out.violations.push(format!(
                "change[{i}](id {id}): 근거 없음 — 근거를 못 대는 교체는 잡음이다"
            ));
            continue;
        }
        let cur_state = cur.get("state").and_then(|v| v.as_str()).unwrap_or("o");
        let cur_title = s(cur, "title");
        let mut history = prior_history(cur);
        if cur_state == "x" {
            // 재추가 — x→o. fold 가 o 로 되접도록 history op=add.
            history.push(json!({ "round": round, "op": "add", "reason": reason, "title": text }));
            out.edits.push(SpecEdit {
                id,
                state: "o".into(),
                title: Some(text),
                description: None,
                history,
            });
            out.adds += 1;
        } else {
            // 텍스트 동일이면 무-op(keep) — 스퓨리어스 diff 로 수렴을 막지 않는다.
            if text == cur_title {
                continue;
            }
            history
                .push(json!({ "round": round, "op": "change", "reason": reason, "title": text }));
            out.edits.push(SpecEdit {
                id,
                state: "o".into(),
                title: Some(text),
                description: None,
                history,
            });
            out.changes += 1;
        }
    }

    // ── remove: 기존 id 를 x 로. id 실재·reason 필수. 이미 x 면 멱등 skip. ──
    for (i, r) in result
        .get("remove")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .enumerate()
    {
        let id = s(r, "id");
        let reason = s(r, "reason");
        if id.is_empty() {
            out.violations.push(format!("remove[{i}]: id 없음"));
            continue;
        }
        let Some(cur) = by_id.get(&id) else {
            out.violations
                .push(format!("remove[{i}]: 미지 id \"{id}\" — 지속 문서에 없다"));
            continue;
        };
        if reason.is_empty() {
            out.violations.push(format!(
                "remove[{i}](id {id}): 사유 없음 — 뺀 것인지 흘린 것인지 구분되지 않는다"
            ));
            continue;
        }
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
        out.removes += 1;
    }

    // 미언급 기존 id = keep(손대지 않음) — 흘림 구조적 불가. "미언급=누락" 체크는 없다.
    out.converged = out.creates.is_empty() && out.edits.is_empty();
    out
}

#[cfg(test)]
#[path = "spec_set_tests.rs"]
mod tests;
