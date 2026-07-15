//! domain_lib — ③파생용 분야별 도메인 지식 라이브러리(빌트인).
//! 사용자가 명시한 분야(웹서비스/소설/여행/결혼식)의 도메인 지시어를 임베드.
//! derive_directive 가 소비하는 콘텐츠 — 일반 아이디어에 없고 일반 워크플로 지식으로도
//! 모르는 도메인 불변을 분야별로 담는다. 신규 분야는 src/domains/<name>.json 추가 +
//! BUILTIN 목록 등록. 외부 라이브러리는 DomainLibrary::from_json 으로 병합.

use crate::derive_directive::{DomainEntry, DomainLibrary};

const WEBSERVICE: &str = include_str!("domains/webservice.json");
const NOVEL: &str = include_str!("domains/novel.json");
const TRAVEL: &str = include_str!("domains/travel.json");
const WEDDING: &str = include_str!("domains/wedding.json");

const BUILTIN: [&str; 4] = [WEBSERVICE, NOVEL, TRAVEL, WEDDING];

/// builtin_library — 빌트인 분야 전부 병합한 라이브러리. 파싱 실패 분야는 건너뛴다.
pub fn builtin_library() -> DomainLibrary {
    let mut entries: Vec<DomainEntry> = vec![];
    for raw in BUILTIN {
        match serde_json::from_str::<DomainEntry>(raw) {
            Ok(e) => entries.push(e),
            Err(_) => continue,
        }
    }
    DomainLibrary { entries }
}

/// merged_library — 빌트인 + 외부 라이브러리(엔트리 추가) 병합.
pub fn merged_library(extra: DomainLibrary) -> DomainLibrary {
    let mut lib = builtin_library();
    lib.entries.extend(extra.entries);
    lib
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive_directive::{match_domains, synth_directives};

    #[test]
    fn builtin_has_all_four_domains() {
        let lib = builtin_library();
        let names: Vec<&str> = lib.entries.iter().map(|e| e.domain.as_str()).collect();
        assert_eq!(lib.entries.len(), 4, "4개 분야 전부 파싱");
        for d in ["webservice", "novel", "travel", "wedding"] {
            assert!(names.contains(&d), "{d} 분야 누락");
        }
    }

    #[test]
    fn permission_idea_pulls_admin_directive() {
        // canonical: "권한 관리 서비스" → webservice 의 web-authz-admin(어드민 페이지).
        let lib = builtin_library();
        let directives = synth_directives("사용자 권한 관리 서비스", &lib);
        let ids: Vec<&str> = directives.iter().map(|d| d.id.as_str()).collect();
        assert!(
            ids.contains(&"web-authz-admin"),
            "권한 → 어드민 페이지 지시어 누락"
        );
        let admin = directives
            .iter()
            .find(|d| d.id == "web-authz-admin")
            .unwrap();
        assert!(admin.directive.contains("어드민"));
        assert_eq!(admin.domain, "webservice");
    }

    #[test]
    fn each_domain_triggers_independently() {
        let lib = builtin_library();
        assert_eq!(match_domains("판타지 웹소설 연재", &lib), vec!["novel"]);
        assert_eq!(match_domains("제주도 여행 일정", &lib), vec!["travel"]);
        assert_eq!(match_domains("결혼식 청첩장", &lib), vec!["wedding"]);
        // 도메인 무관 아이디어 → 매칭 없음.
        assert!(match_domains("그냥 계산기", &lib).is_empty());
    }

    #[test]
    fn merged_library_adds_external() {
        let extra: DomainLibrary = serde_json::from_str(
            r#"{"entries":[{"domain":"game","triggers":["게임","game"],"directives":[{"id":"game-loop","directive":"코어 게임 루프를 먼저 정의한다.","rationale":"루프가 재미의 단위다."}]}]}"#,
        )
        .unwrap();
        let lib = merged_library(extra);
        assert_eq!(lib.entries.len(), 5);
        let d = synth_directives("멀티플레이 게임", &lib);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].id, "game-loop");
    }
}
