//! draft_doc — generate stage 산출(평탄 NodeEvent 스트림)을 **id 기반 정규형 문서(DraftDoc)** 로 접고,
//! 순수 validator 로 인증한다. 공유값은 *변하는 수준에서 1회*,
//! 관계는 *id 참조*, 소비 시점(exec-one) 조립. validator 통과 못하면 발행 거부(fail-loud).
//!
//! **sha 는 넣지 않는다.** 공유값(template/directive/schema)은 verify_contract에 한 번 넣고,
//! relay가 보드 계약의 prompt.put으로 콘텐츠 주소화한다.
//! 여기 규칙 6(콘텐츠 주소 정합)은 담당하지 않는다 — 규칙 1~5,7 만.
use crate::emit_host::NodeEvent;
use serde_json::Value as Json;

/// DraftDoc — generate stage 의 id 기반 정규형. 요건은 고유 필드만, 공유값은 verify_contract 1회.
/// **평탄** — generate 는 category/그룹을 만들지 않는다(CHUNK_REF 직속 flat item). 분류는 별도 classify
/// stage 가 완성 원장(hunt 후) 보고 kanban node.edit(category) 로 부여 — DraftDoc 밖.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DraftDoc {
    pub kind: String,      // 항상 "draft-chunk"
    pub chunk_ref: String, // 기존 청크 kanban id(generate 산출이 붙는 덩어리)
    // 워크플로 return {chunkTitle} — 덩어리 title 갱신용(relay 가 chunk_ref 노드 title 에 적용). 없으면 생략.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub chunk_title: Option<String>,
    pub verify_contract: VerifyContract,
    pub requirements: Vec<Requirement>,
    pub tasks: Vec<Task>,
}

/// 전 요건 공유 계약 — 공유값 inline 1회(sha 아님). relay가 prompt.put으로 주소화한다.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VerifyContract {
    pub template: String,      // verify 프롬프트 템플릿(전역 공유)
    pub directive: String,     // 이 청크의 지시어(청크당 1회)
    pub schema: Json,          // oxf 출력 계약(전역 공유)
    pub initial_badge: String, // 요건 최초 배지("검수전")
}

/// 요건(item 이벤트) — 고유 필드만. 공유값(template/schema/directive)은 인라인 0.
/// **평탄** — category 없음(분류는 classify stage 가 나중에 node.edit 로). parent = CHUNK_REF(덩어리 직속).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Requirement {
    pub id: String,
    pub title: String,
    pub description: String,
    pub origin: String, // user|agent|search
    pub badge: String,
    // 라우팅 tier(자기선택) — 저작이 요건별 검증 난이도로 실어 보낸다. apply_draft_doc 이 item 노드에
    // 실어 exec 이 honor. 미지정 = 실행자 기본(최고, 품질우선). routing-skill.md.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub model: Option<String>,
}

/// stage 작업(task 이벤트: hunt/classify/audit) — id + blockedBy(id 참조).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Task {
    pub id: String,
    pub stage: String,
    pub blocked_by: Vec<String>,
}

