//! derive_directive — ③파생: 일반 아이디어에 도메인 지식을 발라주는 변환기.
//! 핵심 메커니즘(사용자 최우선) — 일반 문장에 **없고** 일반 워크플로 지식으로도 **못 채우는**
//! 도메인 지시어(예: "권한 서비스 → 권한별 어드민 페이지 필수")를, 분야별 도메인 지식
//! 라이브러리에서 끌어와 생성·주입한다. 사람 수작업 → 기계.
//!
//! 엔진 = idea → 도메인 매칭(trigger keyword) → 그 도메인의 directive 끌어와 합성.
//! 도메인 라이브러리 **내용**(웹서비스/소설/여행/결혼식 등)은 분야별 신규 구축 대상 —
//! 본 모듈은 그 라이브러리를 소비하는 엔진 + 데이터 구조 + canonical 예시. 출력 directive
//! 가 워크플로에 주입되는 DIRECTIVE-* 의 재료.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// DirectiveTemplate — 도메인 라이브러리에 저장되는 지시어 템플릿.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DirectiveTemplate {
    /// 안정 id(예: "auth-admin-pages"). dedup/정렬 키.
    pub id: String,
    /// 명령형 지시어(아이디어에 발라줄 도메인 요구).
    pub directive: String,
    /// 근거(왜 이 지시어가 도메인 불변인지 — 도메인 지식).
    pub rationale: String,
}

/// DomainEntry — 한 분야의 trigger + 지시어 묶음.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DomainEntry {
    pub domain: String,
    /// 아이디어에서 이 분야를 매칭하는 keyword(부분일치, 대소문자 무시).
    pub triggers: Vec<String>,
    pub directives: Vec<DirectiveTemplate>,
}

/// DomainLibrary — 분야별 도메인 지식 라이브러리(md/json 에서 로드).
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct DomainLibrary {
    pub entries: Vec<DomainEntry>,
}

impl DomainLibrary {
    /// from_json — JSON 직렬화된 라이브러리 로드.
    pub fn from_json(raw: &[u8]) -> Result<DomainLibrary, String> {
        serde_json::from_slice(raw).map_err(|e| format!("parse domain library: {e}"))
    }
}

/// DomainDirective — ③파생 산출. provenance(domain) 포함.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DomainDirective {
    pub id: String,
    pub directive: String,
    pub rationale: String,
    pub domain: String,
}

fn entry_matches(idea_lower: &str, entry: &DomainEntry) -> bool {
    entry
        .triggers
        .iter()
        .any(|t| !t.is_empty() && idea_lower.contains(&t.to_lowercase()))
}

/// match_domains — 아이디어에 매칭되는 분야 이름(정렬·dedup).
pub fn match_domains(idea: &str, lib: &DomainLibrary) -> Vec<String> {
    let hay = idea.to_lowercase();
    let mut matched: BTreeSet<String> = BTreeSet::new();
    for e in &lib.entries {
        if entry_matches(&hay, e) {
            matched.insert(e.domain.clone());
        }
    }
    matched.into_iter().collect()
}

