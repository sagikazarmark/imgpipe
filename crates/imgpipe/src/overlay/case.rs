//! Language-aware uppercasing. Rust's `to_uppercase` has no locale.

/// Uppercases `text` for `lang` (BCP 47). Turkish and Azerbaijani map `i` to
/// `İ`; `ß` becomes `ẞ` when a font covers it (`has_glyph`), else `SS`.
pub fn uppercase(text: &str, lang: Option<&str>, has_glyph: impl Fn(char) -> bool) -> String {
    let primary = lang.map(|l| {
        l.split(['-', '_'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
    });
    let turkic = matches!(primary.as_deref(), Some("tr" | "az"));
    let capital_sharp_s = has_glyph('ẞ');
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            'i' if turkic => out.push('İ'),
            'ß' if capital_sharp_s => out.push('ẞ'),
            _ => out.extend(c.to_uppercase()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::uppercase;

    #[test]
    fn locale_rules() {
        let all = |_: char| true;
        assert_eq!(uppercase("istanbul", Some("tr"), all), "İSTANBUL");
        assert_eq!(uppercase("istanbul", Some("en"), all), "ISTANBUL");
        assert_eq!(uppercase("straße", Some("de-DE"), all), "STRAẞE");
        assert_eq!(uppercase("straße", None, |c| c != 'ẞ'), "STRASSE");
        assert_eq!(
            uppercase("árvíztűrő tükörfúrógép", Some("hu"), all),
            "ÁRVÍZTŰRŐ TÜKÖRFÚRÓGÉP"
        );
    }
}
