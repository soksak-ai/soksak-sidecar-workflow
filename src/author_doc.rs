//! author_doc — 아이디어 → workflow-doc(LLM 저작) 파이프라인의 **순수 프롬프트 조립**.
//! LLM 호출·검증 게이트(parse_json_lenient + doc_interp::validate)는 main.rs(run_generate_skeleton)가 한다.

use crate::derive_directive::DomainDirective;

/// build_user_prompt — user 층 프롬프트: 사용자 아이디어(DIRECTIVE) + ③파생 도메인 지시어(있으면).
/// 도메인 지시어는 저작 LLM에 "이 도메인 make-or-break 힌트"로 제공한다(강제 아님).
pub fn build_user_prompt(idea: &str, directives: &[DomainDirective]) -> String {
    let mut s = String::new();
    s.push_str("# 사용자 아이디어 (DIRECTIVE)\n");
    s.push_str(idea.trim());
    s.push('\n');
    if !directives.is_empty() {
        s.push_str("\n# ③파생 도메인 지시어 (참고 — 이 도메인의 make-or-break 힌트, 강제 아님)\n");
        for d in directives {
            s.push_str(&format!(
                "- [{}] {} — {}\n",
                d.domain, d.directive, d.rationale
            ));
        }
    }
    s
}

/// refine_schema — 정련 턴 StructuredOutput 스키마({directive, description}). CLI/serve 공용.
pub fn refine_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "required": ["directive", "description"],
        "properties": {
            "directive": { "type": "string", "description": "정련된 DIRECTIVE 전문 — 아이디어의 실제 의도를 담은 지시어(섹션 라벨 재구성 허용, 실질 요건 누락 금지)" },
            "description": { "type": "string", "description": "이 드래프트의 한 줄 서술(담백)" }
        }
    })
}

/// generate_doc — 아이디어 → 검증된 workflow-doc(LLM 정련 실경로). CLI(run_generate_skeleton)와
/// serve(wf_service) 의 단일진실. system=번들 draft-skill.md, 골격 상수=번들 draft.doc.json 조립
/// (LLM 은 {directive, description} 소형 JSON 만 — 재타이핑 0). 정련 2회 후 최종 실패는 loud.
/// assemble/with-refined(LLM 0) 모드는 CLI 전용 — 이 함수는 정련 실경로만 소유한다.
pub fn generate_doc(
    idea: &str,
    model: &str,
    lang: Option<&crate::lang::Language>,
    system: &str,
    env: &[(String, String)],
    gen_out: Option<&str>,
) -> Result<serde_json::Value, String> {
    use crate::provider::{run_agent, AgentRequest};
    use serde_json::Value;
    let directives =
        crate::derive_directive::derive_directives(idea, &crate::domain_lib::builtin_library());
    let mut user = build_user_prompt(idea, &directives);
    if let Some(l) = lang {
        user.push_str(&l.contract());
    }
    let schema = refine_schema();
    let template: Value = serde_json::from_str(crate::paths::bundled_workflow("draft")?)
        .map_err(|e| format!("번들 draft 파싱: {e}"))?;
    let mut last_err = String::new();
    for attempt in 1..=2 {
        if attempt > 1 {
            eprintln!("[soksak] generate-skeleton 재정련 시도 {attempt}/2 — 직전: {last_err}");
        }
        let req = AgentRequest {
            prompt: user.clone(),
            model,
            allowed_tools: vec![],
            timeout_secs: 7200,
            system_prompt: Some(system.to_string()),
            text_only: true,
            schema: Some(schema.clone()),
            // 단일턴 저작(아이디어→workflow-doc)은 본질적으로 최고를 요구 — 최고 effort 고정(D). STEP 0
            // 실측: 최고는 claude `max`(xhigh 위)·codex `ultra`. run_codex_once 매핑이 max→ultra 정렬.
            effort: "max".into(),
        };
        let out = match run_agent(&req, env) {
            Ok(o) => o,
            Err(e) => {
                last_err = format!("정련 호출 실패: {e}");
                continue;
            }
        };
        if let Some(p) = gen_out {
            let _ = std::fs::write(p, serde_json::to_string_pretty(&out).unwrap_or_default());
            eprintln!("[soksak] 정련 산출 보존 → {p}");
        }
        let directive = out
            .get("directive")
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let description = out
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if directive.is_empty() {
            last_err = "정련 directive 비어있음".to_string();
            continue;
        }
        let doc = crate::doc_interp::inject_refinement(&template, &directive, &description);
        if let Err(violations) = crate::doc_interp::validate(&doc) {
            last_err = format!(
                "조립 doc 검증 실패({}건): {}",
                violations.len(),
                violations.first().cloned().unwrap_or_default()
            );
            continue;
        }
        return Ok(doc);
    }
    Err(format!("generate-skeleton: 정련 2회 실패 — {last_err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(domain: &str, directive: &str, rationale: &str) -> DomainDirective {
        DomainDirective {
            id: "x".into(),
            directive: directive.into(),
            rationale: rationale.into(),
            domain: domain.into(),
        }
    }

    #[test]
    fn build_user_prompt_includes_idea() {
        let p = build_user_prompt("  약국 재고 SaaS  ", &[]);
        assert!(p.contains("약국 재고 SaaS"), "아이디어 포함");
        assert!(p.contains("DIRECTIVE"), "DIRECTIVE 헤더");
        assert!(!p.contains("③파생"), "지시어 없으면 파생 섹션 없음");
    }

    #[test]
    fn build_user_prompt_lists_directives() {
        let ds = [
            dir(
                "SYSTEM",
                "운영자 콘솔을 권한 등급별로",
                "권한 오남용 make-or-break",
            ),
            dir(
                "LEGAL",
                "마약류 재고 불일치 시 기한 내 신고",
                "마약류관리법 의무",
            ),
        ];
        let p = build_user_prompt("약국 SaaS", &ds);
        assert!(p.contains("③파생"), "지시어 섹션 헤더");
        assert!(p.contains("[SYSTEM] 운영자 콘솔을 권한 등급별로 — 권한 오남용 make-or-break"));
        assert!(p.contains("[LEGAL] 마약류 재고 불일치 시 기한 내 신고 — 마약류관리법 의무"));
    }
}