/// synth_directives — 매칭된 분야들의 지시어를 합성(id dedup, id 정렬). ③파생의 핵심.
/// 결과 = 아이디어에 없던 도메인 지시어 목록 → 워크플로에 주입할 DIRECTIVE-* 재료.
pub fn synth_directives(idea: &str, lib: &DomainLibrary) -> Vec<DomainDirective> {
    let hay = idea.to_lowercase();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out: Vec<DomainDirective> = vec![];
    for e in &lib.entries {
        if !entry_matches(&hay, e) {
            continue;
        }
        for d in &e.directives {
            if seen.insert(d.id.clone()) {
                out.push(DomainDirective {
                    id: d.id.clone(),
                    directive: d.directive.clone(),
                    rationale: d.rationale.clone(),
                    domain: e.domain.clone(),
                });
            }
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 데모 라이브러리 — canonical 예시(권한→어드민) + 웹서비스 일반.
    fn demo_library() -> DomainLibrary {
        DomainLibrary {
            entries: vec![
                DomainEntry {
                    domain: "auth".into(),
                    triggers: vec!["권한".into(), "permission".into(), "역할".into(), "role".into()],
                    directives: vec![DirectiveTemplate {
                        id: "auth-admin-pages".into(),
                        directive: "권한 서비스에는 권한별 어드민 페이지를 필수로 제작한다.".into(),
                        rationale: "권한 모델이 존재하면 운영자가 권한을 관리할 UI 가 반드시 필요하다(도메인 불변).".into(),
                    }],
                },
                DomainEntry {
                    domain: "webservice".into(),
                    triggers: vec!["서비스".into(), "웹".into(), "service".into()],
                    directives: vec![DirectiveTemplate {
                        id: "web-auth-session".into(),
                        directive: "사용자 세션·인증 흐름을 명시한다.".into(),
                        rationale: "웹 서비스는 인증 경계가 필수다.".into(),
                    }],
                },
            ],
        }
    }

    #[test]
    fn canonical_permission_to_admin_pages() {
        // 핵심 시나리오: 아이디어엔 "어드민 페이지"가 없지만 ③파생이 발라준다.
        let idea = "사용자 권한 관리 서비스를 만들고 싶다";
        let lib = demo_library();
        let directives = synth_directives(idea, &lib);
        let ids: Vec<&str> = directives.iter().map(|d| d.id.as_str()).collect();
        // 권한(auth) + 서비스(webservice) 둘 다 매칭.
        assert!(
            ids.contains(&"auth-admin-pages"),
            "권한 → 어드민 페이지 지시어 누락"
        );
        assert!(ids.contains(&"web-auth-session"));
        // 아이디어 원문에 "어드민"이 없음을 확인 — ③파생이 생성한 것.
        assert!(!idea.contains("어드민"));
        let admin = directives
            .iter()
            .find(|d| d.id == "auth-admin-pages")
            .unwrap();
        assert!(admin.directive.contains("어드민 페이지"));
        assert_eq!(admin.domain, "auth");
    }

    #[test]
    fn no_match_yields_no_directives() {
        // 도메인 trigger 없는 아이디어 → 빈 결과(발라줄 것 없음).
        let lib = demo_library();
        assert!(synth_directives("그냥 일기장 앱", &lib).is_empty());
        assert!(match_domains("그냥 일기장 앱", &lib).is_empty());
    }

    #[test]
    fn matched_domains_sorted_deduped() {
        let idea = "권한과 역할 기반 서비스"; // 권한+역할 모두 auth, 서비스 web
        let domains = match_domains(idea, &demo_library());
        assert_eq!(domains, vec!["auth", "webservice"]); // 정렬, auth 1회만
    }

    #[test]
    fn directives_dedup_by_id_and_sorted() {
        // 같은 directive id 가 여러 trigger 로 매칭돼도 1회만.
        let idea = "permission 과 role 이 있는 서비스";
        let directives = synth_directives(idea, &demo_library());
        let ids: Vec<&str> = directives.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, vec!["auth-admin-pages", "web-auth-session"]); // dedup + id 정렬
    }

    #[test]
    fn library_loads_from_json() {
        let raw = serde_json::to_vec(&json!({
            "entries": [{
                "domain": "wedding",
                "triggers": ["결혼식", "wedding"],
                "directives": [{
                    "id": "wedding-rsvp",
                    "directive": "참석 여부(RSVP) 수집 기능을 필수로 둔다.",
                    "rationale": "결혼식은 좌석/식사 준비를 위해 참석자 수가 핵심이다."
                }]
            }]
        }))
        .unwrap();
        let lib = DomainLibrary::from_json(&raw).unwrap();
        let directives = synth_directives("결혼식 청첩장 사이트", &lib);
        assert_eq!(directives.len(), 1);
        assert_eq!(directives[0].id, "wedding-rsvp");
        assert_eq!(directives[0].domain, "wedding");
    }
}
