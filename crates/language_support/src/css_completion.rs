//! Lexical CSS property completion; selector/navigation lookup belongs to class indexes.
use crate::lsp_types::{CompletionItem, CompletionItemKind, CompletionTextEdit, TextEdit};

// Standard property identifiers are bundled facts, without runtime network/catalog I/O.
const PROPERTIES: &str = "accent-color align-content align-items align-self alignment-baseline all animation animation-composition animation-delay animation-direction animation-duration animation-fill-mode animation-iteration-count animation-name animation-play-state animation-timeline animation-timing-function appearance aspect-ratio backdrop-filter backface-visibility background background-attachment background-blend-mode background-clip background-color background-image background-origin background-position background-position-x background-position-y background-repeat background-size baseline-shift block-size border border-block border-block-color border-block-end border-block-start border-block-style border-block-width border-bottom border-bottom-color border-bottom-left-radius border-bottom-right-radius border-bottom-style border-bottom-width border-collapse border-color border-image border-image-outset border-image-repeat border-image-slice border-image-source border-image-width border-inline border-inline-end border-inline-start border-left border-left-color border-left-style border-left-width border-radius border-right border-right-color border-right-style border-right-width border-spacing border-style border-top border-top-color border-top-left-radius border-top-right-radius border-top-style border-top-width border-width bottom box-decoration-break box-shadow box-sizing break-after break-before break-inside caption-side caret-color clear clip clip-path color color-scheme column-count column-fill column-gap column-rule column-rule-color column-rule-style column-rule-width column-span column-width columns contain contain-intrinsic-size container container-name container-type content content-visibility counter-increment counter-reset counter-set cursor cx cy d direction display dominant-baseline empty-cells fill fill-opacity fill-rule filter flex flex-basis flex-direction flex-flow flex-grow flex-shrink flex-wrap float flood-color flood-opacity font font-family font-feature-settings font-kerning font-language-override font-optical-sizing font-palette font-size font-size-adjust font-stretch font-style font-synthesis font-synthesis-position font-variant font-variant-alternates font-variant-caps font-variant-east-asian font-variant-ligatures font-variant-numeric font-variant-position font-variation-settings font-weight gap grid grid-area grid-auto-columns grid-auto-flow grid-auto-rows grid-column grid-column-end grid-column-start grid-row grid-row-end grid-row-start grid-template grid-template-areas grid-template-columns grid-template-rows height hyphens image-rendering inline-size inset inset-block inset-block-end inset-block-start inset-inline inset-inline-end inset-inline-start isolation justify-content justify-items justify-self left letter-spacing lighting-color line-break line-height list-style list-style-image list-style-position list-style-type margin margin-block margin-block-end margin-block-start margin-bottom margin-inline margin-inline-end margin-inline-start margin-left margin-right margin-top mask mask-clip mask-composite mask-image mask-mode mask-origin mask-position mask-repeat mask-size mask-type max-block-size max-height max-inline-size max-width min-block-size min-height min-inline-size min-width mix-blend-mode object-fit object-position offset offset-anchor offset-distance offset-path offset-position offset-rotate opacity order orphans outline outline-color outline-offset outline-style outline-width overflow overflow-anchor overflow-clip-margin overflow-wrap overflow-x overflow-y overscroll-behavior overscroll-behavior-x overscroll-behavior-y padding padding-block padding-block-end padding-block-start padding-bottom padding-inline padding-inline-end padding-inline-start padding-left padding-right padding-top page-break-after page-break-before page-break-inside paint-order perspective perspective-origin place-content place-items place-self pointer-events position quotes r resize right rotate row-gap ruby-align ruby-position rx ry scale scroll-behavior scroll-margin scroll-margin-block scroll-margin-bottom scroll-margin-inline scroll-margin-left scroll-margin-right scroll-margin-top scroll-padding scroll-padding-block scroll-padding-bottom scroll-padding-inline scroll-padding-left scroll-padding-right scroll-padding-top scroll-snap-align scroll-snap-stop scroll-snap-type scrollbar-color scrollbar-gutter scrollbar-width shape-outside shape-margin stop-color stop-opacity stroke stroke-dasharray stroke-dashoffset stroke-linecap stroke-linejoin stroke-miterlimit stroke-opacity stroke-width tab-size table-layout text-align text-align-last text-anchor text-combine-upright text-decoration text-decoration-color text-decoration-line text-decoration-style text-decoration-thickness text-emphasis text-emphasis-color text-emphasis-position text-emphasis-style text-indent text-overflow text-rendering text-shadow text-transform text-underline-offset text-underline-position text-wrap text-wrap-mode text-wrap-style top touch-action transform transform-box transform-origin transform-style transition transition-behavior transition-delay transition-duration transition-property transition-timing-function translate unicode-bidi user-select vertical-align visibility white-space widows width will-change word-break word-spacing word-wrap writing-mode x y z-index zoom";

