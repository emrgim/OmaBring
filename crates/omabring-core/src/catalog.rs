/// GTIN helpers for receipt barcodes and catalog matching.

pub fn digits_only(raw: &str) -> String {
    raw.chars().filter(|c| c.is_ascii_digit()).collect()
}

pub fn ean13_check_digit(body: &str) -> Option<char> {
    if body.len() != 12 || !body.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut total = 0u32;
    for (i, ch) in body.chars().enumerate() {
        let d = ch.to_digit(10).unwrap_or(0);
        total += if i % 2 == 0 { d } else { d * 3 };
    }
    char::from_digit((10 - (total % 10)) % 10, 10)
}

/// 13-digit codes are kept. 12-digit codes gain an EAN-13 check digit.
pub fn official_ean(raw: &str) -> String {
    let digits = digits_only(raw);
    if digits.len() == 13 {
        return digits;
    }
    if digits.len() == 12 {
        if let Some(check) = ean13_check_digit(&digits) {
            return format!("{digits}{check}");
        }
    }
    digits
}

pub fn gtin_variants(code: &str) -> Vec<String> {
    let digits = digits_only(code);
    if digits.len() < 8 {
        return Vec::new();
    }
    let mut out = vec![digits.clone()];
    if digits.len() == 12 {
        if let Some(check) = ean13_check_digit(&digits) {
            out.push(format!("{digits}{check}"));
        }
    }
    if (8..13).contains(&digits.len()) {
        out.push(format!("{digits:0>13}"));
    }
    let mut uniq = Vec::new();
    for v in out {
        if !uniq.contains(&v) {
            uniq.push(v);
        }
    }
    uniq
}

pub fn slug_name(name: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in name.to_ascii_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

pub fn raw_product_key(barcode: &str, name: &str) -> String {
    let code = barcode.trim();
    if !code.is_empty() {
        return format!("ean:{code}");
    }
    format!("name:{}", slug_name(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twelve_digits_gain_a_check_digit() {
        let full = official_ean("800227001522");
        assert_eq!(full.len(), 13);
        assert!(full.starts_with("800227001522"));
        assert_eq!(official_ean(&full), full);
    }

    #[test]
    fn name_keys_are_slugs() {
        assert_eq!(raw_product_key("", "White Bread!"), "name:white-bread");
        assert!(raw_product_key("8002270015223", "ignored").starts_with("ean:"));
    }
}
