// mem_board.rs 테스트 — 인메모리 보드 + 드라이버를 결정적 mock exec 로 검증한다(E2E 정본).
// 실 LLM 미호출(비결정적) — exec 를 stub 로 주입한다. make test-unit 의 cargo test 로 돈다.
// #[path] 로 mem_board 에 포함되어 super::* 는 mem_board 를 가리킨다.
//
// DRAFT = 단 하나의 자기-루프 op. 매 라운드 **전체 집합**을 산출하고(델타 아님) 시스템이 델타를
// 계산한다(add/change/remove + 미언급 fail-loud). 새 전체 ≡ 기존 전체면 수렴 = 인증 + 정지.
// round-1 은 기존 집합이 ∅ 이라 자연히 전체 생성이 된다 — 특수 케이스가 없다.
use super::*;
use crate::reconcile::{Deps, StageOut, CONSENSUS_ROUND_MAX};
use serde_json::{json, Value};
use std::cell::RefCell;

fn task_ev(id: &str, stage: &str, parent: &str) -> Value {
    json!({ "id": id, "kind": "task", "stage": stage, "parent": parent, "title": stage, "blocked_by": [] })
}

/// 한 라운드가 다는 마크. 미언급 기존 항목은 그대로 keep(마크 모델의 핵심 — 흘림 구조적 불가).
#[derive(Clone)]
enum Step {
    Add(&'static str),
    /// 기존 title 의 항목을 remove 마크.
    Remove(&'static str),
    /// 기존 title 의 항목을 새 텍스트로 change 마크.
    Change(&'static str, &'static str),
}

/// 결정적 mock provider — LLM 미호출. 지속 문서(args.ledger)를 읽어 title→id 를 해소하고 마크만 낸다.
struct MockExec {
    rounds: Vec<Vec<Step>>,
    /// true 면 매 라운드 새 항목을 하나씩 add 마크해 절대 수렴하지 않는다(상한 봉인 검증).
    never_converge: bool,
    calls: RefCell<Vec<String>>,
}

impl MockExec {
    fn new(rounds: Vec<Vec<Step>>) -> Self {
        MockExec {
            rounds,
            never_converge: false,
            calls: RefCell::new(vec![]),
        }
    }
    fn never_converging() -> Self {
        MockExec {
            rounds: vec![],
            never_converge: true,
            calls: RefCell::new(vec![]),
        }
    }
    fn one_calls(&self) -> usize {
        self.calls.borrow().iter().filter(|c| *c == "one").count()
    }
}

impl Exec for MockExec {
    // per-item 격리검증은 삭제됐다 — draft item 은 태생 o 라 여기로 오면 안 된다.
    fn exec_one(&self, _body: &str) -> Result<Value, String> {
        self.calls.borrow_mut().push("one".into());
        Err("exec_one 호출됨 — DRAFT 는 per-item 격리검증을 하지 않는다".into())
    }

    fn exec_stage(&self, body: &str) -> Result<StageOut, String> {
        let v: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
        let stage = v.get("stage").and_then(|s| s.as_str()).unwrap_or("");
        let chunk_ref = v
            .pointer("/args/chunkRef")
            .and_then(|c| c.as_str())
            .unwrap_or("chunk")
            .to_string();
        let round = v
            .pointer("/args/round")
            .and_then(|r| match r {
                Value::Number(n) => n.as_u64(),
                Value::String(s) => s.parse().ok(),
                _ => None,
            })
            .unwrap_or(1) as u32;
        self.calls.borrow_mut().push(format!("{stage}:{round}"));
        if stage != "draft-review" {
            return Err(format!("mock: 미지 stage {stage:?} — DRAFT 는 단일 op"));
        }

        // 지속 문서(state o 인 것)에서 title→id 해소. round-1 은 ∅.
        let doc = v
            .pointer("/args/ledger")
            .and_then(|l| l.as_array())
            .cloned()
            .unwrap_or_default();
        let id_of = |t: &str| -> Option<String> {
            doc.iter()
                .find(|e| {
                    e.get("state").and_then(|s| s.as_str()) != Some("x")
                        && e.get("title").and_then(|x| x.as_str()) == Some(t)
                })
                .and_then(|e| e.get("id").and_then(|i| i.as_str()).map(String::from))
        };

        let steps: Vec<Step> = if self.never_converge {
            vec![Step::Add("끝없는 요건")]
        } else {
            self.rounds
                .get((round - 1) as usize)
                .cloned()
                .unwrap_or_default()
        };
        let (mut add, mut change, mut remove) = (vec![], vec![], vec![]);
        for st in &steps {
            match st {
                Step::Add(t) => add.push(json!({
                    "text": t, "origin": "agent", "reason": format!("{t} 는 누락된 make-or-break")
                })),
                Step::Remove(t) => {
                    if let Some(id) = id_of(t) {
                        remove.push(json!({ "id": id, "reason": format!("{t} 는 중복") }));
                    }
                }
                Step::Change(from, to) => {
                    if let Some(id) = id_of(from) {
                        change.push(
                            json!({ "id": id, "text": to, "reason": "문구가 모호해 오독됨" }),
                        );
                    }
                }
            }
        }

        Ok(StageOut::Children {
            children: vec![task_ev("draft-review-again", "draft-review", &chunk_ref)],
            result: json!({ "add": add, "change": change, "remove": remove }),
        })
    }
}

/// 초기 DAG 시드 — draft.doc.json 의 `""` 스켈레톤 상당: chunk + Spec 섹션 + draft-review task.
fn seed(board: &MemBoard<MockExec>) -> String {
    let chunk = board
        .add_node(json!({
            "title": "구체화 덩어리", "parentId": Value::Null, "body": "",
            "blockedBy": [], "type": "task", "kind": "chunk",
        }))
        .expect("chunk 발행");
    board
        .add_node(json!({
            "title": "Spec", "parentId": chunk, "body": "", "blockedBy": [],
            "locked": true, "collapsed": true, "type": "task", "kind": "section",
        }))
        .expect("Spec 섹션 발행");
    let dr_body = json!({
        "skeleton": { "spec": "workflow-doc@0.0.1" },
        "stage": "draft-review",
        "args": { "directive": "테스트 지시어", "chunkRef": chunk },
    })
    .to_string();
    board
        .add_node(json!({
            "title": "완전성 합의 루프", "parentId": chunk, "body": dr_body,
            "blockedBy": [], "type": "task", "kind": "task",
        }))
        .expect("draft-review task 발행");
    chunk
}

fn run_rounds(rounds: Vec<Vec<Step>>) -> (MemBoard<MockExec>, DriveReport) {
    let board = MemBoard::new(MockExec::new(rounds));
    seed(&board);
    let report = drive(&board, 400, 3, None);
    (board, report)
}

fn items(board: &MemBoard<MockExec>) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = board
        .nodes()
        .into_iter()
        .filter(|n| n.kind.as_deref() == Some("item"))
        .map(|n| {
            (
                n.title.clone().unwrap_or_default(),
                n.badge.clone().unwrap_or_default(),
            )
        })
        .collect();
    v.sort();
    v
}

// 라이브 관전 — draft-review 틱 로그에 add/change/remove 수가 찍혀야 집합이 좁혀지는 걸 본다.
#[test]
fn drive_log_shows_add_change_remove_per_round() {
    let (_board, report) = run_rounds(vec![
        vec![Step::Add("A"), Step::Add("B"), Step::Add("C")],
        vec![
            Step::Add("D"),
            Step::Change("A", "A-정련"),
            Step::Remove("B"),
        ],
        vec![],
    ]);
    assert!(report.aborted.is_none(), "{:?}", report.aborted);
    let joined = report.log.join("\n");
    // round 1: 신규 3.
    assert!(
        report.log.iter().any(|l| l.contains("round=1")
            && l.contains("add=3")
            && l.contains("change=0")
            && l.contains("remove=0")),
        "round1 add=3 로그 없음:\n{joined}"
    );
    // round 2: 신규 1 · 개정 1 · 제거 1.
    assert!(
        report.log.iter().any(|l| l.contains("round=2")
            && l.contains("add=1")
            && l.contains("change=1")
            && l.contains("remove=1")),
        "round2 add=1 change=1 remove=1 로그 없음:\n{joined}"
    );
    // 수렴 라운드: 모두 0.
    assert!(
        report.log.iter().any(|l| l.contains("round=3")
            && l.contains("add=0")
            && l.contains("change=0")
            && l.contains("remove=0")),
        "round3 수렴 0 로그 없음:\n{joined}"
    );
}

// 내용이 산출물이다 — 라운드마다 요건 전문(title·badge·origin·reason·history 계보)을 사람이 읽는
// 마크다운 파일로 뱉는다. 숫자(add=N)는 지표일 뿐, 무엇이 왜 추가/변경/제거됐는지가 관전의 본체다.
#[test]
fn dump_writes_full_requirement_content_per_round() {
    let dir = std::env::temp_dir().join(format!("soksak-dump-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("spec.md");
    let board = MemBoard::new(MockExec::new(vec![
        vec![Step::Add("무한 분할"), Step::Add("사이드바 충돌 해소")],
        vec![
            Step::Add("크래시 복구"),
            Step::Change("무한 분할", "무한 분할(뼈대 고정)"),
            Step::Remove("사이드바 충돌 해소"),
        ],
        vec![],
    ]));
    seed(&board);
    let report = drive(&board, 400, 3, Some(path.as_path()));
    assert!(report.aborted.is_none(), "{:?}", report.aborted);
    let dump = std::fs::read_to_string(&path).expect("덤프 파일 생성");

    // (a) 요건 title 전문(잘림 없이).
    assert!(
        dump.contains("무한 분할(뼈대 고정)"),
        "개정된 title 전문:
{dump}"
    );
    assert!(
        dump.contains("크래시 복구"),
        "신규 title:
{dump}"
    );
    // (b) badge · origin — origin 은 (user)/(agent) 로 각 요건에 붙는다.
    assert!(
        dump.contains("(agent)"),
        "origin 표기(user/agent):
{dump}"
    );
    // (c) reason(왜 넣었나) — history 의 근거가 보여야 한다.
    assert!(
        dump.contains("누락된 make-or-break"),
        "add 근거:
{dump}"
    );
    // (d) history 계보 여러 줄 — add → change 가 한 줄씩.
    assert!(
        dump.contains("add") && dump.contains("change"),
        "history 계보:
{dump}"
    );
    // (e) 제거된 x 항목도 사유와 함께 포함(계보 안 끊김).
    assert!(
        dump.contains("사이드바 충돌 해소") && dump.contains("중복"),
        "제거 항목+사유 잔존:
{dump}"
    );
    assert!(
        dump.contains(" x ") || dump.contains("badge=x") || dump.contains("[x]"),
        "x badge 표기:
{dump}"
    );
    // (f) 라운드 헤더(델타 포함).
    assert!(
        dump.contains("round 2") && dump.contains("add=1") && dump.contains("remove=1"),
        "라운드 헤더+델타:
{dump}"
    );
    // (g) 최신 라운드가 위 — round 3(수렴)이 round 1 보다 먼저 나온다.
    let p3 = dump.find("round 3");
    let p1 = dump.find("round 1");
    assert!(
        p3.is_some() && p1.is_some() && p3 < p1,
        "최신 라운드가 위:
{dump}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// (a) round-1 = ∅ → 전체 생성. 격리검증 0회.
#[test]
fn round_one_from_empty_produces_the_whole_spec() {
    let (board, report) = run_rounds(vec![
        vec![
            Step::Add("무한 분할"),
            Step::Add("사이드바 충돌 해소"),
            Step::Add("크래시 복구"),
        ],
        vec![],
    ]);
    assert!(report.aborted.is_none(), "완주: {:?}", report.aborted);
    assert_eq!(report.items, 3, "round-1 이 ∅ 에서 3개를 생성");
    assert!(
        items(&board).iter().all(|(_, b)| b == "o"),
        "태생 o: {:?}",
        items(&board)
    );
    assert_eq!(
        board.exec().one_calls(),
        0,
        "per-item 격리검증 0회 — 그 경로는 DRAFT 에 없다"
    );
    assert_eq!(report.draft_review_rounds, 2, "round1 생성 + round2 수렴");
}

// (b) add/change/remove 가 누적되고, 집합이 그대로인 라운드에서 수렴한다.
#[test]
fn set_identity_across_a_round_is_convergence() {
    let (board, report) = run_rounds(vec![
        vec![Step::Add("무한 분할"), Step::Add("사이드바 충돌 해소")],
        vec![Step::Add("크래시 복구"), Step::Remove("무한 분할")],
        vec![Step::Change("크래시 복구", "크래시 복구·세션 복원")],
        vec![],
    ]);
    assert!(report.aborted.is_none(), "{:?}", report.aborted);
    assert_eq!(report.draft_review_rounds, 4, "3라운드 변경 + 4라운드 수렴");
    assert_eq!(
        items(&board),
        vec![
            ("무한 분할".to_string(), "x".to_string()),
            ("사이드바 충돌 해소".to_string(), "o".to_string()),
            ("크래시 복구·세션 복원".to_string(), "o".to_string()),
        ],
        "제거는 x 로 잔존(계보 보존), change 는 같은 노드의 제목만 바뀐다"
    );
    assert!(report.converged);
    assert_eq!(
        report.final_chunk_badge.as_deref(),
        Some("o"),
        "수렴 = 인증"
    );
}

// (c) change 는 remove+add 가 아니다 — 노드 수가 늘지 않고 history 가 적층된다.
#[test]
fn change_preserves_the_node_and_stacks_history() {
    let (board, _) = run_rounds(vec![
        vec![Step::Add("무한 분할")],
        vec![Step::Change("무한 분할", "무한 분할(뼈대 고정)")],
        vec![],
    ]);
    let its: Vec<_> = board
        .nodes()
        .into_iter()
        .filter(|n| n.kind.as_deref() == Some("item"))
        .collect();
    assert_eq!(its.len(), 1, "change 는 새 노드를 만들지 않는다");
    let n = &its[0];
    assert_eq!(n.title.as_deref(), Some("무한 분할(뼈대 고정)"));
    let hist: Value = serde_json::from_str(n.result.as_deref().unwrap()).expect("result JSON");
    let ops: Vec<&str> = hist["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["op"].as_str().unwrap())
        .collect();
    assert_eq!(ops, vec!["add", "change"], "history 적층(덮어쓰기 0)");
}

// (d) 재설계 핵심 이득 — 언급 안 한 기존 요건은 그대로 keep. 흘림이 구조적으로 불가능하다.
// 약한 모델이 40개 중 하나만 손대고 나머지를 "옮겨적기"로 흘리던 결함을 원천 봉쇄.
#[test]
fn unmentioned_requirements_survive_untouched() {
    // r1: 3개 생성. r2: 하나만 remove(나머지 2개는 언급조차 안 함). r3: 마크 0 → 수렴.
    let (board, report) = run_rounds(vec![
        vec![
            Step::Add("무한 분할"),
            Step::Add("사이드바 충돌 해소"),
            Step::Add("파일트리"),
        ],
        vec![Step::Remove("파일트리")],
        vec![],
    ]);
    assert!(report.aborted.is_none(), "{:?}", report.aborted);
    // 언급 안 한 두 개는 o 로 그대로, remove 한 하나만 x — 흘린 것 없음.
    assert_eq!(
        items(&board),
        vec![
            ("무한 분할".to_string(), "o".to_string()),
            ("사이드바 충돌 해소".to_string(), "o".to_string()),
            ("파일트리".to_string(), "x".to_string()),
        ],
        "미언급=keep — 손대지 않은 요건은 그대로 살아 있다"
    );
    assert!(report.converged, "r3 마크 0 → 수렴");
}

// (e) 상한 봉인 — 절대 수렴하지 않으면 chunk badge=f(인증 아님).
#[test]
fn seals_at_round_cap_when_never_converges() {
    let board = MemBoard::new(MockExec::never_converging());
    seed(&board);
    let report = drive(&board, 400, 3, None);
    assert!(
        report.aborted.is_none(),
        "봉인은 자연 종료: {:?}",
        report.aborted
    );
    assert!(report.sealed);
    assert!(!report.converged);
    assert_eq!(report.final_chunk_badge.as_deref(), Some("f"));
    assert_eq!(
        report.draft_review_rounds as u32, CONSENSUS_ROUND_MAX,
        "상한만큼만 돌고 멈춘다"
    );
    assert_eq!(board.exec().one_calls(), 0, "격리검증 0회");
}

// (f) 결정적 완주 — 재실행해도 같은 결과.
#[test]
fn full_convergence_is_deterministic() {
    let plan = || {
        vec![
            vec![Step::Add("A"), Step::Add("B")],
            vec![Step::Add("C"), Step::Remove("B")],
            vec![Step::Change("C", "C-정련")],
            vec![],
        ]
    };
    let (_, a) = run_rounds(plan());
    let (_, b) = run_rounds(plan());
    assert!(a.converged && b.converged);
    assert_eq!(a.ticks, b.ticks, "틱 수 결정적");
    assert_eq!(a.draft_review_rounds, b.draft_review_rounds);
    assert_eq!(a.items, b.items);
    assert_eq!(a.final_chunk_badge, b.final_chunk_badge);
}
