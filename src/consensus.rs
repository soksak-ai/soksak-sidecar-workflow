//! consensus — 재사용 합의 루프의 순수 핵심. 완전성/정확성은 조각(hunt·audit·per-item·렌즈)이 아니라
//! **하나의 자율 루프**다: 한 집합을 놓고 각 라운드 reviewer 가 [현재 집합 + 변경 히스토리] 를 보고 **더하거나
//! (add) 뺀다(remove)**. 아무도 이견 없을 때(변경 0)까지 반복. 목적 = 3자(사람) 개입 없이 스스로 교정·종료.
//!
//! 세 요소가 없으면 사람을 부르게 된다:
//!   1. remove — 잘못 든 것을 루프가 스스로 걷어냄(add 전용은 자기교정 불가).
//!   2. 변경 히스토리 — "무엇을·왜 add/remove 했나"를 다음 라운드에 주입 → 재론·진동(remove→re-add) 차단.
//!   3. 이견 0 수렴 — add·remove 둘 다 0 = 합의 = 종료.
//!
//! 이 모듈은 순수(reviewer 산출 → 적용 결정)다. 이벤트 발행·badge 편집·다음 라운드 발행은 reconcile 이
//! 이 결과로 수행한다. draft·research·design·plan 네 완전성 지점이 같은 루프를 재사용한다.

use serde_json::{json, Value};

/// 한 라운드 reviewer 산출을 적용한 결과.
#[derive(Debug, PartialEq)]
pub struct ReviewOutcome {
    /// 신규 항목(그대로 발행 — badge=검수전 또는 파생물이면 o).
    pub additions: Vec<Value>,
    /// 자기교정: (targetId, reason) — 대상 항목 badge → x(반박·중복·범위밖). 삭제 아님(이력·감사 보존).
    pub badge_edits: Vec<(String, String)>,
    /// 이 라운드 변경 요약 — 다음 라운드 프롬프트의 {{history}} 로 주입(진동 차단).
    pub history_lines: Vec<String>,
    /// add·remove 둘 다 0 = 이견 없음 = 합의 = 다음 스테이지로.
    pub converged: bool,
}

/// apply_review — 순수. reviewer 의 {additions, removals} + 라운드 번호 → ReviewOutcome.
/// additions = 신규 항목 배열. removals = [{id, reason}] 배열. id 빈 remove 는 무시(방어).
pub fn apply_review(review: &Value, round: u32) -> ReviewOutcome {
    let additions: Vec<Value> = review
        .get("additions")
        .and_then(|a| a.as_array())
        .cloned()
        .unwrap_or_default();
    let removals: Vec<Value> = review
        .get("removals")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();

    let mut badge_edits = Vec::new();
    let mut history_lines = Vec::new();
    for a in &additions {
        if let Some(t) = a.get("title").and_then(|t| t.as_str()) {
            history_lines.push(format!("R{round} +add: {t}"));
        }
    }
    for r in &removals {
        let id = r
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if id.is_empty() {
            continue;
        }
        let reason = r
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        history_lines.push(format!("R{round} -remove {id}: {reason}"));
        badge_edits.push((id, reason));
    }

    let converged = additions.is_empty() && badge_edits.is_empty();
    ReviewOutcome {
        additions,
        badge_edits,
        history_lines,
        converged,
    }
}

/// 확정 문서 규약 v1 — 항목이 history[]{round, action, reason} 를 지닌다. reviewer 출력 = changes[].
/// 한 항목 편집(remove/reraise) — 새 state + append 된 전체 history.
#[derive(Debug, PartialEq)]
pub struct ItemEdit {
    pub id: String,
    pub state: String,
    pub history: Vec<Value>,
}

