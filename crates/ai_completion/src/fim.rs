//! Bounded Unicode FIM contexts, independent of editor coordinates.
/// Builds the Qwen FIM prompt from the last 2000 prefix and first 1000 suffix scalars.
#[must_use]
pub fn prompt(prefix: &str, suffix: &str) -> String {
    let start = prefix.char_indices().rev().nth(1999).map_or(0, |(i, _)| i);
    let end = suffix
        .char_indices()
        .nth(1000)
        .map_or(suffix.len(), |(i, _)| i);
    format!(
        "<|fim_prefix|>{}<|fim_suffix|>{}<|fim_middle|>",
        &prefix[start..],
        &suffix[..end]
    )
}
#[cfg(test)]
mod tests {
    #[test]
    fn tokens_and_unicode_limits() {
        assert_eq!(
            super::prompt("abc", "def"),
            "<|fim_prefix|>abc<|fim_suffix|>def<|fim_middle|>"
        );
        let result = super::prompt(&"я".repeat(2001), &"🦀".repeat(1001));
        assert_eq!(result.matches('я').count(), 2000);
        assert_eq!(result.matches('🦀').count(), 1000);
    }
}
