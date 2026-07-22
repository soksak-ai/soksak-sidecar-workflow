// spec_set.rs 순수 델타 계산 테스트 — 전체집합 산출 → add/change/remove/수렴/fail-loud,
// 그리고 핵심 불변식: state 는 history 로부터 정확히 재구성된다.
use super::*;

fn cur(id: &str, state: &str, title: &str, desc: &str, history: Value) -> Value {
    json!({ "id": id, "state": state, "title": title, "description": desc, "history": history })
}

/// add 이력(값 포함 — 재구성의 출발점).
fn h_add(round: u32, reason: &str, title: &str, desc: &str) -> Value {
    json!({ "round": round, "op": "add", "reason": reason, "title": title, "description": desc })
}

fn base() -> Vec<Value> {
    vec![
        cur(
            "i0",
            "o",
            "무한 분할",
            "d0",
            json!([h_add(1, "지시서 명시", "무한 분할", "d0")]),
        ),
        cur(
            "i1",
            "o",
            "사이드바 충돌",
            "d1",
            json!([h_add(1, "핵심 난제", "사이드바 충돌", "d1")]),
        ),
    ]
}

#[test]
fn empty_current_makes_every_item_a_new_add() {
    // round-1 은 기존 집합이 ∅ 이라 자연히 전체 생성이 된다 — 특수 케이스가 필요 없다.
    let out = plan(
        &[],
        &json!({ "requirements": [
            { "title": "A", "description": "da", "origin": "user", "reason": "지시서 명시" },
            { "title": "B", "description": "db", "origin": "agent", "reason": "back-side" }
        ], "removed": [] }),
        1,
    );
    assert!(out.violations.is_empty(), "{:?}", out.violations);
    assert_eq!(out.creates.len(), 2);
    assert_eq!(out.creates[0]["origin"], "user");
    assert_eq!(out.creates[0]["history"][0]["op"], "add");
    assert_eq!(out.creates[0]["history"][0]["reason"], "지시서 명시");
    assert!(!out.converged, "생성이 있으면 수렴 아님");
}

#[test]
fn new_item_without_reason_fails_loud_even_on_first_round() {
    // 최초 라운드라고 근거를 면제하지 않는다 — 그때가 전부 add 라 전부 근거가 필요한 순간이다.
    let out = plan(
        &[],
        &json!({ "requirements": [{ "title": "A", "description": "d" }], "removed": [] }),
        1,
    );
    assert!(
        out.violations.iter().any(|v| v.contains("근거")),
        "{:?}",
        out.violations
    );
    assert!(out.creates.is_empty(), "위반이면 아무것도 만들지 않는다");
}

#[test]
fn identical_set_converges() {
    let out = plan(
        &base(),
        &json!({ "requirements": [
            { "id": "i0", "title": "무한 분할", "description": "d0" },
            { "id": "i1", "title": "사이드바 충돌", "description": "d1" }
        ], "removed": [] }),
        3,
    );
    assert!(out.violations.is_empty(), "{:?}", out.violations);
    assert!(out.converged, "집합 동일 = 수렴");
    assert!(out.creates.is_empty() && out.edits.is_empty());
}

#[test]
fn kept_item_needs_no_reason_and_inherits_history() {
    // 유지 항목에 매 라운드 근거 재서술을 강요하면 스퓨리어스 diff 만 생긴다 — id 로 승계한다.
    let out = plan(
        &base(),
        &json!({ "requirements": [
            { "id": "i0", "title": "무한 분할", "description": "d0" },
            { "id": "i1", "title": "사이드바 충돌", "description": "d1" }
        ], "removed": [] }),
        2,
    );
    assert!(out.violations.is_empty(), "근거 없어도 유지는 정상");
    assert!(out.edits.is_empty(), "유지는 편집조차 만들지 않는다");
}

#[test]
fn unmentioned_existing_item_fails_loud_naming_the_id() {
    // 누락과 의도적 제거를 같게 처리하면 요건이 조용히 증발한다.
    let out = plan(
        &base(),
        &json!({ "requirements": [{ "id": "i0", "title": "무한 분할", "description": "d0" }], "removed": [] }),
        2,
    );
    let v = out.violations.join(" | ");
    assert!(v.contains("i1"), "누락 id 지목: {v}");
    assert!(v.contains("미언급"), "{v}");
}

#[test]
fn removed_without_reason_fails_loud() {
    let out = plan(
        &base(),
        &json!({ "requirements": [{ "id": "i0", "title": "무한 분할", "description": "d0" }],
                 "removed": [{ "id": "i1" }] }),
        2,
    );
    assert!(
        out.violations.iter().any(|x| x.contains("사유 없음")),
        "{:?}",
        out.violations
    );
}