/// apply_changes 결과 — 신규(creates)·기존편집(edits)·수렴(changes 빈 라운드).
#[derive(Debug, PartialEq)]
pub struct ChangeSet {
    /// add — {state:"o", title, description, history:[{round,"add",reason}]}. id 는 보드가 배정.
    pub creates: Vec<Value>,
    /// remove/reraise — 대상 state 전환 + history append(사유 보존, 재론 차단).
    pub edits: Vec<ItemEdit>,
    /// changes 빈 라운드 = 이견 없음 = 합의.
    pub converged: bool,
}

/// apply_changes — 순수. 현재 items(history 포함) + reviewer changes[{op,id?,title?,description?,reason,origin?}] + round.
/// add=신규(history 시작). remove=o→x(사유 append). reraise=x→o(반박사유 append). 전이 불일치는 무시(방어).
/// reraise 의 "제거사유 반박" 은 프롬프트가 강제(reason 이 무엇을 반박하는지는 순수코어가 판정 불가).
pub fn apply_changes(items: &[Value], changes: &[Value], round: u32) -> ChangeSet {
    use std::collections::HashMap;
    let by_id: HashMap<&str, &Value> = items
        .iter()
        .filter_map(|it| it.get("id").and_then(|v| v.as_str()).map(|id| (id, it)))
        .collect();
    let mut creates = Vec::new();
    let mut edits = Vec::new();
    for c in changes {
        let op = c.get("op").and_then(|v| v.as_str()).unwrap_or("");
        let reason = c
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        match op {
            "add" => {
                let title = c
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if title.is_empty() {
                    continue;
                }
                let description = c
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let decision = json!({ "round": round, "action": "add", "reason": reason });
                let mut item = json!({ "state": "o", "title": title, "description": description, "history": [decision] });
                // origin 은 optional 통과 — 지정한 지점(draft)만 싣고, 안 내는 지점(research/design)의
                // create 는 키 자체가 없어 기존 발행과 동일하게 남는다. 스키마 밖 값은 통과시키지 않는다.
                if let Some(o) = c
                    .get("origin")
                    .and_then(|v| v.as_str())
                    .filter(|o| matches!(*o, "user" | "agent"))
                {
                    item["origin"] = json!(o);
                }
                creates.push(item);
            }
            "remove" | "reraise" => {
                let id = c.get("id").and_then(|v| v.as_str()).unwrap_or("").trim();
                let Some(item) = by_id.get(id) else { continue };
                let cur = item.get("state").and_then(|v| v.as_str()).unwrap_or("o");
                let (from, to) = if op == "remove" {
                    ("o", "x")
                } else {
                    ("x", "o")
                };
                if cur != from {
                    continue; // remove 는 o 만, reraise 는 x 만 — 전이 불일치 방어
                }
                let mut history: Vec<Value> = item
                    .get("history")
                    .and_then(|h| h.as_array())
                    .cloned()
                    .unwrap_or_default();
                history.push(json!({ "round": round, "action": op, "reason": reason }));
                edits.push(ItemEdit {
                    id: id.to_string(),
                    state: to.to_string(),
                    history,
                });
            }
            _ => {}
        }
    }
    let converged = creates.is_empty() && edits.is_empty();
    ChangeSet {
        creates,
        edits,
        converged,
    }
}

