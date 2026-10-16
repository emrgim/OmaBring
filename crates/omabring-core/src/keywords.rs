/// Whole-token phrase match. A drink word must not fire inside a longer word
/// (`tea` in steak, `gin` in virgin, `rum` in crumbs).
pub fn has_phrase(blob: &str, phrase: &str) -> bool {
    let phrase = phrase.trim();
    if phrase.is_empty() {
        return false;
    }
    let raw_blob = blob.to_ascii_lowercase();
    let raw_phrase = phrase.to_ascii_lowercase();
    if raw_phrase.contains('/') || raw_phrase.contains('.') {
        return raw_blob.contains(raw_phrase.trim());
    }
    let hay = tokens(&raw_blob);
    let needle = tokens(&raw_phrase);
    if needle.is_empty() || hay.len() < needle.len() {
        return false;
    }
    hay.windows(needle.len()).any(|w| w == needle.as_slice())
}

pub fn has_any(blob: &str, phrases: &[&str]) -> bool {
    phrases.iter().any(|p| has_phrase(blob, p))
}

fn tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            cur.push(c.to_ascii_lowercase());
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_do_not_match_inside_words() {
        assert!(!has_phrase("steak", "tea"));
        assert!(!has_phrase("virgin olive oil", "gin"));
        assert!(!has_phrase("breadcrumbs", "rum"));
        assert!(has_phrase("green tea bags", "tea"));
        assert!(has_phrase("still water 1.5L", "still water"));
    }
}
