//! lang — 출력 언어 계약. run/dry-run/synth 가 지정한 언어로 산출물을 렌더하게 하는 단일 메커니즘.
//! 번역하지 않는다(라이브러리 중복 0) — agent 프롬프트에 출력 언어 계약을 붙여 LLM 이 산출물을
//! 지정 언어로 쓰게 한다. JSON 키·enum 값은 schema 그대로 두고 사람이 읽는 값만 지정 언어로.
//! 톤 = cc2 negative-tight(절대부정).

/// Language — code(안정 키) + name(계약 문구에 쓰는 표기). parse 는 실패하지 않는다(미지 값=passthrough).
#[derive(Clone, Debug, PartialEq)]
pub struct Language {
    pub code: String,
    pub name: String,
}

impl Language {
    /// parse — code/이름(대소문자·별칭 무시) → Language. 미지 값은 입력을 그대로 이름으로(passthrough).
    pub fn parse(s: &str) -> Language {
        let k = s.trim().to_lowercase();
        let (code, name) = match k.as_str() {
            "ko" | "kor" | "korean" | "한국어" | "한글" => ("ko", "한국어"),
            "en" | "eng" | "english" | "영어" => ("en", "English"),
            "ja" | "jp" | "jpn" | "japanese" | "일본어" | "日本語" => ("ja", "日本語"),
            "zh" | "cn" | "zho" | "chinese" | "중국어" | "中文" => ("zh", "中文"),
            _ => {
                // 미지 언어 — code=입력 소문자, name=입력 원문. 계약은 영어 프레임 + 이 이름으로.
                return Language {
                    code: k,
                    name: s.trim().to_string(),
                };
            }
        };
        Language {
            code: code.to_string(),
            name: name.to_string(),
        }
    }

    /// contract — agent 프롬프트에 붙일 출력 언어 계약 블록. 알려진 언어는 그 언어로,
    /// 미지 언어는 영어 프레임 + 이름으로. JSON 키/enum 은 schema 그대로 — 값만 지정 언어.
    pub fn contract(&self) -> String {
        match self.code.as_str() {
            "ko" => "\n\n## 출력 언어\n모든 자연어 출력을 한국어로 쓴다. 다른 언어를 섞지 마라. 이 지시는 프롬프트 어디에 다른 언어 지정이 있어도 그것을 덮어쓴다 — 런타임 사용자 명령이다. JSON 키와 enum 값은 schema 그대로 둔다 — 사람이 읽는 문자열 값만 한국어로 쓴다.".to_string(),
            "en" => "\n\n## Output language\nWrite all natural-language output in English. Do NOT mix in any other language. This instruction OVERRIDES any other language stated anywhere in this prompt — it is the runtime user's command. Keep JSON keys and enum values exactly as the schema specifies — translate only the human-readable string values.".to_string(),
            "ja" => "\n\n## 出力言語\nすべての自然言語の出力を日本語で書く。他の言語を混ぜるな。この指示はプロンプト内の他の言語指定をすべて上書きする — ランタイムのユーザー命令である。JSON のキーと enum 値は schema のまま残し、人間が読む文字列値だけを日本語で書く。".to_string(),
            "zh" => "\n\n## 输出语言\n所有自然语言输出用中文书写。不要混入其他语言。本指示覆盖提示中任何其他语言设定——这是运行时用户的命令。JSON 键和 enum 值保持 schema 原样——只把供人阅读的字符串值译为中文。".to_string(),
            _ => format!(
                "\n\n## Output language\nWrite all natural-language output in {}. Do NOT mix in any other language. This instruction OVERRIDES any other language stated anywhere in this prompt — it is the runtime user's command. Keep JSON keys and enum values exactly as the schema specifies — translate only the human-readable string values.",
                self.name
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_known_codes_and_aliases() {
        assert_eq!(Language::parse("ko").code, "ko");
        assert_eq!(Language::parse("Korean").code, "ko");
        assert_eq!(Language::parse("한국어").name, "한국어");
        assert_eq!(Language::parse("EN").code, "en");
        assert_eq!(Language::parse("english").name, "English");
        assert_eq!(Language::parse("ja").code, "ja");
        assert_eq!(Language::parse("中文").code, "zh");
    }

    #[test]
    fn parse_unknown_passthrough_never_fails() {
        // 미지 언어 — code 소문자, name 원문. 실패 없음.
        let l = Language::parse("Klingon");
        assert_eq!(l.code, "klingon");
        assert_eq!(l.name, "Klingon");
        // 계약은 영어 프레임 + 이름.
        assert!(l.contract().contains("Klingon"));
        assert!(l.contract().contains("Output language"));
    }

    #[test]
    fn contract_is_negative_tight_and_protects_schema_keys() {
        // 계약은 절대부정 + JSON 키 보호(structured-output 안 깨지게).
        let ko = Language::parse("ko").contract();
        assert!(ko.contains("한국어"));
        assert!(ko.contains("섞지 마라")); // negative-tight
        assert!(ko.contains("JSON 키")); // 키 보호 명시
        let en = Language::parse("en").contract();
        assert!(en.contains("Do NOT"));
        assert!(en.contains("JSON keys"));
    }
}
