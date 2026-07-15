//! exec_one — 단일 노드 실행기(stateless). 발행(interp)과 실행을 분리한다(규칙 C).
//! interp 는 노드만 그리고, 실제 LLM 실행은 이 경로(soksak-sidecar-workflow exec-one)뿐.
//! 코어 스케줄러가 칸반 ready 노드 하나의 {prompt, schema?} 를 stdin 으로 주면
//! claude 로 실행 → {oxf, result} 를 stdout 으로 돌려준다. 무상태 — 칸반이 단일 진실.

use serde_json::{json, Value};

/// ExecOneInput — 한 노드의 실행 사양(stdin JSON).
#[derive(Debug, Clone, PartialEq)]
pub struct ExecOneInput {
    pub prompt: String,
    pub schema: Option<Value>,
    pub model: Option<String>,
    /// reasoning effort tier(우리 어휘: low/medium/high/xhigh/max). 노드가 난이도로 실어 보낸다.
    /// 미지정이면 실행자가 기본(최고)로 — 품질우선: 명시 하향만 낮춘다(under-fund 방지).
    pub effort: Option<String>,
}

/// parse_input — stdin JSON({prompt, schema?, model?, effort?}) 파싱. prompt 필수.
pub fn parse_input(raw: &str) -> Result<ExecOneInput, String> {
    let v: Value =
        serde_json::from_str(raw.trim()).map_err(|e| format!("exec-one 입력 JSON 파싱: {e}"))?;
    let prompt = v
        .get("prompt")
        .and_then(|p| p.as_str())
        .ok_or("exec-one 입력에 prompt(문자열) 필수")?
        .to_string();
    if prompt.is_empty() {
        return Err("exec-one prompt 비어있음".into());
    }
    let schema = v.get("schema").filter(|s| s.is_object()).cloned();
    let model = v
        .get("model")
        .and_then(|m| m.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from);
    let effort = v
        .get("effort")
        .and_then(|m| m.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from);
    Ok(ExecOneInput {
        prompt,
        schema,
        model,
        effort,
    })
}

/// extract_oxf — 검증 agent 결과에서 oxf 판정(o/x/f) 추출. 필드명 oxf|verdict 지원.
/// o=통과(keep) / x=검증 후 버림 / f=치명. 없으면 None(검증 노드 아님 — generate 등).
pub fn extract_oxf(result: &Value) -> Option<String> {
    for key in ["oxf", "verdict"] {
        if let Some(s) = result.get(key).and_then(|v| v.as_str()) {
            let t = s.trim().to_lowercase();
            if t == "o" || t == "x" || t == "f" {
                return Some(t);
            }
        }
    }
    None
}

/// build_output — claude 결과를 {oxf, result} 로. oxf 없으면 null(검증 아님).
pub fn build_output(result: Value) -> Value {
    json!({ "oxf": extract_oxf(&result), "result": result })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_prompt_only() {
        let i = parse_input(r#"{"prompt":"검증하라"}"#).unwrap();
        assert_eq!(i.prompt, "검증하라");
        assert_eq!(i.schema, None);
        assert_eq!(i.model, None);
        assert_eq!(i.effort, None, "미지정 effort=None → 실행자 기본(최고)");
    }

    #[test]
    fn parse_with_schema_and_model() {
        let i =
            parse_input(r#"{"prompt":"p","schema":{"type":"object"},"model":"sonnet"}"#).unwrap();
        assert!(i.schema.is_some());
        assert_eq!(i.model.as_deref(), Some("sonnet"));
    }

    #[test]
    fn parse_reads_effort_tier() {
        // 노드가 실은 난이도 tier — 실행자가 provider 별로 honor.
        let i = parse_input(r#"{"prompt":"p","model":"gpt-5.6-luna","effort":"low"}"#).unwrap();
        assert_eq!(i.model.as_deref(), Some("gpt-5.6-luna"));
        assert_eq!(i.effort.as_deref(), Some("low"));
        // 빈 문자열 effort 는 None(기본).
        assert_eq!(
            parse_input(r#"{"prompt":"p","effort":""}"#).unwrap().effort,
            None
        );
    }

    #[test]
    fn parse_rejects_missing_prompt() {
        assert!(parse_input(r#"{"schema":{}}"#).is_err());
        assert!(parse_input(r#"{"prompt":""}"#).is_err(), "빈 prompt 거부");
        assert!(parse_input("not json").is_err());
    }

    #[test]
    fn parse_ignores_non_object_schema() {
        // schema 가 객체가 아니면(예: 빈 문자열) None — raw 텍스트 agent.
        let i = parse_input(r#"{"prompt":"p","schema":"nope"}"#).unwrap();
        assert_eq!(i.schema, None);
    }

    #[test]
    fn extract_oxf_from_oxf_field() {
        assert_eq!(extract_oxf(&json!({"oxf":"o"})), Some("o".into()));
        assert_eq!(
            extract_oxf(&json!({"oxf":"X"})),
            Some("x".into()),
            "대소문자 무시"
        );
        assert_eq!(extract_oxf(&json!({"oxf":" f "})), Some("f".into()), "trim");
    }

    #[test]
    fn extract_oxf_from_verdict_field() {
        assert_eq!(extract_oxf(&json!({"verdict":"x"})), Some("x".into()));
    }

    #[test]
    fn extract_oxf_none_when_absent_or_invalid() {
        assert_eq!(extract_oxf(&json!({"status":"done"})), None);
        assert_eq!(
            extract_oxf(&json!({"oxf":"pass"})),
            None,
            "o/x/f 아닌 값은 무시"
        );
        assert_eq!(extract_oxf(&json!("string-result")), None);
    }

    #[test]
    fn build_output_carries_oxf_and_result() {
        let out = build_output(json!({"oxf":"o","reason":"실재 요건"}));
        assert_eq!(out["oxf"], json!("o"));
        assert_eq!(out["result"]["reason"], json!("실재 요건"));
    }

    #[test]
    fn build_output_oxf_null_for_generate() {
        // 검증 아닌 노드(generate 등) — oxf 없음 → null. result 는 통째 보존.
        let out = build_output(json!({"items":["a","b"]}));
        assert_eq!(out["oxf"], Value::Null);
        assert_eq!(out["result"]["items"], json!(["a", "b"]));
    }
}
