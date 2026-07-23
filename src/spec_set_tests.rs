// spec_set.rs 순수 마크-적용 테스트 — 모델은 add/change/remove 마크만 달고 시스템이 지속 문서에 적용.
// 핵심: 미언급 기존 id = keep(흘림 구조적 불가), fail-loud(무근거·미지 id), fold(history)==state 재구성.
use super::*;

fn cur(id: &str, state: &str, title: &str, history: Value) -> Value {
    json!({ "id": id, "state": state, "title": title, "description": "", "history": history })
}

fn h_add(round: u32, reason: &str, title: &str) -> Value {
    json!({ "round": round, "op": "add", "reason": reason, "title": title })
}

fn base() -> Vec<Value> {
    vec![
        cur(
            "i0",
            "o",
            "무한 분할",
            json!([h_add(1, "지시서 명시", "무한 분할")]),
        ),
        cur(
            "i1",
            "o",
            "사이드바 충돌",
            json!([h_add(1, "핵심 난제", "사이드바 충돌")]),
        ),
    ]
}

// (g) round-1: 문서 ∅ → 전부 add.
#[test]
fn empty_document_round_one_is_all_adds() {
    let out = plan(
        &[],
        &json!({ "add": [
            { "text": "A", "reason": "지시서 명시", "origin": "user" },
            { "text": "B", "reason": "back-side", "origin": "agent" }
        ], "change": [], "remove": [] }),
        1,
    );
    assert!(out.violations.is_empty(), "{:?}", out.violations);
    assert_eq!(out.creates.len(), 2);
    assert_eq!(out.adds, 2);
    assert_eq!(out.creates[0]["origin"], "user");
    assert_eq!(out.creates[0]["history"][0]["op"], "add");
    assert!(!out.converged);
}

// (b) 미언급 기존 id = keep — 흘림 구조적 불가. 40개 중 안 건드린 건 그대로.
#[test]
fn unmentioned_ids_are_kept_untouched() {
    let doc: Vec<Value> = (0..40)
        .map(|n| {
            cur(
                &format!("k{n}"),
                "o",
                &format!("요건 {n}"),
                json!([h_add(1, "r", &format!("요건 {n}"))]),
            )
        })
        .collect();
    let out = plan(
        &doc,
        &json!({ "add": [], "change": [], "remove": [{ "id": "k7", "reason": "중복" }] }),
        2,
    );
    assert!(
        out.violations.is_empty(),
        "미언급은 위반이 아니다: {:?}",
        out.violations
    );
    assert_eq!(out.creates.len(), 0, "신규 없음");
    assert_eq!(
        out.edits.len(),
        1,
        "k7 remove 하나만 — 나머지 39개는 손대지 않음"
    );
    assert_eq!(out.edits[0].id, "k7");
    assert_eq!(out.edits[0].state, "x");
    assert_eq!(out.removes, 1);
}

// (a) add/change/remove 마크가 문서에 정확 적용.
#[test]
fn marks_apply_add_change_remove() {
    let out = plan(
        &base(),
        &json!({
            "add": [{ "text": "크래시 복구", "reason": "back-side", "origin": "agent" }],
            "change": [{ "id": "i0", "text": "무한 분할(뼈대 고정)", "reason": "모호" }],
            "remove": [{ "id": "i1", "reason": "중복" }]
        }),
        2,
    );
    assert!(out.violations.is_empty(), "{:?}", out.violations);
    assert_eq!((out.adds, out.changes, out.removes), (1, 1, 1));
    assert_eq!(out.creates[0]["title"], "크래시 복구");
    let e0 = out.edits.iter().find(|e| e.id == "i0").unwrap();
    assert_eq!(e0.state, "o");
    assert_eq!(e0.title.as_deref(), Some("무한 분할(뼈대 고정)"));
    assert_eq!(e0.history.len(), 2);
    assert_eq!(e0.history[1]["op"], "change");
    let e1 = out.edits.iter().find(|e| e.id == "i1").unwrap();
    assert_eq!(e1.state, "x");
    assert_eq!(e1.history.len(), 2);
    assert_eq!(
        e1.history[0]["reason"], "핵심 난제",
        "이전 사유 보존(덮어쓰기 0)"
    );
    assert_eq!(e1.history[1]["op"], "remove");
    assert!(!out.converged);
}

// (c) 없는 id 의 change/remove = fail-loud.
#[test]
fn change_or_remove_of_unknown_id_fails_loud() {
    let a = plan(
        &base(),
        &json!({ "add": [], "change": [{ "id": "ghost", "text": "T", "reason": "r" }], "remove": [] }),
        2,
    );
    assert!(
        a.violations.iter().any(|v| v.contains("미지 id")),
        "{:?}",
        a.violations
    );
    assert!(a.edits.is_empty(), "위반이면 아무것도 적용하지 않는다");

    let b = plan(
        &base(),
        &json!({ "add": [], "change": [], "remove": [{ "id": "ghost", "reason": "r" }] }),
        2,
    );
    assert!(
        b.violations.iter().any(|v| v.contains("미지 id")),
        "{:?}",
        b.violations
    );
}

