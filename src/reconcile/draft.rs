//! prompt 템플릿 등록 — registerPrompts 를 보드 prompt-store 로 넣고 role→hash 를 돌려준다.

use super::Deps;
use serde_json::Value;

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
