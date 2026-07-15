//! directive_loop — 한 줄 지시 → 검증된 기능정의 스펙. run_loop 드라이버가 4 기능을 오케스트레이션:
//!   생성기능(초기 주제, broad) → [검증기능(공백 검증+누락제안, grounded) ↔ 결정기능(승격·수렴, broad)] → 렌더기능.
//!   누락은 검증기능이 제안 → 결정기능이 승격. 파일(주제 목록)이 단일 진실. loop-until-dry. 모든 호출 = claude -p.
//!   원칙: (1) 모든 상태 전이에 사유 (2) 파일은 영속·재사용(누적).

use crate::provider::{parse_json_lenient, run_agent_text, AgentRequest};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

pub const BLANK: &str = "공백";
pub const OK: &str = "[o]";
pub const FAIL: &str = "[x]"; // 마이너/정당한 off(요건 아님) — [o]처럼 해결됨 취급, 수렴 안 막음
pub const FATAL: &str = "[f]"; // 크리티컬(자기모순·불가 전제·치명적 미검증) — 즉시 중단

/// 한 검증기능 콜이 다루는 공백 주제 수. 0=무한대(한 콜). 단 25-50개는 900s 타임아웃이라 적정값으로.
const BATCH: usize = 12;
/// 검증기능 콜 사이 쿨다운(초). real Anthropic 은 최소.
const COOLDOWN: u64 = 2;

/// COMMON — 생성·검증·결정이 공유하는 개념(1회 정의, 중복 제거). 각 역할 프롬프트에 prepend.
const COMMON: &str = r#"SHARED CONCEPTS:
- A REQUIREMENT = an imperative the result must satisfy ("the system/plan/novel/work must …"): concrete and developable/executable — NOT a background fact, NOT a restatement of the directive. (Form: not "X regulations" but "the system must DO <Y> to satisfy <X>".)
- MAKE-OR-BREAK = its absence would make the result FAIL or be WRONG, not merely less polished. A genuine one is a DECISION two competent practitioners could resolve DIFFERENTLY — NOT a nice-to-have, NOT one methodology's enumerated beat-list, NOT the HOW / implementation-detail of another requirement (that is covered by its parent, not a separate requirement).
- EXPERT STANCE: read THIS directive as the SENIOR PRACTITIONER of its real domain (a pharmacist / compliance officer for a drug system, a novelist for a novel, an expedition leader for a climb). An expert never stops at the CATEGORY — "comply with the narcotics law" / "protect personal data" is NOT a requirement; it HIDES the concrete obligations the law compels, each its own make-or-break ("on a stock discrepancy, file the incident report to the authority within the statutory deadline"; "verify the vendor is a licensed wholesaler at registration"). Name the SPECIFIC trigger / deadline / check whoever builds, writes, or executes it must satisfy — a distinct requirement an expert knows, never an implementation beat. (Likewise outside law: a novel — not "a satisfying ending" but the specific turn that earns it; a plan — not "be safe" but the specific abort threshold.) A broad "support / comply with X" topic HIDES the gap, it does not cover it.
- THE BACK-SIDE: the requester is NOT a domain expert — they named the visible SURFACE (the easy 80%) and, even in a DETAILED directive, omit the make-or-break BACK-SIDE (the 20% that decides success) a senior practitioner / law / safety requires for the intent to actually work, be legal, be safe (the administrative, legal, financial, safety, contingency/failure-handling, oversight/who-administers substrate). DRAW IT OUT — adversarially ask of THIS intent: who actually OPERATES it, OVERSEES/administers it, PAYS FOR it, is kept SAFE/legal by it, and RECOVERS it when it fails or ends — and what does each REQUIRE that the requester never said? Don't be seduced by a polished, plausible surface: that polish IS the 80% trap. Then use the per-domain SHAPES below — the KIND to hunt; they COMPLEMENT the questions above (a minimal domain hint), never REPLACE them, and are NOT answers (search the real content; apply ONLY what genuinely fits THIS directive, never force a non-applicable category):
    · SYSTEM → operator/admin console per permission grade & oversight, data model, regulation (the SPECIFIC reporting/incident triggers, deadlines, and qualifications the governing law compels — kinds to hunt, not answers), security boundaries, monitoring, lifecycle/offboarding.
    · NOVEL → the avenger's corrosion, justice-vs-vengeance, antagonist depth, the delay engine, the payoff, the aftermath, POV/reveal-order, setting/world, reader complicity.
    · PLAN → go/no-go gates, per-step verification, contingency/rollback, failure modes, legal/safety preconditions (the SPECIFIC approvals, clearances, and qualifications required before an act may proceed — kinds to hunt, not answers), responsibility, exit criteria.
    · EVERYDAY (e.g. moving house) → registration, deposit/fee settlement, address changes, defect-check — not just the visible act.