#[test]
fn removal_flips_to_x_and_appends_history_without_overwriting() {
    let out = plan(
        &base(),
        &json!({ "requirements": [{ "id": "i0", "title": "무한 분할", "description": "d0" }],
                 "removed": [{ "id": "i1", "reason": "i0 이 흡수 — 중복" }] }),
        2,
    );
    assert!(out.violations.is_empty(), "{:?}", out.violations);
    let e = out.edits.iter().find(|e| e.id == "i1").unwrap();
    assert_eq!(e.state, "x", "제거는 삭제가 아니라 x 잔존");
    assert_eq!(e.history.len(), 2, "적층: 최초 add + 이번 remove");
    assert_eq!(e.history[0]["reason"], "핵심 난제", "이전 사유 보존");
    assert_eq!(e.history[1]["op"], "remove");
    assert_eq!(e.history[1]["reason"], "i0 이 흡수 — 중복");
    assert!(!out.converged);
}

// ── change — 일급 연산: id·history 유지한 채 문장/맥락 교정(remove+add 아님) ──

#[test]
fn text_change_with_reason_preserves_id_and_stacks_history() {
    let out = plan(
        &base(),
        &json!({ "requirements": [
            { "id": "i0", "title": "무한 분할(뼈대 고정)", "description": "d0-명확화", "reason": "분할 범위가 모호해 오독됨" },
            { "id": "i1", "title": "사이드바 충돌", "description": "d1" }
        ], "removed": [] }),
        4,
    );
    assert!(out.violations.is_empty(), "{:?}", out.violations);
    assert_eq!(out.edits.len(), 1, "change 1건");
    let e = &out.edits[0];
    assert_eq!(e.id, "i0", "id 유지 — remove+add 아님");
    assert_eq!(e.state, "o");
    assert_eq!(e.title.as_deref(), Some("무한 분할(뼈대 고정)"));
    assert_eq!(e.history.len(), 2, "history 적층");
    assert_eq!(e.history[0]["reason"], "지시서 명시", "원 근거 보존");
    assert_eq!(e.history[1]["op"], "change");
    assert_eq!(
        e.history[1]["title"], "무한 분할(뼈대 고정)",
        "무엇이 어떻게 바뀌었는지 기록"
    );
    assert!(out.creates.is_empty(), "change 는 신규 발행이 아니다");
    assert!(!out.converged, "change 가 있으면 수렴 아님");
}

#[test]
fn text_change_without_reason_fails_loud() {
    // 근거를 못 대는 재서술은 개정이 아니라 잡음이다 — 이 의무가 churn 을 억제한다.
    let out = plan(
        &base(),
        &json!({ "requirements": [
            { "id": "i0", "title": "무한 분할 기능", "description": "d0" },
            { "id": "i1", "title": "사이드바 충돌", "description": "d1" }
        ], "removed": [] }),
        4,
    );
    assert!(
        out.violations.iter().any(|v| v.contains("change")),
        "{:?}",
        out.violations
    );
    assert!(out.edits.is_empty());
}

#[test]
fn readd_of_removed_item_is_an_add_requiring_a_counter_reason() {
    // 연산은 셋뿐 — 제거된 것을 되살리는 것도 add 다. 계보: add → remove → add.
    let current = vec![cur(
        "ix",
        "x",
        "권한 경계",
        "dx",
        json!([
            h_add(1, "보안 필수", "권한 경계", "dx"),
            json!({ "round": 2, "op": "remove", "reason": "범위밖" })
        ]),
    )];
    let ok = plan(
        &current,
        &json!({ "requirements": [{ "id": "ix", "title": "권한 경계", "description": "dx",
                 "reason": "AI 실행 권한은 범위 안 — r2 의 범위밖 판단이 틀렸다" }], "removed": [] }),
        5,
    );
    assert!(ok.violations.is_empty(), "{:?}", ok.violations);
    let e = &ok.edits[0];
    assert_eq!(e.state, "o", "x→o 복원");
    assert_eq!(e.history.len(), 3, "add → remove → add 계보");
    assert_eq!(e.history[2]["op"], "add");

    let bad = plan(
        &current,
        &json!({ "requirements": [{ "id": "ix", "title": "권한 경계", "description": "dx" }], "removed": [] }),
        5,
    );
    assert!(
        bad.violations.iter().any(|v| v.contains("재추가")),
        "근거 없는 재추가 거부: {:?}",
        bad.violations
    );
}

