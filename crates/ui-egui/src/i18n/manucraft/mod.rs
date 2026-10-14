//! MANUCRAFT's own translations (`manucraft/<code>.tsv`, same columns as the bundled catalogs),
//! merged into each language when its catalog is parsed. Kept apart from the upstream `.tsv` files
//! so syncing with upstream never conflicts on them; the bundled files stay byte-identical.

/// The extra catalog text for language `code` (empty when there is none).
pub(super) fn extra(code: &str) -> &'static str {
    match code {
        "cs" => include_str!("cs.tsv"),
        "de" => include_str!("de.tsv"),
        "el" => include_str!("el.tsv"),
        "es" => include_str!("es.tsv"),
        "fr" => include_str!("fr.tsv"),
        "hu" => include_str!("hu.tsv"),
        "id" => include_str!("id.tsv"),
        "it" => include_str!("it.tsv"),
        "ja" => include_str!("ja.tsv"),
        "ko" => include_str!("ko.tsv"),
        "nl" => include_str!("nl.tsv"),
        "pl" => include_str!("pl.tsv"),
        "pt-br" => include_str!("pt-br.tsv"),
        "ru" => include_str!("ru.tsv"),
        "sr" => include_str!("sr.tsv"),
        "tr" => include_str!("tr.tsv"),
        "uk" => include_str!("uk.tsv"),
        "vi" => include_str!("vi.tsv"),
        "zh-hans" => include_str!("zh-hans.tsv"),
        "zh-hant" => include_str!("zh-hant.tsv"),
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::super::catalog::{parse_entries, placeholders};
    use super::super::LANGUAGES;

    /// The same checks `bundled_catalogs_are_consistent` makes on the upstream files, for ours,
    /// plus: every complete language translates exactly the same keys.
    #[test]
    fn manucraft_catalogs_are_consistent_and_complete() {
        let mut keys: Option<Vec<(String, String)>> = None;
        for l in LANGUAGES.iter().filter(|l| l.complete_menus) {
            let text = super::extra(l.code);
            assert!(!text.is_empty(), "{}: no MANUCRAFT catalog", l.code);
            let (entries, errors) = parse_entries(text);
            assert!(errors.is_empty(), "{}: {errors:?}", l.code);
            let mut seen = std::collections::HashSet::new();
            for (ctx, src, tr) in &entries {
                assert!(seen.insert((ctx.clone(), src.clone())), "{}: duplicate {src:?}", l.code);
                let (mut want, mut got) = (placeholders(src), placeholders(tr));
                want.sort_unstable();
                got.sort_unstable();
                assert_eq!(want, got, "{}: placeholders differ in {src:?}", l.code);
                assert_eq!(src.ends_with('…'), tr.ends_with('…'), "{}: ellipsis mismatch: {src:?}", l.code);
            }
            let mut mine: Vec<(String, String)> = entries.iter().map(|(c, s, _)| (c.clone(), s.clone())).collect();
            mine.sort();
            match &keys {
                None => keys = Some(mine),
                Some(k) => assert_eq!(k, &mine, "{}: translates other strings than the first language", l.code),
            }
        }
    }
}
