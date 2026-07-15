//! DraftDoc 발행과 prompt 템플릿 등록.
//! generate stage 산출(DraftDoc)을 kanban 노드로 배치 발행한다.

use super::Deps;
use serde_json::{json, Value};
use std::collections::HashMap;

/// registerPrompts({role:text}) → kanban prompt.put 등록(sha256 dedup). role→hash 목록 반환(삽입 순서).
pub fn register_prompt_templates(
    register_prompts: &Value,
    deps: &dyn Deps,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Some(obj) = register_prompts.as_object() {
        for (role, text) in obj {
            if let Some(hash) = deps.put_prompt(text.clone()) {
                out.push((role.clone(), hash));
            }
        }
    }
    out
}

/// DraftDoc 배치 발행 — verify_contract 템플릿/directive/schema → 해시,
/// requirements → item 노드(평탄), tasks → task 노드(blockedBy DraftDoc id→칸반 id 해석). 반환=발행 수.
pub fn apply_draft_doc(
    deps: &dyn Deps,
    doc: &Value,
    chunk_kanban_id: Option<&str>,
    task_ctx: Option<&Value>,
) -> Result<usize, String> {
    let vc = doc
        .get("verify_contract")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let put = |value: Option<&Value>| -> Option<String> {
        match value {
            Some(v) if !v.is_null() && v.as_str() != Some("") => deps.put_prompt(v.clone()),
            _ => None,
        }
    };
    let h_t = put(vc.get("template"));
    let h_d = put(vc.get("directive"));
    let h_s = put(vc.get("schema"));

    // DraftDoc id → 칸반 id (chunk_ref 는 이미 칸반 id).
    let mut key_of: HashMap<String, String> = HashMap::new();
    if let (Some(cr), Some(ck)) = (
        doc.get("chunk_ref").and_then(|v| v.as_str()),
        chunk_kanban_id,
    ) {
        key_of.insert(cr.to_string(), ck.to_string());
    }
    let mut published = 0usize;
    let initial_badge = vc
        .get("initial_badge")
        .and_then(|v| v.as_str())
        .unwrap_or("검수전");

    // requirements → item 노드(평탄: chunk 직속). 정규화 body = 해시 3개.
    if let Some(reqs) = doc.get("requirements").and_then(|v| v.as_array()) {
        for r in reqs {
            let mut base = serde_json::Map::new();
            base.insert("promptHash".into(), json!(h_t));
            if let Some(hd) = &h_d {
                base.insert("refs".into(), json!({ "directive": hd }));
            }
            if let Some(hs) = &h_s {
                base.insert("schemaHash".into(), json!(hs));
            }
            let mut params = serde_json::Map::new();
            params.insert(
                "title".into(),
                r.get("title").cloned().unwrap_or(Value::Null),
            );
            params.insert("parentId".into(), json!(chunk_kanban_id));
            params.insert("body".into(), json!(Value::Object(base).to_string()));
            params.insert("blockedBy".into(), json!([]));
            params.insert("locked".into(), json!(true));
            params.insert("type".into(), json!("task"));
            params.insert("kind".into(), json!("item"));
            params.insert(
                "badge".into(),
                json!(r
                    .get("badge")
                    .and_then(|v| v.as_str())
                    .unwrap_or(initial_badge)),
            );
            if let Some(d) = r.get("description").and_then(|v| v.as_str()) {
                params.insert("description".into(), json!(d));
            }
            if let Some(o) = r.get("origin").and_then(|v| v.as_str()) {
                params.insert("origin".into(), json!(o));
            }
            // 라우팅 tier — 요건이 실은 난이도를 item 노드에 싣는다. reconcile 이 exec 시 with_routing 으로
            // 실행자에 honor. 빈문자/부재 = 미삽입(기본 최고 보존).
            if let Some(e) = r
                .get("effort")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                params.insert("effort".into(), json!(e));
            }
            if let Some(m) = r
                .get("model")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                params.insert("model".into(), json!(m));
            }
            if let Some(id) = deps.add_node(Value::Object(params)) {
                if let Some(rid) = r.get("id").and_then(|v| v.as_str()) {
                    key_of.insert(rid.to_string(), id);
                }
            }
            published += 1;
        }
    }

    // tasks → task 노드(hunt/audit). body=exec-stage 입력(skeleton 임베드).
    if let Some(tasks) = doc.get("tasks").and_then(|v| v.as_array()) {
        for t in tasks {
            let blocked_by: Vec<String> = t
                .get("blocked_by")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|id| id.as_str())
                        .map(|id| key_of.get(id).cloned().unwrap_or_else(|| id.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            let stage = t.get("stage").cloned().unwrap_or(Value::Null);
            let body = if let Some(sk) = task_ctx.and_then(|c| c.get("skeleton")) {
                json!({ "skeleton": sk, "stage": stage, "args": { "directive": task_ctx.and_then(|c| c.get("directive")).cloned().unwrap_or(Value::Null), "chunkRef": chunk_kanban_id } }).to_string()
            } else {
                json!({ "stage": stage }).to_string()
            };
            let params = json!({
                "title": stage,
                "parentId": chunk_kanban_id,
                "body": body,
                "blockedBy": blocked_by,
                "locked": true,
                "type": "task",
                "kind": "task",
            });
            if let Some(id) = deps.add_node(params) {
                if let Some(tid) = t.get("id").and_then(|v| v.as_str()) {
                    key_of.insert(tid.to_string(), id);
                }
            }
            published += 1;
        }
    }

    // chunk_title → 덩어리 title 갱신.
    if let Some(ct) = doc.get("chunk_title").and_then(|v| v.as_str()) {
        if !ct.is_empty() {
            if let Some(ck) = chunk_kanban_id {
                deps.edit_node(ck, json!({ "title": ct }));
            }
        }
    }
    Ok(published)
}