/// build — 평탄 NodeEvent 스트림 → DraftDoc(정규형).
/// **평탄** — generate 는 그룹을 내지 않는다(item = CHUNK_REF 직속). 방어적: register_prompts 없거나 필드 순서 달라도 견고하게 접는다.
/// - verify_contract: 첫 item 의 register_prompts{verify,directive}(+schema) → template/directive. schema 는
///   register_prompts.schema 우선, 없으면 첫 item 의 inline schema(폴백). initial_badge = 첫 item badge.
/// - requirements: item 이벤트(고유 필드만; category 없음 — 분류는 classify stage 가 나중에).
/// - tasks: task 이벤트(hunt/classify/audit; stage + blocked_by).
/// - chunk_ref: 첫 item 의 parent(= 기존 덩어리 id). item 없으면 첫 task 의 parent, 그것도 없으면 "chunk".
pub fn build(events: &[NodeEvent]) -> Result<DraftDoc, String> {
    let mut requirements: Vec<Requirement> = vec![];
    let mut tasks: Vec<Task> = vec![];

    // verify_contract 재료 — 첫 item 의 register_prompts + 첫 item inline schema 폴백.
    let mut template: Option<String> = None;
    let mut directive: Option<String> = None;
    let mut schema: Option<Json> = None;
    let mut initial_badge: Option<String> = None;
    let mut item_schema_fallback: Option<Json> = None;

    // chunk_ref 유추 재료: 첫 item 의 parent, 첫 task 의 parent(둘 다 없으면 "chunk").
    let mut first_item_parent: Option<String> = None;
    let mut first_task_parent: Option<String> = None;

    for ev in events {
        let NodeEvent::Add {
            id,
            parent,
            kind,
            title,
            description,
            origin,
            badge,
            register_prompts,
            schema: ev_schema,
            stage,
            blocked_by,
            effort,
            model,
            ..
        } = ev;
        match kind.as_str() {
            "item" => {
                if first_item_parent.is_none() {
                    first_item_parent = parent.clone().filter(|s| !s.is_empty());
                }
                if initial_badge.is_none() {
                    initial_badge = badge.clone();
                }
                if item_schema_fallback.is_none() {
                    if let Some(s) = ev_schema {
                        if s.is_object() {
                            item_schema_fallback = Some(s.clone());
                        }
                    }
                }
                // 첫 item에만 register_prompts 공유값을 등록한다.
                if let Some(Json::Object(m)) = register_prompts {
                    if template.is_none() {
                        if let Some(Json::String(t)) = m.get("verify") {
                            template = Some(t.clone());
                        }
                    }
                    if directive.is_none() {
                        if let Some(Json::String(d)) = m.get("directive") {
                            directive = Some(d.clone());
                        }
                    }
                    if schema.is_none() {
                        if let Some(s) = m.get("schema") {
                            if s.is_object() {
                                schema = Some(s.clone());
                            }
                        }
                    }
                }
                requirements.push(Requirement {
                    id: id.clone(),
                    title: title.clone(),
                    description: description.clone(),
                    origin: origin.clone().unwrap_or_default(),
                    badge: badge.clone().unwrap_or_default(),
                    effort: effort.clone(),
                    model: model.clone(),
                });
            }
            "task" => {
                if first_task_parent.is_none() {
                    first_task_parent = parent.clone().filter(|s| !s.is_empty());
                }
                tasks.push(Task {
                    id: id.clone(),
                    stage: stage.clone().unwrap_or_default(),
                    blocked_by: blocked_by.clone(),
                });
            }
            // chunk/group/phase/agent 등은 generate 정규형에 편입 안 함(방어적으로 무시 — 평탄엔 group 없음).
            _ => {}
        }
    }

    // chunk_ref: 첫 item 의 parent(= 덩어리 id, 보통 "chunk"). item 없으면 첫 task 의 parent, 그것도 없으면 "chunk".
    let chunk_ref = first_item_parent
        .or(first_task_parent)
        .unwrap_or_else(|| "chunk".to_string());

    let verify_contract = VerifyContract {
        template: template.unwrap_or_default(),
        directive: directive.unwrap_or_default(),
        // schema: register_prompts.schema 우선, 없으면 item inline schema, 그것도 없으면 null.
        schema: schema.or(item_schema_fallback).unwrap_or(Json::Null),
        initial_badge: initial_badge.unwrap_or_else(|| "검수전".to_string()),
    };

    Ok(DraftDoc {
        kind: "draft-chunk".to_string(),
        chunk_ref,
        chunk_title: None, // main.rs 가 워크플로 return {chunkTitle} 로 채운다.
        verify_contract,
        requirements,
        tasks,
    })
}