// (d) 무근거 마크 = fail-loud (add·change·remove 전부).
#[test]
fn marks_without_reason_fail_loud() {
    for mark in [
        json!({ "add": [{ "text": "T" }], "change": [], "remove": [] }),
        json!({ "add": [], "change": [{ "id": "i0", "text": "T2" }], "remove": [] }),
        json!({ "add": [], "change": [], "remove": [{ "id": "i0" }] }),
    ] {
        let out = plan(&base(), &mark, 2);
        assert!(
            out.violations
                .iter()
                .any(|v| v.contains("근거") || v.contains("사유")),
            "무근거 마크는 위반: {mark} → {:?}",
            out.violations
        );
    }
}

// (e) 마크 0 = 수렴.
#[test]
fn no_marks_is_convergence() {
    let out = plan(
        &base(),
        &json!({ "add": [], "change": [], "remove": [] }),
        3,
    );
    assert!(out.violations.is_empty());
    assert!(out.converged, "마크 0 = 문서 불변 = 수렴");
    assert_eq!((out.adds, out.changes, out.removes), (0, 0, 0));
}

// change 로 텍스트 동일 = 무-op(keep) — 스퓨리어스 diff 로 수렴을 막지 않는다.
#[test]
fn change_to_identical_text_is_a_noop_keep() {
    let out = plan(
        &base(),
        &json!({ "add": [], "change": [{ "id": "i0", "text": "무한 분할", "reason": "재확인" }], "remove": [] }),
        3,
    );
    assert!(out.violations.is_empty());
    assert!(out.edits.is_empty(), "동일 텍스트 change 는 무-op");
    assert!(out.converged);
}

// 재추가 — 제거된 id 에 change 를 걸면 x→o. 계보 add→remove→add.
#[test]
fn change_on_removed_id_readds_it() {
    let current = vec![cur(
        "ix",
        "x",
        "권한 경계",
        json!([
            h_add(1, "보안 필수", "권한 경계"),
            json!({ "round": 2, "op": "remove", "reason": "범위밖" })
        ]),
    )];
    let out = plan(
        &current,
        &json!({ "add": [], "change": [{ "id": "ix", "text": "권한 경계", "reason": "r2 범위밖 판단이 틀렸다" }], "remove": [] }),
        5,
    );
    assert!(out.violations.is_empty(), "{:?}", out.violations);
    let e = &out.edits[0];
    assert_eq!(e.state, "o", "x→o 복원");
    assert_eq!(e.history.len(), 3, "add→remove→add 계보");
    assert_eq!(e.history[2]["op"], "add");
    assert_eq!(out.adds, 1, "재추가는 add 로 계수");
}

// ── history=진실 / fold==state 재구성 불변식(마크 시퀀스로 실제 굴려 확인) ──
#[test]
fn state_is_reconstructible_from_history_across_add_change_remove_readd() {
    let p1 = plan(
        &[],
        &json!({ "add": [{ "text": "권한 경계", "reason": "보안" }], "change": [], "remove": [] }),
        1,
    );
    let mut hist: Vec<Value> = p1.creates[0]["history"].as_array().unwrap().clone();
    let (mut title, mut badge) = ("권한 경계".to_string(), "o".to_string());
    let mut current = vec![cur("k1", &badge, &title, json!(hist))];

    let p2 = plan(
        &current,
        &json!({ "add": [], "change": [{ "id": "k1", "text": "권한 경계(실행)", "reason": "모호" }], "remove": [] }),
        2,
    );
    let e = &p2.edits[0];
    hist = e.history.clone();
    title = e.title.clone().unwrap();
    current = vec![cur("k1", &badge, &title, json!(hist))];

    let p3 = plan(
        &current,
        &json!({ "add": [], "change": [], "remove": [{ "id": "k1", "reason": "범위밖" }] }),
        3,
    );
    let e = &p3.edits[0];
    hist = e.history.clone();
    badge = e.state.clone();
    current = vec![cur("k1", &badge, &title, json!(hist))];

    let p4 = plan(
        &current,
        &json!({ "add": [], "change": [{ "id": "k1", "text": "권한 경계(실행)", "reason": "r3 틀림" }], "remove": [] }),
        4,
    );
    let e = &p4.edits[0];
    hist = e.history.clone();
    badge = e.state.clone();

    assert_eq!(hist.len(), 4, "add→change→remove→add 4건 적층(덮어쓰기 0)");
    let ops: Vec<&str> = hist.iter().map(|h| h["op"].as_str().unwrap()).collect();
    assert_eq!(ops, vec!["add", "change", "remove", "add"], "순서 보존");

    let folded = fold(&hist).expect("재구성");
    assert_eq!(
        folded,
        Folded {
            title: title.clone(),
            description: String::new(),
            badge: badge.clone()
        },
        "state 는 history 의 투영 — 불일치 = 기록 없는 상태 변경"
    );
}

#[test]
fn fold_reconstructs_removed_state_as_x() {
    let hist = vec![
        h_add(1, "r", "A"),
        json!({ "round": 2, "op": "remove", "reason": "범위밖" }),
    ];
    let f = fold(&hist).unwrap();
    assert_eq!(f.badge, "x");
    assert_eq!(f.title, "A", "제거해도 내용은 계보에 남는다");
}

#[test]
fn fold_of_empty_history_is_none() {
    assert!(fold(&[]).is_none());
}
