//! Compile-time bundle lookup. Release artifacts are standalone binaries: no
//! executable-relative traversal, working-directory assumption, or symlink is
//! part of the runtime contract.

pub fn bundled_workflow(name: &str) -> Result<&'static str, String> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!("--workflow 이름 부적합: {name:?} (영숫자/-/_ 만)"));
    }
    match name {
        "draft" => Ok(include_str!("../workflows/draft.doc.json")),
        "research" => Ok(include_str!("../workflows/research.doc.json")),
        _ => Err(format!("번들 워크플로 없음: {name}")),
    }
}

pub fn draft_skill() -> &'static str {
    include_str!("../references/draft-skill.md")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_declared_workflows_are_discoverable() {
        for name in ["draft", "research"] {
            let value: serde_json::Value =
                serde_json::from_str(bundled_workflow(name).unwrap()).unwrap();
            assert_eq!(value["spec"], "workflow-doc@0.0.1");
        }
        assert!(bundled_workflow("../draft").is_err());
        assert!(bundled_workflow("unknown").is_err());
        assert!(!draft_skill().trim().is_empty());
    }
}