#[test]
fn removed_item_is_not_required_to_be_relisted() {
    // 제거된 항목은 현재 집합이 아니다 — 매 라운드 다시 싣지 않아도 미언급 위반이 아니다.
    let current = vec![
        cur("i0", "o", "A", "d", json!([h_add(1, "r", "A", "d")])),
        cur(
            "ix",
            "x",
            "B",
            "d",
            json!([
                h_add(1, "r", "B", "d"),
                json!({ "round": 2, "op": "remove", "reason": "범위밖" })
            ]),
        ),
    ];
    let out = plan(
        &current,
        &json!({ "requirements": [{ "id": "i0", "title": "A", "description": "d" }], "removed": [] }),
        3,
    );
    assert!(out.violations.is_empty(), "{:?}", out.violations);
    assert!(out.converged, "제거 항목 제외하면 집합 동일 = 수렴");
}

#[test]
fn unknown_id_fails_loud_instead_of_silently_becoming_new() {
    let out = plan(
        &base(),
        &json!({ "requirements": [
            { "id": "i0", "title": "무한 분할", "description": "d0" },
            { "id": "i1", "title": "사이드바 충돌", "description": "d1" },
            { "id": "ghost", "title": "유령", "description": "d", "reason": "r" }
        ], "removed": [] }),
        2,
    );
    assert!(
        out.violations.iter().any(|v| v.contains("미지 id")),
        "{:?}",
        out.violations
    );
}

// ── 핵심 불변식: state 는 history 로부터 재구성 가능하다 ──
// 재구성이 안 되면 어딘가에서 기록 없이 상태를 만진 것이고, 그게 곧 무성 증발이다.

#[test]
fn state_is_reconstructible_from_history_across_add_change_remove_add() {
    // 임의 시퀀스를 plan 으로 실제 굴려 history 를 쌓고, fold 로 되접어 state 와 대조한다.
    let mut current = vec![];

    // r1: add
    let p1 = plan(
        &[],
        &json!({ "requirements": [{ "title": "권한 경계", "description": "d1", "reason": "보안" }], "removed": [] }),
        1,
    );
    let mut hist: Vec<Value> = p1.creates[0]["history"].as_array().unwrap().clone();
    let (mut title, mut desc, mut badge) =
        ("권한 경계".to_string(), "d1".to_string(), "o".to_string());
    current.push(cur("k1", &badge, &title, &desc, json!(hist)));

    // r2: change
    let p2 = plan(
        &current,
        &json!({ "requirements": [{ "id": "k1", "title": "권한 경계(실행)", "description": "d2", "reason": "모호" }], "removed": [] }),
        2,
    );
    let e = &p2.edits[0];
    hist = e.history.clone();
    title = e.title.clone().unwrap();
    desc = e.description.clone().unwrap();
    current = vec![cur("k1", &badge, &title, &desc, json!(hist))];

    // r3: remove
    let p3 = plan(
        &current,
        &json!({ "requirements": [], "removed": [{ "id": "k1", "reason": "범위밖" }] }),
        3,
    );
    let e = &p3.edits[0];
    hist = e.history.clone();
    badge = e.state.clone();
    current = vec![cur("k1", &badge, &title, &desc, json!(hist))];

    // r4: 재추가(=add)
    let p4 = plan(
        &current,
        &json!({ "requirements": [{ "id": "k1", "title": "권한 경계(실행)", "description": "d4",
                 "reason": "r3 범위밖 판단이 틀렸다 — AI 실행은 범위 안" }], "removed": [] }),
        4,
    );
    let e = &p4.edits[0];
    hist = e.history.clone();
    desc = e.description.clone().unwrap();
    badge = e.state.clone();

    assert_eq!(hist.len(), 4, "add→change→remove→add 4건 적층(덮어쓰기 0)");
    let ops: Vec<&str> = hist.iter().map(|h| h["op"].as_str().unwrap()).collect();
    assert_eq!(ops, vec!["add", "change", "remove", "add"], "순서 보존");

    // 불변식 — 접은 결과가 현재 state 와 정확히 일치.
    let folded = fold(&hist).expect("재구성");
    assert_eq!(
        folded,
        Folded {
            title: title.clone(),
            description: desc.clone(),
            badge: badge.clone(),
        },
        "state 는 history 의 투영이어야 한다 — 불일치 = 기록 없는 상태 변경"
    );
}

#[test]
fn fold_reconstructs_removed_state_as_x() {
    let hist = vec![
        h_add(1, "r", "A", "da"),
        json!({ "round": 2, "op": "remove", "reason": "범위밖" }),
    ];
    let f = fold(&hist).unwrap();
    assert_eq!(f.badge, "x", "제거는 상태 x — 기록은 계속 살아 있다");
    assert_eq!(f.title, "A", "제거해도 내용은 계보에 남는다");
}

#[test]
fn fold_of_empty_history_is_none() {
    assert!(fold(&[]).is_none(), "기록이 없으면 재구성할 상태도 없다");
}