- LEGAL LENS: wherever the intent's success turns on real-world LAW, RULE, or LEGITIMACY — to be COMPLIANT (a statutory duty it must satisfy), to be PERMITTED (an approval / license / qualification that gates an act or a participation), or to be ACCURATE (a work that portrays or relies on real law) — surface the binding obligations, approvals, triggers, and deadlines the real, current law actually compels, not just the functional surface (ground them by GROUNDING below). This is NOT only for regulated systems: a plan may need a clearance, a novel may need its law right. Apply ONLY where the intent genuinely turns on law; never force it onto one that does not.
- GROUNDING (when to SEARCH vs REASON — the one rule for any fact you rely on): the real test is "could you be WRONG from memory?" — info beyond your knowledge cutoff (a recent event, the CURRENT status of a law/program/standard), OR the SPECIFICS of a named statute/article/standard/figure/framework you could misremember, OR genuine uncertainty → WebSearch (put the current year in queries; NEVER assert such specifics from memory). A general principle or common design/craft choice you RELIABLY know → reason it; do NOT search what you reliably know (wasteful), and never re-search the settled.
- COMMON-SENSE DEFAULT: where the directive leaves a gap or ambiguity, resolve it with the SIMPLEST answer common reasoning reaches — what most competent people would call obvious. Do NOT invent an unusual, elaborate, or restrictive mechanism (a quantity cap, an enforcement, a control) the directive never asked for; a plain reading beats a clever one. The unusual must come from the directive — never from you.
- INVARIANTS — every requirement, whether GENERATED or ADDED: (1) ATOMIC — one subject, not bundled, not over-split; (2) NO DUPLICATE — not a restatement of another, judged by MEANING not wording (a narrower / re-angled / renamed / split version of an existing one is NOT new); (3) NO FORCING/FABRICATION — a genuine grounded make-or-break, never invented to seem thorough."#;

/// Attempt — 한 라운드의 검증 시도(과정 한 단계). 사유 필수.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Attempt {
    pub round: u32,
    pub status: String, // [o] | [x]
    pub reason: String, // 성공: 어떻게·왜 / 실패: 왜·무엇 보완
    #[serde(default)]
    pub verified_value: String,
}

/// Topic — 검증 단위(주제). status 는 history 마지막 attempt 에서 도출.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Topic {
    pub id: String,
    pub subject: String, // 주제
    #[serde(default = "default_origin")]
    pub origin: String, // user | agent | search — 이 요구의 근거 출처(증거는 verified_value/sources)
    pub status: String, // 공백 | [o] | [x]
    #[serde(default)]
    pub verified_value: String,
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub version: u32, // 값 바뀐 횟수(교정마다 +1)
    #[serde(default)]
    pub verify_count: u32, // 검증기능이 이 주제를 검증한 횟수 — 하버스가 셈(LLM 아님). history 의 검증외 이벤트 제외.
    #[serde(default)]
    pub history: Vec<Attempt>,
}
fn default_origin() -> String {
    "agent".into()
}

/// Ledger — 주제 목록(파일 = 단일 진실).
pub struct Ledger {
    pub path: PathBuf,
    pub topics: Vec<Topic>,
}
impl Ledger {
    pub fn load(path: PathBuf) -> Ledger {
        let topics = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Vec<Topic>>(&b).ok())
            .unwrap_or_default();
        Ledger { path, topics }
    }
    pub fn save(&self) -> Result<(), String> {
        let j = serde_json::to_string_pretty(&self.topics).map_err(|e| e.to_string())?;
        std::fs::write(&self.path, j)
            .map_err(|e| format!("ledger write {}: {e}", self.path.display()))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoundLog {
    pub round: u32,
    pub verified: usize,
    pub failed: usize,
    pub promoted: usize,
    pub decision: String,
    pub reason: String,
}

/// Outcome — 루프 산출.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Outcome {
    pub directive: String,
    pub spec: String,
    pub topics: Vec<Topic>,
    pub log: Vec<RoundLog>,
    pub converged: bool,
    pub aborted: bool,
    pub abort_reason: String,
    pub rounds: u32,
}

pub struct LoopConfig {
    pub agent_env: Vec<(String, String)>,
    pub verifier_model: String, // 집중 검증 — 가벼운 모델(glm-5.1=sonnet) 권장
    pub exec_model: String,     // broad 추론(생성·승격·결정·렌더) — glm-5.2=opus
    pub max_rounds: u32,
    pub concurrency: usize, // (A)검증 병렬 배치 수(기본 5). 웹서치 버스트 시 줄임.
}

// === 에이전트 산출 파싱 구조 ===

#[derive(Deserialize)]
struct GenTopic {
    id: String,
    subject: String,
    #[serde(default = "default_origin")]
    origin: String,
}
#[derive(Deserialize)]
struct GenResult {
    topics: Vec<GenTopic>,
}

