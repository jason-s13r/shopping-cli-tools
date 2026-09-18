//! Where a store can be.
//!
//! Per-store stock is asked for **one region at a time**: the endpoint takes a
//! `State` code and answers only that region's stores, so "which stores have
//! this" nationwide is thirteen requests rather than one. The list is fixed --
//! it is New Zealand's regions -- so it is written here rather than discovered.

/// The region codes the storefront's store finder accepts, with the names it
/// shows for them.
///
/// Ordered north to south, as the site's own dropdown is. Note there are
/// thirteen: `Tasman`, `Marlborough` and `West Coast` have no Farmers stores
/// and the storefront does not offer them.
pub const REGIONS: [(&str, &str); 13] = [
    ("NTL", "Northland"),
    ("AUK", "Auckland"),
    ("WKO", "Waikato"),
    ("BOP", "Bay of Plenty"),
    ("GIS", "Gisborne"),
    ("TKI", "Taranaki"),
    ("MWT", "Manawatū-Whanganui"),
    ("HKB", "Hawke's Bay"),
    ("WGN", "Wellington"),
    ("NSN", "Nelson"),
    ("CAN", "Canterbury"),
    ("OTA", "Otago"),
    ("STL", "Southland"),
];

/// Whether a code is one the store finder knows.
pub fn is_region(code: &str) -> bool {
    REGIONS
        .iter()
        .any(|(c, _)| c.eq_ignore_ascii_case(code.trim()))
}

/// A region by code or by name, however it was typed.
///
/// Both are accepted because both are what someone has: the code is what the
/// endpoint takes and the name is what the site shows. Matching is loose on
/// case and on the punctuation in `Hawke's Bay` and `Manawatū-Whanganui`,
/// neither of which is reasonable to expect at a keyboard.
pub fn region(input: &str) -> Option<&'static str> {
    let wanted = squash(input);
    if wanted.is_empty() {
        return None;
    }
    REGIONS
        .iter()
        .find(|(code, name)| squash(code) == wanted || squash(name) == wanted)
        .map(|(code, _)| *code)
}

/// The name shown for a code.
pub fn region_name(code: &str) -> Option<&'static str> {
    REGIONS
        .iter()
        .find(|(c, _)| c.eq_ignore_ascii_case(code.trim()))
        .map(|(_, name)| *name)
}

/// Letters and digits only, lowercased, with accents folded to their base
/// letter. `Hawke's Bay`, `hawkes bay` and `HAWKESBAY` all reduce to one
/// string, and so do `Manawatū` and `Manawatu`.
fn squash(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            'ū' | 'Ū' => 'u',
            'ā' | 'Ā' => 'a',
            'ī' | 'Ī' => 'i',
            'ō' | 'Ō' => 'o',
            'ē' | 'Ē' => 'e',
            other => other,
        })
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_region_is_found_by_its_code_or_by_its_name() {
        assert_eq!(region("AUK"), Some("AUK"));
        assert_eq!(region("auk"), Some("AUK"));
        assert_eq!(region("Auckland"), Some("AUK"));
        assert_eq!(region("auckland"), Some("AUK"));
    }

    #[test]
    fn a_macron_and_an_apostrophe_are_not_required_at_a_keyboard() {
        // Nobody types either, and failing the lookup would read as "there is
        // no such region".
        assert_eq!(region("Hawke's Bay"), Some("HKB"));
        assert_eq!(region("hawkes bay"), Some("HKB"));
        assert_eq!(region("Manawatū-Whanganui"), Some("MWT"));
        assert_eq!(region("manawatu whanganui"), Some("MWT"));
    }

    #[test]
    fn an_unknown_region_is_none_rather_than_a_guess() {
        assert_eq!(region("Tasman"), None, "no Farmers store, so no code");
        assert_eq!(region("Sydney"), None);
        assert_eq!(region(""), None);
        assert_eq!(region("  "), None);
    }

    #[test]
    fn a_code_can_be_checked_and_named() {
        assert!(is_region("CAN"));
        assert!(is_region("can"));
        assert!(!is_region("XYZ"));
        assert_eq!(region_name("OTA"), Some("Otago"));
        assert_eq!(region_name("XYZ"), None);
    }

    #[test]
    fn every_code_round_trips_through_both_lookups() {
        for (code, name) in REGIONS {
            assert_eq!(region(code), Some(code), "{code}");
            assert_eq!(region(name), Some(code), "{name}");
            assert_eq!(region_name(code), Some(name));
        }
    }
}