/// Returns property suggestions only in declaration-name positions, excluding comments,
/// strings, values, selectors and custom properties. No parser or filesystem I/O.
#[must_use]
pub fn properties(source: &str, caret: usize, explicit: bool) -> Vec<CompletionItem> {
    let Some(range) = property_range(source, caret, explicit) else {
        return Vec::new();
    };
    let colon = source[range.end..].trim_start().starts_with(':');
    let prefix = source[range.start..caret].to_ascii_lowercase();
    PROPERTIES
        .split_ascii_whitespace()
        .filter(|name| name.starts_with(&prefix))
        .take(100)
        .map(|name| CompletionItem {
            label: name.into(),
            kind: Some(CompletionItemKind::PROPERTY),
            detail: Some("CSS property".into()),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range: crate::lsp_types::Range::new(
                    crate::position(source, range.start),
                    crate::position(source, range.end),
                ),
                new_text: if colon {
                    name.into()
                } else {
                    format!("{name}: ")
                },
            })),
            ..CompletionItem::default()
        })
        .collect()
}
fn property_range(source: &str, caret: usize, explicit: bool) -> Option<std::ops::Range<usize>> {
    let before = source.get(..caret)?;
    let mut quote = None;
    let mut comment = false;
    let mut escaped = false;
    let mut blocks = Vec::new();
    let mut comment_start = 0;
    let mut parentheses = 0usize;
    let mut start = 0;
    let bytes = before.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if comment {
            if b == b'*' && bytes.get(i + 1) == Some(&b'/') {
                comment = false;
                i += 2;
                if before[start..comment_start].trim().is_empty() {
                    start = i;
                }
            } else {
                i += 1;
            }
            continue;
        }
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        if b == b'/' && bytes.get(i + 1) == Some(&b'*') {
            comment_start = i;
            comment = true;
            i += 2;
            continue;
        }
        match b {
            b'\'' | b'"' => quote = Some(b),
            b'(' | b'[' => parentheses += 1,
            b')' | b']' => parentheses = parentheses.saturating_sub(1),
            b'{' if parentheses == 0 => {
                let header = before[start..i].trim();
                blocks.push(
                    !header.starts_with('@')
                        || [
                            "@font-face",
                            "@page",
                            "@property",
                            "@counter-style",
                            "@font-palette-values",
                            "@position-try",
                        ]
                        .iter()
                        .any(|rule| header.starts_with(rule)),
                );
                start = i + 1;
            }
            b'}' if parentheses == 0 => {
                blocks.pop();
                start = i + 1;
            }
            b';' if parentheses == 0 => start = i + 1,
            _ => {}
        }
        i += 1;
    }
    if comment || quote.is_some() || parentheses != 0 || blocks.last() != Some(&true) {
        return None;
    }
    let segment = before[start..].trim_start();
    if segment.starts_with("--")
        || (!explicit && segment.is_empty())
        || !segment
            .bytes()
            .all(|b| b.is_ascii_alphabetic() || b == b'-')
    {
        return None;
    }
    let start = caret - segment.len();
    let end = source[caret..]
        .find(|c: char| !c.is_ascii_alphabetic() && c != '-')
        .map_or(source.len(), |i| caret + i);
    Some(start..end)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn property_prefix_hyphen_edit_and_context_exclusions() -> Result<(), Box<dyn std::error::Error>>
    {
        let source = ".card { font- }";
        let caret = source.find("font-").ok_or("prefix")? + 5;
        let items = properties(source, caret, false);
        let item = items
            .iter()
            .find(|i| i.label == "font-family")
            .ok_or("font-family suggestion")?;
        let edits = crate::completion::edits(item, source, caret).ok_or("property edit")?;
        let mut formatted = source.to_string();
        for (range, text) in edits.into_iter().rev() {
            formatted.replace_range(range, &text);
        }
        assert_eq!(formatted, ".card { font-family:  }");
        for source in [
            "font",
            ".font",
            ".a{color:font",
            ".a{/*font",
            ".a{content:'font",
            ".a{--font",
            ".a{background:url(font",
            ".a{background:/*x*/font",
            "@media screen{font",
        ] {
            assert_eq!(properties(source, source.len(), false).len(), 0, "{source}");
        }
        let source = ".a{/*note*/font";
        assert!(
            properties(source, source.len(), false)
                .iter()
                .any(|i| i.label == "font-family")
        );
        let source = ".a{font-fa: serif}";
        let caret = source.find(':').ok_or("colon")?;
        let item = properties(source, caret, false)
            .into_iter()
            .find(|i| i.label == "font-family")
            .ok_or("property before colon")?;
        let edits = crate::completion::edits(&item, source, caret).ok_or("colon edit")?;
        assert_eq!(edits[0].1, "font-family");
        Ok(())
    }
}