/// render_history — 누적 히스토리 줄들을 다음 라운드 프롬프트 주입용 블록으로. 비면 빈 문자열.
/// reviewer 는 이걸 보고 "이미 뺀 걸 도로 넣지" 않고, "이미 논의된 걸 재론" 하지 않는다.
pub fn render_history(lines: &[String]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let body = lines.join("\n");
    format!("변경 이력(이미 add/remove 된 것 — 재론·되돌림 금지):\n{body}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn add_and_remove_produces_edits_and_history() {
        let review = json!({
            "additions": [{ "title": "누락: 캐시·세션 전략", "description": "..." }],
            "removals": [{ "id": "fact3", "reason": "지시서가 명시적으로 배제한 범위 — 범위밖" }]
        });
        let out = apply_review(&review, 2);
        assert_eq!(out.additions.len(), 1, "add 1");
        assert_eq!(
            out.badge_edits,
            vec![(
                "fact3".to_string(),
                "지시서가 명시적으로 배제한 범위 — 범위밖".to_string()
            )],
            "remove → badge 편집(자기교정)"
        );
        assert!(!out.converged, "변경 있으면 미수렴");
        assert!(
            out.history_lines.iter().any(|l| l.contains("+add")),
            "add 이력: {:?}",
            out.history_lines
        );
        assert!(
            out.history_lines
                .iter()
                .any(|l| l.contains("-remove fact3")),
            "remove 이력(사유 포함): {:?}",
            out.history_lines
        );
    }

    #[test]
    fn no_change_is_consensus() {
        let out = apply_review(&json!({ "additions": [], "removals": [] }), 3);
        assert!(out.converged, "add·remove 0 = 이견 없음 = 합의(종료)");
        assert!(out.history_lines.is_empty());
    }

    #[test]
    fn add_only_not_yet_converged() {
        // add 만 있어도 이견 있음 → 미수렴(다음 라운드가 그 add 를 remove 할 수도).
        let out = apply_review(&json!({ "additions": [{ "title": "X" }] }), 1);
        assert!(!out.converged, "add 만 있어도 미수렴");
        assert!(out.badge_edits.is_empty());
    }

    #[test]
    fn remove_only_not_converged() {
        let out = apply_review(
            &json!({ "removals": [{ "id": "i5", "reason": "umbrella" }] }),
            1,
        );
        assert!(!out.converged, "remove 만 있어도 미수렴");
        assert_eq!(out.badge_edits.len(), 1);
    }

    #[test]
    fn empty_id_removal_ignored() {
        let out = apply_review(
            &json!({ "removals": [{ "id": "", "reason": "x" }, { "id": "i1", "reason": "y" }] }),
            1,
        );
        assert_eq!(
            out.badge_edits,
            vec![("i1".to_string(), "y".to_string())],
            "빈 id remove 무시(방어)"
        );
    }

    // ── 확정 문서 규약 v1: apply_changes(history 모델) ──
    #[test]
    fn changes_add_creates_item_with_history() {
        let cs = apply_changes(
            &[],
            &[
                json!({ "op": "add", "title": "약 등록면", "description": "제형·용량·용법", "reason": "중심 엔티티 입력면" }),
            ],
            1,
        );
        assert_eq!(cs.creates.len(), 1);
        let it = &cs.creates[0];
        assert_eq!(it["state"], "o");
        assert_eq!(it["title"], "약 등록면");
        assert_eq!(
            it["history"][0],
            json!({ "round": 1, "action": "add", "reason": "중심 엔티티 입력면" })
        );
        assert!(!cs.converged, "변경 있으면 미수렴");
    }

    #[test]
    fn changes_add_passes_origin_through_when_given() {
        // origin 은 "사용자가 말한 것 vs 에이전트가 채운 것" 을 보드에서 가르는 출처 축이다.
        let cs = apply_changes(
            &[],
            &[
                json!({ "op": "add", "title": "무한 분할", "description": "d", "reason": "지시서 명시", "origin": "user" }),
                json!({ "op": "add", "title": "크래시 복구", "description": "d", "reason": "back-side", "origin": "agent" }),
            ],
            1,
        );
        assert_eq!(cs.creates[0]["origin"], "user");
        assert_eq!(cs.creates[1]["origin"], "agent");
    }

    #[test]
    fn changes_add_omits_origin_key_entirely_when_absent() {
        // 공유 코어 additive 보장 — origin 을 내지 않는 지점(research-audit·design-audit)의 create 는
        // origin 키 자체가 없어야 한다. 기본값으로 채우면 그들의 발행 노드가 오염된다.
        let cs = apply_changes(
            &[],
            &[json!({ "op": "add", "title": "캐시 전략", "description": "d", "reason": "r" })],
            1,
        );
        assert!(
            cs.creates[0].get("origin").is_none(),
            "origin 미지정 → 키 부재(기본값 주입 금지): {}",
            cs.creates[0]
        );
    }

    #[test]
    fn changes_add_ignores_unknown_origin_value() {
        // 스키마 밖 값은 통과시키지 않는다 — 보드 출처 축이 임의 문자열로 오염되면 구분이 무의미해진다.
        let cs = apply_changes(
            &[],
            &[json!({ "op": "add", "title": "T", "reason": "r", "origin": "search" })],
            1,
        );
        assert!(cs.creates[0].get("origin").is_none(), "미지 origin 은 생략");
    }

    #[test]
    fn changes_remove_o_to_x_appends_history() {
        let items = vec![json!({ "id": "a6", "state": "o", "title": "마약류 대장",
            "history": [{ "round": 1, "action": "add", "reason": "대장 의무" }] })];
        let cs = apply_changes(
            &items,
            &[json!({ "op": "remove", "id": "a6", "reason": "a7 통합대장이 흡수 — 중복" })],
            2,
        );
        assert_eq!(cs.edits.len(), 1);
        assert_eq!(cs.edits[0].id, "a6");
        assert_eq!(cs.edits[0].state, "x", "remove → x");
        assert_eq!(cs.edits[0].history.len(), 2, "기존 add + remove append");
        assert_eq!(
            cs.edits[0].history[1],
            json!({ "round": 2, "action": "remove", "reason": "a7 통합대장이 흡수 — 중복" })
        );
    }

    #[test]
    fn changes_reraise_x_to_o_preserves_full_trail() {
        let items = vec![
            json!({ "id": "a5", "state": "x", "title": "입고 검수", "history": [
            { "round": 1, "action": "add", "reason": "재고정확도" },
            { "round": 2, "action": "remove", "reason": "i0 에 암시됨" }
        ] }),
        ];
        let cs = apply_changes(
            &items,
            &[json!({ "op": "reraise", "id": "a5", "reason": "독립 상태전이 — R2 반박" })],
            3,
        );
        assert_eq!(cs.edits[0].state, "o", "reraise → o");
        assert_eq!(
            cs.edits[0].history.len(),
            3,
            "add·remove·reraise 전 이력 보존"
        );
        assert_eq!(cs.edits[0].history[2]["action"], "reraise");
        assert_eq!(cs.edits[0].history[2]["reason"], "독립 상태전이 — R2 반박");
    }

    #[test]
    fn changes_transition_mismatch_ignored() {
        let items = vec![
            json!({ "id": "x1", "state": "x", "title": "t", "history": [] }),
            json!({ "id": "o1", "state": "o", "title": "t", "history": [] }),
        ];
        // 이미 x 를 remove / 이미 o 를 reraise → 무시(방어)
        let cs = apply_changes(
            &items,
            &[
                json!({ "op": "remove", "id": "x1", "reason": "r" }),
                json!({ "op": "reraise", "id": "o1", "reason": "r" }),
                json!({ "op": "remove", "id": "없는id", "reason": "r" }),
            ],
            4,
        );
        assert!(
            cs.edits.is_empty(),
            "전이 불일치·미존재 전부 무시: {:?}",
            cs.edits
        );
    }

    #[test]
    fn changes_empty_is_converged() {
        let cs = apply_changes(&[json!({ "id": "a", "state": "o", "history": [] })], &[], 5);
        assert!(cs.converged, "changes 0 = 이견 없음 = 합의");
    }

    #[test]
    fn history_renders_or_empty() {
        assert_eq!(render_history(&[]), "", "히스토리 없으면 빈 문자열");
        let r = render_history(&["R1 +add: A".to_string(), "R2 -remove i2: dup".to_string()]);
        assert!(
            r.contains("변경 이력") && r.contains("R1 +add: A") && r.contains("R2 -remove i2"),
            "{r}"
        );
    }
}