/// validate — DraftDoc 인증(플랜 규칙 1,2,3,4,5,7 + ⑧⑨ — 규칙 6 sha 정합은 kanban 담당). 위반 목록 반환.
/// **평탄** — category 개념 제거(그룹 없음, requirement.category_id 없음). 분류는 classify stage 가 나중에 node.edit.
/// 통과(빈 위반) 못하면 발행 거부(fail-loud). 규칙:
///   ① id 유일 — requirements ∪ tasks 전 id 유일.
///   ② FK — task.blocked_by ∈ requirements ∪ tasks. (category FK 규칙 제거 — 평탄.)
///   ③ 완결 — 요건마다 title·description 비지 않음 · origin ∈ {user,agent,search}.
///   ④ 정규화 불변 — 요건에 schema/directive/template/category 이름 인라인 0(고유 필드만; 구조로 보장).
///   ⑤ 트리 — hunt.blocked_by = 전 요건 · classify.blocked_by = 전 요건 ∪ {hunt} · audit.blocked_by = 전 요건 ∪ {hunt,classify}.
///   ⑦ 비어있지 않음 — requirements ≥ 1. (categories≥1 규칙 제거 — 평탄.)
///   ⑧ 배지 enum — requirement.badge ∈ {검수전,o,x,f}(빈 값은 initial_badge 폴백 — 허용) · initial_badge ∈ enum.
///     비enum 배지는 칸반이 드랍해 "검수전 필터에도 done 판정에도 안 걸리는" 영구 not-done 무음 정지가 된다.
///   ⑨ verify_contract 완결 — template·directive 비지 않음 · schema 객체. 빈 계약(registerPrompts 누락)은
///     item body 가 해시 없이 발행돼 exec-one 'prompt 필수' 무한 backoff(지연 실패) — 발행 시점 거부가 fail-loud.
pub fn validate(doc: &DraftDoc) -> Result<(), Vec<String>> {
    let mut v: Vec<String> = vec![];

    // ⑦ 비어있지 않음 — requirements ≥ 1.
    if doc.requirements.is_empty() {
        v.push("[⑦] requirements 비어있음(≥1 필요)".to_string());
    }

    // ① id 유일 — requirements ∪ tasks.
    let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    let all_ids = doc
        .requirements
        .iter()
        .map(|r| r.id.as_str())
        .chain(doc.tasks.iter().map(|t| t.id.as_str()));
    for id in all_ids {
        if !seen.insert(id) {
            v.push(format!("[①] id 중복: {id:?}"));
        }
    }

    // ② FK — task.blocked_by ∈ requirements ∪ tasks.
    let ref_targets: std::collections::BTreeSet<&str> = doc
        .requirements
        .iter()
        .map(|r| r.id.as_str())
        .chain(doc.tasks.iter().map(|t| t.id.as_str()))
        .collect();
    for t in &doc.tasks {
        for b in &t.blocked_by {
            if !ref_targets.contains(b.as_str()) {
                v.push(format!(
                    "[②] task {:?} blocked_by {:?} 미존재(FK 위반)",
                    t.id, b
                ));
            }
        }
    }

    // ③ 완결 — 요건마다 title·description 비지 않음 · origin ∈ {user,agent,search}.
    const ORIGINS: [&str; 3] = ["user", "agent", "search"];
    for r in &doc.requirements {
        if r.title.trim().is_empty() {
            v.push(format!("[③] requirement {:?} title 비어있음", r.id));
        }
        if r.description.trim().is_empty() {
            v.push(format!("[③] requirement {:?} description 비어있음", r.id));
        }
        if !ORIGINS.contains(&r.origin.as_str()) {
            v.push(format!(
                "[③] requirement {:?} origin {:?} ∉ {{user,agent,search}}",
                r.id, r.origin
            ));
        }
    }

    // ⑤ 트리 — hunt.blocked_by = 전 요건 · classify.blocked_by = 전 요건 ∪ {hunt} · audit.blocked_by = 전 요건 ∪ {hunt,classify}.
    // 각 stage task 가 존재할 때만 검사(hunt-단독 재실행 등 부분 문서는 일부 task 만 있을 수 있음).
    let req_ids: std::collections::BTreeSet<&str> =
        doc.requirements.iter().map(|r| r.id.as_str()).collect();
    let hunt_id = doc
        .tasks
        .iter()
        .find(|t| t.stage == "hunt")
        .map(|t| t.id.as_str());
    let classify_id = doc
        .tasks
        .iter()
        .find(|t| t.stage == "classify")
        .map(|t| t.id.as_str());
    if let Some(hunt) = doc.tasks.iter().find(|t| t.stage == "hunt") {
        let hb: std::collections::BTreeSet<&str> =
            hunt.blocked_by.iter().map(|s| s.as_str()).collect();
        if hb != req_ids {
            v.push("[⑤] hunt.blocked_by ≠ 전 요건 id 집합".to_string());
        }
    }
    if let Some(classify) = doc.tasks.iter().find(|t| t.stage == "classify") {
        let cb: std::collections::BTreeSet<&str> =
            classify.blocked_by.iter().map(|s| s.as_str()).collect();
        // 기대 = 전 요건 ∪ {hunt}(hunt 존재 시).
        let mut expected: std::collections::BTreeSet<&str> = req_ids.clone();
        if let Some(hid) = hunt_id {
            expected.insert(hid);
        }
        if hunt_id.is_none() {
            v.push("[⑤] classify 존재하나 hunt task 부재(분류는 hunt 후행)".to_string());
        } else if cb != expected {
            v.push("[⑤] classify.blocked_by ≠ 전 요건 ∪ {hunt}".to_string());
        }
    }
    if let Some(audit) = doc.tasks.iter().find(|t| t.stage == "audit") {
        let ab: std::collections::BTreeSet<&str> =
            audit.blocked_by.iter().map(|s| s.as_str()).collect();
        // 기대 = 전 요건 ∪ {hunt task id, classify task id}(각각 존재 시).
        let mut expected: std::collections::BTreeSet<&str> = req_ids.clone();
        if let Some(hid) = hunt_id {
            expected.insert(hid);
        }
        if let Some(cid) = classify_id {
            expected.insert(cid);
        }
        if hunt_id.is_none() {
            v.push("[⑤] audit 존재하나 hunt task 부재(감사는 hunt 후행)".to_string());
        } else if ab != expected {
            v.push("[⑤] audit.blocked_by ≠ 전 요건 ∪ {hunt,classify}".to_string());
        }
    }

    // ⑧ 배지 enum — LLM 저작 리터럴이라 이탈 가능(origin 은 ③이 검사하는데 badge 만 공백이면 비대칭).
    const BADGES: [&str; 4] = ["검수전", "o", "x", "f"];
    for r in &doc.requirements {
        if !r.badge.is_empty() && !BADGES.contains(&r.badge.as_str()) {
            v.push(format!(
                "[⑧] requirement {:?} badge {:?} ∉ {{검수전,o,x,f}}",
                r.id, r.badge
            ));
        }
    }
    if !BADGES.contains(&doc.verify_contract.initial_badge.as_str()) {
        v.push(format!(
            "[⑧] verify_contract.initial_badge {:?} ∉ {{검수전,o,x,f}}",
            doc.verify_contract.initial_badge
        ));
    }

    // ⑨ verify_contract 완결 — build 는 부재를 빈 값으로 접지만(total), 발행은 여기서 거부한다.
    if doc.verify_contract.template.trim().is_empty() {
        v.push("[⑨] verify_contract.template 비어있음(registerPrompts.verify 누락)".to_string());
    }
    if doc.verify_contract.directive.trim().is_empty() {
        v.push(
            "[⑨] verify_contract.directive 비어있음(registerPrompts.directive 누락)".to_string(),
        );
    }
    if !doc.verify_contract.schema.is_object() {
        v.push(
            "[⑨] verify_contract.schema 객체 아님(registerPrompts.schema/item inline 부재)"
                .to_string(),
        );
    }

    // 규칙 ④(정규화 불변)는 Requirement 구조 자체가 고유 필드만 갖게 강제 — 인라인 슬롯이 없다.
    // (schema/directive/template/category 필드가 struct 에 존재하지 않음 → 구조로 보장.)

    if v.is_empty() {
        Ok(())
    } else {
        Err(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// item Add 이벤트(평탄: parent=CHUNK_REF 직속, category 없음, prompt '' + schema inline 폴백).
    /// register_prompts는 첫 item에만 얹는다. None이면 미등록 item이다.
    fn item_ev(
        id: &str,
        parent: &str,
        title: &str,
        desc: &str,
        origin: &str,
        badge: &str,
        schema: Option<Json>,
        register: Option<Json>,
    ) -> NodeEvent {
        NodeEvent::Add {
            id: id.into(),
            parent: Some(parent.into()),
            kind: "item".into(),
            title: title.into(),
            description: desc.into(),
            prompt: String::new(),
            stage: None,
            schema,
            category: None,
            origin: Some(origin.into()),
            prompt_role: Some("verify".into()),
            vars: Some(json!({ "title": title })),
            register_prompts: register,
            var_refs: Some(json!({ "directive": "directive" })),
            schema_ref: None,
            blocked_by: vec![],
            badge: Some(badge.into()),
            is_draft: false,
            parent_draft_id: None,
            effort: None,
            model: None,
        }
    }
    /// task Add 이벤트(hunt/classify/audit).
    fn task_ev(id: &str, parent: &str, stage: &str, blocked_by: &[&str]) -> NodeEvent {
        NodeEvent::Add {
            id: id.into(),
            parent: Some(parent.into()),
            kind: "task".into(),
            title: stage.into(),
            description: String::new(),
            prompt: String::new(),
            stage: Some(stage.into()),
            schema: None,
            category: None,
            origin: None,
            prompt_role: None,
            vars: None,
            register_prompts: None,
            var_refs: None,
            schema_ref: None,
            blocked_by: blocked_by.iter().map(|s| s.to_string()).collect(),
            badge: None,
            is_draft: false,
            parent_draft_id: None,
            effort: None,
            model: None,
        }
    }
    fn schema_json() -> Json {
        json!({ "type": "object", "required": ["oxf", "origin"], "properties": { "oxf": { "type": "string" } } })
    }

    /// 정상 generate 이벤트 스트림(평탄: item = CHUNK_REF 직속, register_prompts 는 첫 item, task 3개 hunt→classify→audit).
    fn good_events() -> Vec<NodeEvent> {
        let register = json!({ "verify": "VERIFY_TMPL {{title}} {{directive}}", "directive": "약국 SaaS 지시어", "schema": schema_json() });
        vec![
            item_ev(
                "i0",
                "chunk",
                "재고 차감",
                "판매 시 차감",
                "user",
                "검수전",
                Some(schema_json()),
                Some(register),
            ),
            item_ev(
                "i1",
                "chunk",
                "유통기한 경고",
                "만료 임박 알림",
                "agent",
                "검수전",
                Some(schema_json()),
                None,
            ),
            task_ev("hunt", "chunk", "hunt", &["i0", "i1"]),
            task_ev("classify", "chunk", "classify", &["i0", "i1", "hunt"]),
            task_ev("audit", "chunk", "audit", &["i0", "i1", "hunt", "classify"]),
        ]
    }

    #[test]
    fn build_folds_flat_events_into_normalized_doc() {
        let doc = build(&good_events()).unwrap();
        assert_eq!(doc.kind, "draft-chunk");
        assert_eq!(
            doc.chunk_ref, "chunk",
            "chunk_ref = 첫 item 의 parent(덩어리 id)"
        );
        assert_eq!(doc.requirements.len(), 2);
        assert_eq!(doc.requirements[0].id, "i0");
        assert_eq!(doc.requirements[0].origin, "user");
        assert_eq!(doc.tasks.len(), 3, "hunt+classify+audit");
    }

    #[test]
    fn build_carries_routing_tier_to_requirement() {
        // 저작이 실은 노드 tier(effort/model)가 NodeEvent→DraftDoc 요건까지 관통해야 apply_draft_doc 이
        // item 노드에 싣고 exec 이 honor. 여기서 끊기면 draft(주 워크플로) 경로 라우팅이 무음 no-op.
        let mut ev = item_ev(
            "i0",
            "chunk",
            "auth 경계",
            "d",
            "agent",
            "검수전",
            Some(schema_json()),
            Some(json!({ "verify": "T {{title}}", "directive": "D" })),
        );
        let NodeEvent::Add { effort, model, .. } = &mut ev;
        *effort = Some("max".into());
        *model = Some("gpt-5.6-sol".into());
        let doc = build(&[ev]).unwrap();
        let w = serde_json::to_string(&doc.requirements[0]).unwrap();
        assert!(
            w.contains(r#""effort":"max""#),
            "tier 가 요건까지 관통: {w}"
        );
        assert!(
            w.contains(r#""model":"gpt-5.6-sol""#),
            "model tier 관통: {w}"
        );
        // tier 미지정 요건 = 직렬화에 effort 키 없음(기본 최고 보존, 군더더기 0).
        let plain = serde_json::to_string(&build(&good_events()).unwrap().requirements[0]).unwrap();
        assert!(
            !plain.contains("\"effort\""),
            "미지정 = effort 생략: {plain}"
        );
    }

    #[test]
    fn build_extracts_verify_contract_from_first_item_register_prompts() {
        let doc = build(&good_events()).unwrap();
        assert_eq!(
            doc.verify_contract.template,
            "VERIFY_TMPL {{title}} {{directive}}"
        );
        assert_eq!(doc.verify_contract.directive, "약국 SaaS 지시어");
        assert_eq!(
            doc.verify_contract.schema,
            schema_json(),
            "schema = register_prompts.schema"
        );
        assert_eq!(doc.verify_contract.initial_badge, "검수전");
    }

    #[test]
    fn build_falls_back_to_item_inline_schema_when_register_lacks_schema() {
        // register_prompts 에 schema 없고 item 이 inline schema 보유 → 폴백.
        let register = json!({ "verify": "T {{title}}", "directive": "D" });
        let events = vec![item_ev(
            "i0",
            "chunk",
            "요건",
            "설명",
            "user",
            "검수전",
            Some(schema_json()),
            Some(register),
        )];
        let doc = build(&events).unwrap();
        assert_eq!(
            doc.verify_contract.schema,
            schema_json(),
            "register.schema 부재 시 item inline schema 폴백"
        );
    }

    #[test]
    fn build_defensive_no_register_prompts_then_validate_rejects() {
        // register_prompts 전무 — build 는 견고하게 접지만(total function), 빈 계약을 **발행하면 안 된다**:
        // item body 가 해시 없이 나가 exec-one 'prompt 필수' 무한 backoff(지연 실패)가 되므로 ⑨가 발행 시점에 거부.
        let events = vec![item_ev(
            "i0",
            "chunk",
            "요건",
            "설명",
            "user",
            "검수전",
            None,
            None,
        )];
        let doc = build(&events).unwrap();
        assert_eq!(doc.verify_contract.template, "");
        assert_eq!(doc.verify_contract.directive, "");
        assert_eq!(doc.verify_contract.schema, Json::Null);
        assert_eq!(doc.chunk_ref, "chunk", "chunk_ref = 첫 item 의 parent");
        assert_eq!(doc.requirements.len(), 1);
        let errs = validate(&doc).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.contains("[⑨]") && e.contains("template")),
            "빈 template 거부: {errs:?}"
        );
        assert!(
            errs.iter()
                .any(|e| e.contains("[⑨]") && e.contains("directive")),
            "빈 directive 거부: {errs:?}"
        );
        assert!(
            errs.iter()
                .any(|e| e.contains("[⑨]") && e.contains("schema")),
            "schema Null 거부: {errs:?}"
        );
    }

    #[test]
    fn validate_rule8_rejects_non_enum_badge() {
        // LLM 이 badge:"pending" 을 내면 종전엔 통과 → 칸반이 드랍 → 영구 not-done 무음 정지. ⑧이 발행 시점 거부.
        let mut doc = build(&good_events()).unwrap();
        doc.requirements[0].badge = "pending".to_string();
        let errs = validate(&doc).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.contains("[⑧]") && e.contains("pending")),
            "비enum badge 거부: {errs:?}"
        );
    }

    #[test]
    fn validate_rule8_allows_empty_badge_falls_back_to_initial() {
        // 빈 badge 는 applyDraftDoc 이 initial_badge 로 폴백 — 위반 아님.
        let mut doc = build(&good_events()).unwrap();
        doc.requirements[0].badge = String::new();
        assert_eq!(
            validate(&doc),
            Ok(()),
            "빈 badge 는 initial_badge 폴백(허용)"
        );
    }

    #[test]
    fn validate_rule8_rejects_non_enum_initial_badge() {
        let mut doc = build(&good_events()).unwrap();
        doc.verify_contract.initial_badge = "todo".to_string();
        let errs = validate(&doc).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.contains("[⑧]") && e.contains("initial_badge")),
            "비enum initial_badge 거부: {errs:?}"
        );
    }

    #[test]
    fn build_requirement_has_only_unique_fields_no_inline_shared() {
        // 정규화 불변(④): 요건은 고유 필드만 — Requirement struct 에 schema/directive/template/category 슬롯이 없다(평탄: category_id 도 없음).
        let doc = build(&good_events()).unwrap();
        let r0 = serde_json::to_value(&doc.requirements[0]).unwrap();
        let obj = r0.as_object().unwrap();
        for k in [
            "schema",
            "directive",
            "template",
            "prompt",
            "category",
            "category_id",
            "name",
        ] {
            assert!(
                !obj.contains_key(k),
                "요건에 공유값·분류 슬롯 {k:?} 있음(정규화·평탄 위반)"
            );
        }
        assert!(
            obj.contains_key("id") && obj.contains_key("title"),
            "요건은 고유 필드만(id/title/description/origin/badge)"
        );
    }

    #[test]
    fn validate_accepts_good_doc() {
        let doc = build(&good_events()).unwrap();
        assert_eq!(validate(&doc), Ok(()), "정상 문서는 통과");
    }

    // ── 규칙별 위반 fixture (RED→GREEN) ──

    #[test]
    fn validate_rule1_rejects_duplicate_id() {
        let mut doc = build(&good_events()).unwrap();
        doc.requirements[1].id = "i0".to_string(); // 중복
        let errs = validate(&doc).unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("[①]")),
            "id 중복 위반 검출: {errs:?}"
        );
    }

    #[test]
    fn validate_rule2_rejects_dangling_blocked_by() {
        let mut doc = build(&good_events()).unwrap();
        doc.tasks[0].blocked_by = vec!["nope".to_string()]; // 미존재 참조
        let errs = validate(&doc).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.contains("[②]") && e.contains("blocked_by")),
            "blocked_by FK 위반: {errs:?}"
        );
    }

    #[test]
    fn validate_rule3_rejects_empty_title_or_description() {
        let mut doc = build(&good_events()).unwrap();
        doc.requirements[0].title = "".to_string();
        doc.requirements[1].description = "  ".to_string();
        let errs = validate(&doc).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.contains("[③]") && e.contains("title")),
            "빈 title 검출: {errs:?}"
        );
        assert!(
            errs.iter()
                .any(|e| e.contains("[③]") && e.contains("description")),
            "빈 description 검출: {errs:?}"
        );
    }

    #[test]
    fn validate_rule3_rejects_bad_origin() {
        let mut doc = build(&good_events()).unwrap();
        doc.requirements[0].origin = "made-up".to_string();
        let errs = validate(&doc).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.contains("[③]") && e.contains("origin")),
            "origin enum 위반: {errs:?}"
        );
    }

    #[test]
    fn validate_rule5_rejects_wrong_hunt_blocked_by() {
        let mut doc = build(&good_events()).unwrap();
        // hunt 이 전 요건이 아닌 일부만 blockedBy — 트리 무결성 위반.
        doc.tasks
            .iter_mut()
            .find(|t| t.stage == "hunt")
            .unwrap()
            .blocked_by = vec!["i0".to_string()];
        let errs = validate(&doc).unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("[⑤]") && e.contains("hunt")),
            "hunt 트리 위반: {errs:?}"
        );
    }

    #[test]
    fn validate_rule5_rejects_wrong_classify_blocked_by() {
        let mut doc = build(&good_events()).unwrap();
        // classify 가 hunt 를 빠뜨림(전 요건만) — 분류는 hunt 후행이어야.
        doc.tasks
            .iter_mut()
            .find(|t| t.stage == "classify")
            .unwrap()
            .blocked_by = vec!["i0".to_string(), "i1".to_string()];
        let errs = validate(&doc).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.contains("[⑤]") && e.contains("classify")),
            "classify 트리 위반: {errs:?}"
        );
    }

    #[test]
    fn validate_rule5_rejects_wrong_audit_blocked_by() {
        let mut doc = build(&good_events()).unwrap();
        // audit 이 classify 를 빠뜨림.
        doc.tasks
            .iter_mut()
            .find(|t| t.stage == "audit")
            .unwrap()
            .blocked_by = vec!["i0".to_string(), "i1".to_string(), "hunt".to_string()];
        let errs = validate(&doc).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.contains("[⑤]") && e.contains("audit")),
            "audit 트리 위반: {errs:?}"
        );
    }

    #[test]
    fn validate_rule7_rejects_empty_requirements() {
        let mut doc = build(&good_events()).unwrap();
        doc.requirements.clear();
        doc.tasks.clear();
        let errs = validate(&doc).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.contains("[⑦]") && e.contains("requirements")),
            "빈 requirements: {errs:?}"
        );
    }

    #[test]
    fn draft_doc_round_trips_json() {
        // serde round-trip으로 wire 형태를 고정한다.
        let doc = build(&good_events()).unwrap();
        let s = serde_json::to_string(&doc).unwrap();
        let back: DraftDoc = serde_json::from_str(&s).unwrap();
        assert_eq!(doc, back);
        // 평탄: 직렬화에 categories 필드 없음(군더더기 0).
        assert!(
            !s.contains("categories"),
            "평탄 DraftDoc 에 categories 필드 없음"
        );
    }

    #[test]
    fn chunk_title_serializes_only_when_set() {
        let mut doc = build(&good_events()).unwrap();
        let s0 = serde_json::to_string(&doc).unwrap();
        assert!(
            !s0.contains("chunk_title"),
            "미설정 시 chunk_title 생략(군더더기 0)"
        );
        doc.chunk_title = Some("약국 재고 SaaS".to_string());
        let s1 = serde_json::to_string(&doc).unwrap();
        assert!(
            s1.contains("\"chunk_title\":\"약국 재고 SaaS\""),
            "설정 시 직렬화"
        );
        let back: DraftDoc = serde_json::from_str(&s1).unwrap();
        assert_eq!(back.chunk_title.as_deref(), Some("약국 재고 SaaS"));
    }

    /// [통합] 실측 fixture doc(gen.pharmacy.doc.json)의 generate stage 를 doc_exec(stub agent)로 돌린
    /// 이벤트 → build → validate 가 정규형 인증까지 통과하는지. LLM·앱 없이 결정적으로.
    ///
    /// fixture 는 **평탄 계약**(classify-late): generate 는 tree.requirements(평탄)를 CHUNK_REF 직속 item 으로
    /// 발행 + hunt/classify/audit 3 task, register_prompts 는 첫 item 에.
    #[test]
    fn build_and_validate_from_fixture_generate_events() {
        let wdoc: Json =
            serde_json::from_str(include_str!("../fixtures/gen.pharmacy.doc.json")).unwrap();
        let mut agent = |_p: &str, _s: Option<&Json>, _l: &str| -> Result<Json, String> {
            Ok(
                json!({ "title": "테스트 덩어리", "titleOrigin": "agent", "requirements": [
                { "title": "항목1", "description": "설명1", "origin": "user" },
                { "title": "항목2", "description": "설명2", "origin": "agent" }
            ] }),
            )
        };
        let args = json!({ "stage": "generate", "directive": "테스트 지시", "chunkRef": "chunk" });
        let (events, _result) =
            crate::doc_exec::run(&wdoc, "generate", &args, &mut agent).expect("doc generate");

        let doc = build(&events).expect("build");
        // 평탄: item 2개(CHUNK_REF 직속, category 없음), hunt+classify+audit 3 task.
        assert_eq!(
            doc.requirements.len(),
            2,
            "평탄 요건 2개(그룹 없이 CHUNK_REF 직속)"
        );
        assert_eq!(doc.tasks.len(), 3, "hunt+classify+audit");
        // verify_contract: register_prompts 를 첫 item 에 얹음 → template/directive 채워짐. schema 는 register 또는 item inline 폴백.
        assert!(
            doc.verify_contract.schema.is_object(),
            "schema 채워짐(register 또는 item inline)"
        );
        assert!(
            !doc.verify_contract.template.is_empty(),
            "verify 템플릿 등록됨(첫 item)"
        );
        assert_eq!(
            doc.verify_contract.directive, "테스트 지시",
            "directive 등록됨(첫 item)"
        );
        // validate 통과 — 평탄 인증(hunt=전 요건, classify=전 요건∪{hunt}, audit=전 요건∪{hunt,classify} 트리).
        assert_eq!(validate(&doc), Ok(()), "평탄 generate 산출 검증 통과");
    }
}