#[derive(Deserialize)]
struct Verification {
    id: String,
    status: String, // [o] | [x]
    #[serde(default = "default_origin")]
    origin: String, // user | agent | search — 검증기가 확정한 최종 근거 출처
    #[serde(default)]
    reason: String,
    #[serde(default)]
    verified_value: String,
    #[serde(default)]
    sources: Vec<String>,
}
#[derive(Deserialize)]
struct Addition {
    subject: String,
    #[serde(default = "default_origin")]
    origin: String,
    #[serde(default)]
    reason: String,
}
#[derive(Deserialize)]
struct VerifyResult {
    #[serde(default)]
    verifications: Vec<Verification>,
    #[serde(default)]
    additions: Vec<Addition>,
}

#[derive(Deserialize)]
struct Promote {
    id: String,
    subject: String,
    #[serde(default = "default_origin")]
    origin: String,
}
#[derive(Deserialize)]
struct Refine {
    id: String,
    new_subject: String,
    #[serde(default)]
    reason: String,
}
#[derive(Deserialize)]
struct JudgeResult {
    #[serde(default)]
    promote: Vec<Promote>,
    #[serde(default)]
    refine: Vec<Refine>,
    decision: String, // continue | converge | abort
    #[serde(default)]
    reason: String,
}

// === 검증 게이트 (순수함수) ===

/// validate_ledger — 각 주제 status 가 history 마지막 attempt 와 일치하는지.
/// 불일치는 기계적으로 [x] 강등(주장된 [o] 불신) + 사유. 자가치유. 강등된 id 반환.
pub fn validate_ledger(topics: &mut [Topic], round: u32) -> Vec<String> {
    let mut downgraded = vec![];
    for t in topics.iter_mut() {
        if t.status == BLANK {
            continue; // 아직 검증 안 함 — 정상.
        }
        let consistent = matches!(t.history.last(), Some(a) if a.status == t.status);
        if !consistent {
            let reason = format!("status({})↔history 불일치 — 미검증 처리", t.status);
            t.history.push(Attempt {
                round,
                status: FAIL.into(),
                reason,
                verified_value: String::new(),
            });
            t.status = FAIL.into();
            downgraded.push(t.id.clone());
        }
    }
    downgraded
}

// === 에이전트 호출 ===

/// call_json — 에이전트 호출 + 파싱. 529/timeout/파싱실패는 백오프 재시도(인프라 실패는 [x] 아님).
fn call_json<T: DeserializeOwned>(
    prompt: String,
    tools: Vec<String>,
    model: &str,
    cfg: &LoopConfig,
) -> Result<T, String> {
    let mut last = String::new();
    for attempt in 0u64..3 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_secs(COOLDOWN * attempt)); // 백오프 — 레이트리밋 쿨다운.
            eprintln!(
                "  [재시도 {attempt}/2: {}]",
                last.chars().take(80).collect::<String>()
            );
        }
        match run_agent_text(
            &AgentRequest {
                prompt: prompt.clone(),
                model,
                allowed_tools: tools.clone(),
                timeout_secs: 3600,
                system_prompt: None,
                schema: None,
                effort: "xhigh".into(),
                text_only: false,
            },
            &cfg.agent_env,
        ) {
            Ok(text) => match parse_as::<T>(&text) {
                Ok(t) => return Ok(t),
                Err(e) => last = e,
            },
            Err(e) => last = e, // 529/timeout 등 — 재시도.
        }
    }
    Err(format!("call_json 3회 실패: {last}"))
}

/// parse_as — 산출 텍스트에서 T 로 역직렬화. 전체 lenient → 실패 시 top-level 객체 스캔.
fn parse_as<T: DeserializeOwned>(text: &str) -> Result<T, String> {
    if let Ok(v) = parse_json_lenient(text) {
        if let Ok(t) = serde_json::from_value::<T>(v) {
            return Ok(t);
        }
    }
    for obj in top_level_objects(text) {
        if let Ok(t) = serde_json::from_str::<T>(&obj) {
            return Ok(t);
        }
    }
    Err(format!(
        "산출 파싱 실패({}); head={}",
        std::any::type_name::<T>(),
        text.chars().take(300).collect::<String>()
    ))
}

