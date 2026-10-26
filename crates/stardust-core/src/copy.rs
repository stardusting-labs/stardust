//! User-facing wording checks.

/// Returns the first disallowed product-copy term found in `text`.
///
/// English terms match without case. Korean terms are the same claims in
/// Korean product copy.
///
/// The license text is not product copy and is not passed through here.
pub fn find_disallowed_copy(text: &str) -> Option<&'static str> {
    const TERMS: &[&str] = &[
        "monitoring",
        "real-time",
        "realtime",
        "real time",
        "verified",
        "compliance",
        "guaranteed",
        "모니터링",
        "실시간",
        "검증",
        "보장",
        "컴플라이언스",
        "규정 준수",
    ];
    let lower = text.to_ascii_lowercase();
    TERMS.iter().copied().find(|term| lower.contains(term))
}

#[cfg(test)]
mod tests {
    use super::find_disallowed_copy;
    use std::path::PathBuf;

    #[test]
    fn product_docs_avoid_disallowed_copy() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        for name in ["README.md", "SECURITY.md", "CONTRIBUTING.md"] {
            let text = std::fs::read_to_string(root.join(name))
                .unwrap_or_else(|err| panic!("read {name}: {err}"));
            assert_eq!(find_disallowed_copy(&text), None, "{name}");
        }
    }
}
