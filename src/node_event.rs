//! node_event — 보드 노드 발행 이벤트(NodeEvent) wire 정본.
//!
//! doc_interp(publish op)가 이 이벤트를 생산하고, stdout JSON line → 서비스 relay → 보드 계약의
//! node.add 로 흐른다. 실행 경로는 이 wire에만 의존한다.
//!
//! 규칙(rule_workflow_pipeline_node_model):
//! - 규칙 A: 노드는 작업이다. parallel/pipeline 관계는 blockedBy 데이터로 표현 — 노드가 아니다.
//! - 규칙 B: title(요건명) / description(사람용 설명) / body(exec 입력) 세 축 분리.
//! - 규칙 C: 발행은 실행이 아니다 — 실행은 코어 스케줄러(reconcile → exec-one/exec-stage).
use serde_json::Value as Json;

/// 발행되는 보드 노드 이벤트(→ JSON line → 서비스 relay → 발견된 구현체의 node.add).
/// 발행 전용: Add 만(실행 lifecycle status 없음 — 실행은 스케줄러+exec-one 의 몫).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "ev", rename_all = "lowercase")]
pub enum NodeEvent {
    Add {
        id: String,
        parent: Option<String>,
        // node kind — draft 모델: "chunk"(덩어리·isDraft) | "item"(요건·badge) | "task"(stage 작업).
        // research 모델: "fact"(기초지식·badge) | "plan-unit"(슈도코드 단위). 서비스가 kind 로 처리를 가른다.
        kind: String,
        title: String,
        description: String, // 규칙 B: 사람용 설명(칸반 description 필드). exec 입력 아님 — body 와 별개 축.
        #[serde(skip_serializing_if = "String::is_empty")]
        prompt: String, // agent 프롬프트 인라인 통로. 정규화 item·task 는 빈 문자열이면 직렬화하지 않는다.
        // task 노드의 stage — 서비스 relay 가 exec-stage body 에 임베드. 일반 노드는 생략.
        #[serde(skip_serializing_if = "Option::is_none")]
        stage: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        schema: Option<Json>, // 구조화 출력 계약(exec-one 용). 없으면 raw 텍스트 agent.
        #[serde(skip_serializing_if = "Option::is_none")]
        category: Option<String>, // 의미 분류 라벨(classify 사후 부여·fact 의 area).
        #[serde(skip_serializing_if = "Option::is_none")]
        origin: Option<String>, // 출처(user/agent/search) — 규칙 D 출처 추적의 본질 메타.
        // ── 프롬프트 정규화(콘텐츠 주소화) 통로 — Rust 는 해시 모름, 텍스트/role 만 relay. 서비스가 sha256·치환.
        #[serde(skip_serializing_if = "Option::is_none")]
        prompt_role: Option<String>, // item/fact: 논리 role(verify/fact-verify). 서비스 relay 가 role→promptHash 치환.
        #[serde(skip_serializing_if = "Option::is_none")]
        vars: Option<Json>, // {{key}} 바인딩 변수(작은 값만: title/description). 소비 시점 조립.
        #[serde(skip_serializing_if = "Option::is_none")]
        register_prompts: Option<Json>, // run 당 1회: {role: 텍스트}. 서비스가 prompt.put(sha256 dedup).
        #[serde(skip_serializing_if = "Option::is_none")]
        var_refs: Option<Json>, // {{key}} → 등록 role 라벨. 큰 공유값(directive) 콘텐츠 주소 참조(복붙 X).
        #[serde(skip_serializing_if = "Option::is_none")]
        schema_ref: Option<String>, // 출력 schema 의 등록 role 라벨 → schemaHash(전역 1행 참조).
        #[serde(skip_serializing_if = "Vec::is_empty")]
        blocked_by: Vec<String>,
        // 칸반 드래프트 계약: 항목=badge("검수전"), 덩어리=is_draft, 계보=parent_draft_id.
        // 마커는 드래프트 노드에만 — 일반 노드엔 생략(보드 오염 방지).
        #[serde(skip_serializing_if = "Option::is_none")]
        badge: Option<String>,
        #[serde(skip_serializing_if = "is_false")]
        is_draft: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_draft_id: Option<String>,
        // ── 라우팅 tier(자기선택): 저작이 노드 난이도로 실어 보낸다. reconcile 이 wire 를 읽어 exec 에
        // honor(claude --effort, codex -c model_reasoning_effort). 미emit = 실행자 기본(최고, 품질우선). routing-skill.md.
        #[serde(skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
}

/// serde skip_serializing_if(bool) — false 면 직렬화 생략(일반 노드에 is_draft:false 안 새게).
fn is_false(b: &bool) -> bool {
    !*b
}
