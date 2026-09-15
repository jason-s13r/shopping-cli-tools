//! How this client presents itself.
//!
//! Cloudflare fronts the API but only sets `__cf_bm`; no managed challenge and
//! no bot manager was observed on any endpoint this crate calls. So the
//! emulation profile is insurance, not load-bearing like `twlnz_api::EMULATION`.
//!
//! What *is* required is `Origin` and `Referer`: the API's CORS rules answer
//! `403 Invalid CORS request` without them. [`crate::Client`] sets both.

use net_kit::wreq_util::Profile;
use net_kit::ClientSpec;

/// The browser this client presents as, at every layer.
pub const EMULATION: Profile = Profile::Firefox139;

/// A profile by name, for the `M10_EMULATION` escape hatch.
///
/// Matched against the enum's own variant names, so upgrading `wreq-util`
/// brings its new browsers with it. Punctuation and case are ignored.
pub fn profile(name: &str) -> Option<Profile> {
    let wanted = squash(name);
    Profile::VARIANTS
        .iter()
        .copied()
        .find(|p| squash(&format!("{p:?}")) == wanted)
}

fn squash(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The client, **not following redirects**.
///
/// The OAuth flow depends on it: `/oauth/authorize` answers `302` and the
/// authorization code is in the `Location` header. Following it would discard
/// the code and land on the website.
pub fn client_spec() -> ClientSpec {
    client_spec_for(EMULATION)
}

/// The same client, presenting as some other browser.
pub fn client_spec_for(profile: Profile) -> ClientSpec {
    ClientSpec::new(profile, net_kit::wreq::redirect::Policy::none())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_profile_names_a_real_browser() {
        let ua = net_kit::http::user_agent(EMULATION);
        assert!(ua.contains("Mozilla"), "{ua}");
        assert!(ua.contains("Firefox"), "{ua}");
    }

    #[test]
    fn a_profile_is_found_however_its_name_is_punctuated() {
        assert_eq!(profile("firefox139"), Some(Profile::Firefox139));
        assert_eq!(profile("Firefox_139"), Some(Profile::Firefox139));
        assert_eq!(profile("FIREFOX-139"), Some(Profile::Firefox139));
        assert_eq!(profile("netscape"), None);
    }
}
