//! prompt_assembly — exec-one과 doc_interp가 공유하는 agent 프롬프트 조립.

use crate::lang::Language;
use serde_json::Value as Json;

const SCHEMA_INSTRUCTION: &str =
    "\n\n## Output format\nReturn ONLY a JSON object — no markdown fence, no prose, no explanation — conforming to this JSON Schema:\n";

/// build_prompt_with_schema — Json schema 로 프롬프트 조립(exec-one/exec-stage 러너 공유).
/// 언어 계약을 schema 뒤에 둔다 — 계약이 "the schema" 를 가리키고, 모델이 마지막에 읽는다.
/// 본문 → schema 지시 → 언어 계약 순.
pub fn build_prompt_with_schema(
    prompt: &str,
    schema: Option<&Json>,
    lang: Option<&Language>,
) -> String {
    let mut full = prompt.to_string();
    if let Some(sj) = schema {
        if sj.is_object() {
            full.push_str(SCHEMA_INSTRUCTION);
            full.push_str(&serde_json::to_string_pretty(sj).unwrap_or_default());
        }
    }
    if let Some(l) = lang {
        full.push_str(&l.contract());
    }
    full
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn build_prompt_appends_language_contract() {
        // [기준] lang 지정 시 agent 프롬프트 끝에 출력 언어 계약이 붙는다(exec-one/exec-stage 러너가 실제로 보냄).
        let no_lang = build_prompt_with_schema("본문", None, None);
        assert_eq!(no_lang, "본문", "lang 없으면 본문 그대로");
        let en = build_prompt_with_schema("body", None, Some(&Language::parse("en")));
        assert!(en.starts_with("body"));
        assert!(en.contains("Output language"));
        assert!(en.contains("Do NOT"));
        // schema 와 함께면: 본문 → schema → 언어 계약 순.
        let schema = json!({ "type": "object", "required": ["x"] });
        let ko = build_prompt_with_schema("body", Some(&schema), Some(&Language::parse("ko")));
        let i_schema = ko.find("Output format").unwrap();
        let i_lang = ko.find("출력 언어").unwrap();
        assert!(i_schema < i_lang, "schema 지시 뒤에 언어 계약");
    }
}