fn top_level_objects(s: &str) -> Vec<String> {
    let bytes = s.as_bytes();
    let mut out = vec![];
    let mut depth = 0i32;
    let mut start: Option<usize> = None;
    let mut in_str = false;
    let mut esc = false;
    for (i, &b) in bytes.iter().enumerate() {
        let c = b as char;
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    if let Some(st) = start.take() {
                        out.push(s[st..=i].to_string());
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// ledger_view — 에이전트 프롬프트용 원장 요약(history 전량 대신 핵심).
fn ledger_view(topics: &[Topic]) -> String {
    let mut s = String::new();
    for t in topics {
        let last = t.history.last().map(|a| a.reason.as_str()).unwrap_or("");
        s.push_str(&format!(
            "- id={} | status={} | 출처={} | 주제: {}{}{}\n",
            t.id,
            t.status,
            t.origin,
            t.subject,
            if t.verified_value.is_empty() {
                String::new()
            } else {
                format!(" | 값: {}", t.verified_value)
            },
            if last.is_empty() {
                String::new()
            } else {
                format!(" | 최근: {}", last.chars().take(80).collect::<String>())
            },
        ));
    }
    s
}

// === 기능: 생성·검증·결정·렌더 ===

/// 생성기능 — 지시 → 초기 주제(공백) 분해. broad seed, 비슷한 건 한 주제로. 도구 off.
fn exec_generate(directive: &str, cfg: &LoopConfig) -> Result<Vec<Topic>, String> {
    let prompt = format!(
        r#"{COMMON}

YOUR ROLE — GENERATOR: turn the directive into the full set of REQUIREMENTS (per SHARED CONCEPTS).

**INTERPRET, do NOT echo.** Read the directive's real intent — never pass its surface phrasing through verbatim. A terse directive often bundles several DISTINCT requirements in one run-on clause (a constraint AND an action; a rule stated AS its own justification): read the meaning and split each distinct requirement into its OWN atomic topic, restated as a clear imperative. (ATOMIC: split bundled DISTINCT requirements; never split ONE requirement into its implementation beats.)
**GENERATION IS GENEROUS — cast WIDE.** Include EVERY plausible make-or-break (content, structural/craft, operational, regulated, the back-side). Generosity is SAFE here: the verifier grounds each and rejects ([x]) any that does not hold — so it is better to slightly OVER-include than to miss one. No cap, no stinginess; this set must be COMPLETE (a novel: POV/reveal-order AND setting/world, not only theme; a system: the data model, the operator back-side, regulation). Tightness belongs to the verifier's later ADDITIONS — NOT here. Obey the INVARIANTS. Tag each topic's ORIGIN: "user" if the directive states or clearly implies it, "agent" if you derive it as a make-or-break the directive never stated (you cannot search here — never "search"). There is NO optional tier: a nice-to-have is NOT a requirement — drop it, never defer it.

Directive: "{directive}"

Return ONLY JSON, no prose:
{{"topics":[{{"id":"<short-kebab-slug>","subject":"<one imperative requirement>","origin":"user"|"agent"}}]}}"#,
        COMMON = COMMON,
        directive = directive
    );
    let r: GenResult = call_json(prompt, vec![], &cfg.exec_model, cfg)?;
    Ok(r.topics
        .into_iter()
        .map(|g| Topic {
            id: g.id,
            subject: g.subject,
            origin: g.origin,
            status: BLANK.into(),
            verified_value: String::new(),
            sources: vec![],
            version: 0,
            verify_count: 0,
            history: vec![],
        })
        .collect())
}

/// 검증기능 — 공백 주제 전체를 grounded·domain-aware 로 검증 + 누락 추가요청. WebSearch on.
/// (A) 검증기능 — 나열된 주제만 RIGHT method 로 검증. 추가 제안 안 함(누락사냥은 exec_hunt). 병렬 배치로 호출. WebSearch on.
fn exec_verify(
    directive: &str,
    batch: &[Topic],
    all: &[Topic],
    cfg: &LoopConfig,
) -> Result<Vec<Verification>, String> {
    let to_verify = batch
        .iter()
        .map(|t| format!("- id={} | origin={} | 주제: {}", t.id, t.origin, t.subject))
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = format!(
        r#"{COMMON}

YOUR ROLE — VERIFIER (hostile). Verify ONLY the requirements listed below — do NOT propose NEW ones (that is a separate step). Search is NOT mandatory; for EXTERNAL FACTS use WebSearch and NEVER assert them from memory; reason out design/self-evident requirements.

VERIFY each (touch no others):
{to_verify}
For each, pick the method by GROUNDING (SHARED CONCEPTS): could you be WRONG from memory → WebSearch the specific (verified_value = fact + source); reliably know it → verify by REASONING (necessary AND sound? verified_value = why required/sound).
Then judge the OUTCOME — YOU decide the severity: holds AND is a real requirement → "{OK}" + origin + verified_value + sources + reason. NOT a real requirement (wrong / unnecessary / out-of-scope / a duty the directive disclaims) → "{FAIL}" + reason — a minor, LEGITIMATE off, NOT a failure (it simply is not a requirement; the result still stands without it). CRITICAL break — the directive is self-contradictory or rests on an impossible premise, OR this core make-or-break is fundamentally unverifiable AND fatal so the whole result cannot stand → "{FATAL}" + reason (the pipeline HALTS). Reserve "{FATAL}" for genuine show-stoppers; a negative-but-verified conclusion is "{OK}", not fatal. Set ORIGIN to how this requirement is BACKED, and record that backing in verified_value: "user" (the directive states/implies it → verified_value = the directive's own words) / "agent" (you reasoned it → verified_value = the knowledge basis, WHY it is required/sound) / "search" (you grounded it externally → verified_value = the fact + the quoted passage, sources = the URLs).
"{FAIL}" ≠ a failed search: if a NEEDED WebSearch ERRORS/empty (529), OMIT that topic (retry) — do NOT "{FAIL}". Terse; search ONLY the fact-hinged ones, reason out the rest.

Full ledger (context only — do NOT verify or add these):
{ledger}

Directive: "{directive}"

Do any needed searches first (only the fact-hinged ones). FINAL message = ONLY this JSON (no prose/fence):
{{"verifications":[{{"id":"...","status":"{OK}"|"{FAIL}"|"{FATAL}","origin":"user"|"agent"|"search","reason":"...","verified_value":"...","sources":["..."]}}]}}"#,
        COMMON = COMMON,
        OK = OK,
        FAIL = FAIL,
        FATAL = FATAL,
        to_verify = to_verify,
        ledger = ledger_view(all),
        directive = directive
    );
    let vr: VerifyResult = call_json(
        prompt,
        vec!["WebSearch".into(), "WebFetch".into()],
        &cfg.verifier_model,
        cfg,
    )?;
    Ok(vr.verifications)
}

/// (B) 누락사냥 — 전체 원장이 의도에 충분한지 인증, 빠진 make-or-break만 제안. 라운드당 1회(배치마다 X). WebSearch on.
fn exec_hunt(directive: &str, all: &[Topic], cfg: &LoopConfig) -> Result<Vec<Addition>, String> {
    let prompt = format!(
        r#"{COMMON}

YOUR ROLE — VERIFIER (hostile). CERTIFY THE WHOLE, not the parts. A part-by-part "{OK}" does NOT mean the result works — certify the ASSEMBLED set delivers the goal. The generator is an LLM; DISTRUST the list. Run ALL FIVE checks below; request what each surfaces (→ additions):
  - GOAL-REACH: DO state, in your reasoning, what the result must ACHIEVE for the requester beneath the surface, then check the ledger actually reaches it. Do NOT treat an "{OK}" substrate as proof the goal is reachable — if the core outcome rests on an impossible or unverified premise, VERIFY the premise (search if external) and request the feasibility precondition. Never assume the premise holds.
  - CONTRADICTION: DO mentally BUILD/EXECUTE the whole toward that goal. Where two requirements conflict so a builder is BLOCKED until one is overruled, request the requirement that RESOLVES which wins.
  - SEAM: where the JOIN between two requirements is owned by neither and a builder must GUESS a make-or-break decision (two competent builders would split), request the rule that OWNS the join.
  - DEPTH: apply the EXPERT STANCE + LEGAL LENS (SHARED CONCEPTS) to every named or regulated requirement — does it state the SPECIFIC obligation/trigger the law compels, or stop at the category? If only the category, request the specific one.
  - DOMAIN-FAILURE: as the senior practitioner, put the result as-built into ACTUAL real-world use (deploy the system, publish the novel, run the plan) and run it over time — beyond logical reach, what FAILS in PRACTICE that a part-by-part check misses: the goal's quality collapses, an intended effect never lands, or a real-world condition the set never accounted for breaks it? Request the make-or-break that prevents that failure. Stay inside the directive's stated scope — do NOT invent a duty it disclaims; if nothing breaks, request nothing.
Do NOT request nice-to-haves. Do NOT re-request what the ledger already covers (NO DUPLICATE — by MEANING, not wording). Do NOT rationalize ("looks complete" / "probably enough").
Hold the MAKE-OR-BREAK bar (SHARED CONCEPTS): request ONLY what omitting makes the result WRONG and two competent practitioners would resolve DIFFERENTLY — never the HOW of an existing requirement. Watch the over-enumeration smell — a cascade of ever-finer sub-requirements off one theme (record → its tamper-proofing → its retention → …): STOP and fold into the parent.
Each request grounded + ATOMIC (INVARIANTS). Do NOT manufacture or stretch a gap to seem thorough — ZERO additions is the correct, expected answer for a complete ledger, not a failure; a forced requirement is worse than none. Over-enumeration is failure.

Full ledger (judge whether it SUFFICES — propose ONLY missing make-or-breaks, do NOT re-verify these):
{ledger}

Directive: "{directive}"

Do any needed searches first. FINAL message = ONLY this JSON (no prose/fence):
{{"additions":[{{"subject":"...","origin":"agent"|"search","reason":"..."}}]}}"#,
        COMMON = COMMON,
        OK = OK,
        ledger = ledger_view(all),
        directive = directive
    );
    let vr: VerifyResult = call_json(
        prompt,
        vec!["WebSearch".into(), "WebFetch".into()],
        &cfg.verifier_model,
        cfg,
    )?;
    Ok(vr.additions)
}

/// 결정기능 — 추가요청 승격 + [x] 보완/종료 + 결정. 도구 off.
fn exec_decide(
    directive: &str,
    topics: &[Topic],
    additions: &[Addition],
    cfg: &LoopConfig,
) -> Result<JudgeResult, String> {
    let adds = additions
        .iter()
        .enumerate()
        .map(|(i, a)| format!("{}. [{}] {} — {}", i, a.origin, a.subject, a.reason))
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = format!(
        r#"{COMMON}

YOUR ROLE — JUDGE (orchestrator, BROAD view: you see all topics; the verifier saw a batch).
- promote: which addition requests become NEW topics — apply the INVARIANTS (ATOMIC; DEDUPE against the ledger; make-or-break only). Carry each addition's origin (agent|search). Assign a short kebab id.
- refine: ONLY a "{FAIL}" whose SUBJECT was MALFORMED (a garbled/bundled topic that names a REAL requirement unclearly) → give a clearer subject (it returns to blank, re-verified). Do NOT refine a "{FAIL}" correctly judged OFF (not a requirement) — leave it; it does NOT block convergence.
- decision: "converge" if every topic is "{OK}" or "{FAIL}" (RESOLVED — "{FAIL}" is a legitimate off, NOT a blocker) and no new promotions; "abort" if the directive is nonsensical (meaningless/self-contradictory/impossible premise); else "continue". (A topic the verifier marked "{FATAL}" already HALTS the run — not your call here.)

Directive: "{directive}"
Ledger:
{ledger}
Addition requests:
{adds}

Return ONLY JSON, no prose:
{{"promote":[{{"id":"<slug>","subject":"...","origin":"agent"|"search"}}],"refine":[{{"id":"<existing-id>","new_subject":"...","reason":"..."}}],"decision":"continue"|"converge"|"abort","reason":"..."}}"#,
        COMMON = COMMON,
        FAIL = FAIL,
        FATAL = FATAL,
        OK = OK,
        directive = directive,
        ledger = ledger_view(topics),
        adds = if adds.is_empty() {
            "(none)".into()
        } else {
            adds
        }
    );
    call_json(prompt, vec![], &cfg.exec_model, cfg)
}

/// 렌더기능 — [o] 주제들 → 기능정의 스펙(마크다운). 도구 off.
fn exec_render(directive: &str, topics: &[Topic], cfg: &LoopConfig) -> Result<String, String> {
    let prompt = format!(
        r#"You are the orchestrator. The ledger's "{OK}" topics are the VERIFIED requirements. Render the final one-direction functional-definition spec as markdown. Use plain, correct labels — no buzzwords:
- Objective: ONE sentence — what "done well" means for whoever the result serves, at the OUTCOME level (surface absent).
- The verified requirements, organized (group what belongs together). Tie each to the Objective. Tag each requirement with its ORIGIN — [user] (the directive stated it) / [agent] (derived by reasoning) / [search] (grounded in an external source) — so provenance stays traceable.
- EVERY listed requirement is REQUIRED. Do NOT defer, mark optional, or stage anything to "later / MVP / 유예" — there is no optional tier.
- Order of Work: foundations first.
- "{FAIL}" topics are NOT requirements (the verifier judged them off / out-of-scope) — do NOT put them in the spec body; list them briefly under a "검토 제외(요건 아님)" section with the one-line reason, for transparency.

Directive: "{directive}"
Ledger:
{ledger}

Output ONLY the spec markdown."#,
        OK = OK,
        FAIL = FAIL,
        directive = directive,
        ledger = ledger_view(topics)
    );
    run_agent_text(
        &AgentRequest {
            prompt,
            model: &cfg.exec_model,
            allowed_tools: vec![],
            timeout_secs: 3600,
            system_prompt: None,
            schema: None,
            effort: "xhigh".into(),
            text_only: false,
        },
        &cfg.agent_env,
    )
}

// === 적용 함수 ===

fn apply_verifications(topics: &mut [Topic], vs: &[Verification], round: u32) {
    for v in vs {
        if let Some(t) = topics.iter_mut().find(|t| t.id == v.id) {
            if t.status == OK {
                continue; // [o]는 안 건드림.
            }
            t.verify_count += 1; // 하버스(util)가 검증 횟수를 정확히 셈 — LLM 아님.
            let status = match v.status.as_str() {
                s if s == OK => OK,
                s if s == FATAL => FATAL,
                _ => FAIL,
            };
            // 교정: 이미 값이 있었는데 다른 값으로 [o] → version++.
            if status == OK && !t.verified_value.is_empty() && t.verified_value != v.verified_value
            {
                t.version += 1;
            }
            t.history.push(Attempt {
                round,
                status: status.into(),
                reason: v.reason.clone(),
                verified_value: v.verified_value.clone(),
            });
            t.status = status.into();
            if status == OK {
                t.verified_value = v.verified_value.clone();
                if !v.sources.is_empty() {
                    t.sources = v.sources.clone();
                }
                if !v.origin.is_empty() {
                    t.origin = v.origin.clone(); // 검증기가 확정한 최종 출처(생성기 잠정값 덮어씀).
                }
            }
        }
    }
}

fn apply_promotions(topics: &mut Vec<Topic>, promote: &[Promote]) -> usize {
    let mut n = 0;
    for p in promote {
        if topics.iter().any(|t| t.id == p.id) {
            continue; // 중복 id 방지.
        }
        topics.push(Topic {
            id: p.id.clone(),
            subject: p.subject.clone(),
            origin: p.origin.clone(),
            status: BLANK.into(),
            verified_value: String::new(),
            sources: vec![],
            version: 0,
            verify_count: 0,
            history: vec![],
        });
        n += 1;
    }
    n
}

fn apply_refines(topics: &mut [Topic], refine: &[Refine], round: u32) {
    for r in refine {
        if let Some(t) = topics.iter_mut().find(|t| t.id == r.id && t.status == FAIL) {
            t.subject = r.new_subject.clone();
            t.history.push(Attempt {
                round,
                status: BLANK.into(),
                reason: format!("결정기능 보완 → 재검증: {}", r.reason),
                verified_value: String::new(),
            });
            t.status = BLANK.into();
        }
    }
}

/// run_loop — 생성기능 → [검증기능(공백 전체) → validate → 결정기능(승격·보완·결정)] → 수렴/abort.
pub fn run_loop(directive: &str, ledger: &mut Ledger, cfg: &LoopConfig) -> Result<Outcome, String> {
    if ledger.topics.is_empty() {
        ledger.topics = exec_generate(directive, cfg)?;
        ledger.save()?;
    }
    let mut log = vec![];
    let mut converged = false;
    let mut aborted = false;
    let mut abort_reason = String::new();
    let mut rounds = 0;

    for round in 1..=cfg.max_rounds {
        rounds = round;
        // (A) 검증기능: 공백 주제를 BATCH 씩, concurrency 만큼 병렬 배치로 검증. [o]는 건너뜀.
        // 인프라(529)로 실패한 배치의 주제는 공백 유지 → 다음 라운드 재시도([x] 아님).
        let blank_ids: Vec<String> = ledger
            .topics
            .iter()
            .filter(|t| t.status == BLANK)
            .map(|t| t.id.clone())
            .collect();
        if !blank_ids.is_empty() {
            let chunk_size = if BATCH == 0 {
                blank_ids.len().max(1)
            } else {
                BATCH
            };
            let chunks: Vec<Vec<String>> =
                blank_ids.chunks(chunk_size).map(|c| c.to_vec()).collect();
            let conc = cfg.concurrency.max(1);
            // 배치들을 concurrency 크기의 웨이브로 동시 검증. 웨이브 사이만 쿨다운(웹서치 버스트 완화).
            for (wi, wave) in chunks.chunks(conc).enumerate() {
                if wi > 0 {
                    std::thread::sleep(Duration::from_secs(COOLDOWN));
                }
                let verifs: Vec<Vec<Verification>> = std::thread::scope(|s| {
                    let handles: Vec<_> = wave
                        .iter()
                        .map(|chunk| {
                            let batch: Vec<Topic> = ledger
                                .topics
                                .iter()
                                .filter(|t| chunk.contains(&t.id))
                                .cloned()
                                .collect();
                            let all = ledger.topics.clone();
                            s.spawn(move || exec_verify(directive, &batch, &all, cfg))
                        })
                        .collect();
                    handles
                        .into_iter()
                        .map(|h| match h.join() {
                            Ok(Ok(v)) => v,
                            Ok(Err(e)) => {
                                eprintln!(
                                    "  [batch 검증 실패(공백 유지·재시도): {}]",
                                    e.chars().take(100).collect::<String>()
                                );
                                vec![]
                            }
                            Err(_) => {
                                eprintln!("  [batch 패닉 — 공백 유지]");
                                vec![]
                            }
                        })
                        .collect()
                });
                for v in &verifs {
                    apply_verifications(&mut ledger.topics, v, round);
                }
                ledger.save()?;
            }
        }
        validate_ledger(&mut ledger.topics, round);
        ledger.save()?;

        // [f] = 크리티컬(자기모순·불가 전제·치명적 미검증) → 즉시 중단. hunt·decide 낭비 없이.
        if let Some(ft) = ledger.topics.iter().find(|t| t.status == FATAL) {
            aborted = true;
            abort_reason = format!(
                "[f] {}: {}",
                ft.id,
                ft.history
                    .last()
                    .map(|h| h.reason.clone())
                    .unwrap_or_default()
            );
            break;
        }

        // (B) 누락사냥: 모든 (A)배치 끝난 후 전체 원장에 1회(배치마다 X — 수율 낮아 중복 제거).
        // 공백이 0이어도 매 라운드 1회: 보강된 검증기능·바뀐 세계가 새 make-or-break(모순·이음매·깊이)를 잡게.
        let additions = exec_hunt(directive, &ledger.topics, cfg).unwrap_or_else(|e| {
            eprintln!(
                "  [누락사냥 실패(이번 라운드 추가 없음): {}]",
                e.chars().take(100).collect::<String>()
            );
            vec![]
        });

        // 결정기능: 승격 + 보완 + 결정.
        let jr = exec_decide(directive, &ledger.topics, &additions, cfg)?;
        apply_refines(&mut ledger.topics, &jr.refine, round);
        let promoted = apply_promotions(&mut ledger.topics, &jr.promote);
        ledger.save()?;

        let verified = ledger.topics.iter().filter(|t| t.status == OK).count();
        let failed = ledger.topics.iter().filter(|t| t.status == FAIL).count();
        log.push(RoundLog {
            round,
            verified,
            failed,
            promoted,
            decision: jr.decision.clone(),
            reason: jr.reason.chars().take(200).collect(),
        });

        match jr.decision.as_str() {
            "abort" => {
                aborted = true;
                abort_reason = jr.reason.clone();
                break;
            }
            "converge" => {
                converged = true;
                break;
            }
            _ => {}
        }
        // 자연 수렴: 공백 0 ∧ 새 승격 0.
        let still_blank = ledger.topics.iter().any(|t| t.status == BLANK);
        if !still_blank && promoted == 0 {
            converged = true;
            break;
        }
    }

    let spec = if aborted {
        String::new()
    } else {
        exec_render(directive, &ledger.topics, cfg).unwrap_or_default()
    };
    ledger.save()?;

    Ok(Outcome {
        directive: directive.into(),
        spec,
        topics: ledger.topics.clone(),
        log,
        converged,
        aborted,
        abort_reason,
        rounds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: &str, status: &str, last: Option<(&str, &str)>) -> Topic {
        let history = match last {
            Some((s, r)) => vec![Attempt {
                round: 1,
                status: s.into(),
                reason: r.into(),
                verified_value: String::new(),
            }],
            None => vec![],
        };
        Topic {
            id: id.into(),
            subject: "s".into(),
            origin: "agent".into(),
            status: status.into(),
            verified_value: String::new(),
            sources: vec![],
            version: 0,
            verify_count: 0,
            history,
        }
    }

    #[test]
    fn validate_consistent_ok() {
        let mut v = vec![t("a", OK, Some((OK, "확인")))];
        assert!(validate_ledger(&mut v, 2).is_empty());
        assert_eq!(v[0].status, OK);
    }

    #[test]
    fn validate_downgrades_mismatch() {
        // status=[o] 인데 history 마지막이 [x] → 강등.
        let mut v = vec![t("a", OK, Some((FAIL, "실패")))];
        let d = validate_ledger(&mut v, 3);
        assert_eq!(d, vec!["a".to_string()]);
        assert_eq!(v[0].status, FAIL);
        assert_eq!(v[0].history.last().unwrap().status, FAIL);
    }

    #[test]
    fn validate_blank_untouched() {
        let mut v = vec![t("a", BLANK, None)];
        assert!(validate_ledger(&mut v, 1).is_empty());
        assert_eq!(v[0].status, BLANK);
    }

    #[test]
    fn apply_verification_sets_status_and_value() {
        let mut v = vec![t("a", BLANK, None)];
        apply_verifications(
            &mut v,
            &[Verification {
                id: "a".into(),
                status: OK.into(),
                origin: "search".into(),
                reason: "ok".into(),
                verified_value: "X".into(),
                sources: vec!["u".into()],
            }],
            1,
        );
        assert_eq!(v[0].status, OK);
        assert_eq!(v[0].verified_value, "X");
        assert_eq!(v[0].history.len(), 1);
    }

    #[test]
    fn correction_bumps_version() {
        let mut v = vec![t("a", OK, Some((OK, "v1")))];
        v[0].verified_value = "old".into();
        // [o]는 안 건드린다 → 교정은 [x] 거쳐 공백→재검증 경로. 직접 재검증 시뮬:
        v[0].status = BLANK.into();
        apply_verifications(
            &mut v,
            &[Verification {
                id: "a".into(),
                status: OK.into(),
                origin: "agent".into(),
                reason: "정정".into(),
                verified_value: "new".into(),
                sources: vec![],
            }],
            2,
        );
        assert_eq!(v[0].version, 1);
        assert_eq!(v[0].verified_value, "new");
    }

    #[test]
    fn promotions_dedupe() {
        let mut v = vec![t("a", OK, Some((OK, "x")))];
        let n = apply_promotions(
            &mut v,
            &[
                Promote {
                    id: "a".into(),
                    subject: "dup".into(),
                    origin: "agent".into(),
                },
                Promote {
                    id: "b".into(),
                    subject: "new".into(),
                    origin: "search".into(),
                },
            ],
        );
        assert_eq!(n, 1); // a 중복 → b 만.
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn refine_returns_to_blank() {
        let mut v = vec![t("a", FAIL, Some((FAIL, "fail")))];
        apply_refines(
            &mut v,
            &[Refine {
                id: "a".into(),
                new_subject: "better".into(),
                reason: "보완".into(),
            }],
            2,
        );
        assert_eq!(v[0].status, BLANK);
        assert_eq!(v[0].subject, "better");
    }
}
