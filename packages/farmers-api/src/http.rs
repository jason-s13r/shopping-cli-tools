//! How this client presents itself.
//!
//! Akamai Bot Manager fronts the storefront and scores the TLS handshake and
//! the HTTP/2 settings as well as the headers, so `wreq` rather than `reqwest`:
//! every `reqwest` TLS backend is scored as a bot outright.
//!
//! **This is no longer the whole story, and no longer the gate.** As of
//! 2026-09-18 the bot manager scores *where an `_abck` came from*, and one
//! this crate fetched for itself is refused whatever profile it wears. The
//! remedy is [`crate::Warmer`] -- a browser loads one page, and its cookies
//! then work from here. What follows is the sweep that was true before that,
//! kept because it is still what rules out the explanations that look obvious
//! from a refusal, and because the profile still has to be a real browser.
//!
//! **The profile is load-bearing, and only current Firefox is served.**
//! Measured 2026-09-18 with an interleaved sweep -- each profile probed in
//! three rounds, the order reversed between them, a fresh cookie jar each
//! time, so that a pass cannot be an artefact of when it ran:
//!
//! | profile | result |
//! |---|---|
//! | `firefox139`, `firefox143`, `firefox148` | served, every round |
//! | `firefox133` | refused, every round |
//! | `chrome137`, `chrome139`, `edge139`, `safari18_5` | refused, every round |
//!
//! Twenty probes, no contradictions, and the reversed rounds are the evidence:
//! `firefox139` passed running *last* after three failures, and `chrome137`
//! failed between two `firefox148` passes. Time is excluded; the fingerprint is
//! what is being read.
//!
//! The clincher is curl. In the same minute that `wreq` on `firefox139` was
//! served the whole category tree, curl -- sending a byte-identical
//! `User-Agent` and the same navigation headers -- was answered a 5KB
//! `WAF_Deny_Page` with no `Set-Cookie` at all. Same address, same headers,
//! same second. So the gate is the TLS handshake and the HTTP/2 settings, not
//! the address, the headers or any token, and **a refusal that looks like an
//! IP ban is worth re-testing with a profile before it is believed.** An
//! earlier sweep here concluded the opposite -- that the profile was noise and
//! address reputation dominated -- because it probed each profile once, in one
//! order, and could not tell a profile effect from drift.
//!
//! Everything under `/INTERSHOP/` is gated this way, both the REST API and the
//! `ViewX-` pipelines. The plain pages including `/login` are served cold,
//! which misleads: being handed the sign-in page suggests the gate is about
//! credentials. Constructor.io is a different company's host and is not gated
//! at all, which is why search and suggest work even from a refused client.
//!
//! **A browser is needed, and the sensor still is not.** Measured 2026-09-18 on
//! one address inside one minute: a cookie jar harvested from camoufox was
//! served the whole category tree *through `wreq` on `firefox139`*, and a jar
//! `wreq` earned from the same home page was answered `Access Denied` by the
//! byte-identical request -- same profile, same headers, same second. So the
//! fingerprint of the request spending the token is not what is read; the
//! token's provenance is.
//!
//! What did not change is the sensor. The browser's `_abck` reads `~-1~` too --
//! unvalidated, where a sensor-posting session reads `~0~` -- so what is being
//! demanded is not a proof-of-work but a page load by something that is
//! genuinely a browser. That is why the driver in the CLI loads one page and
//! leaves, and why nothing here tries to run the sensor.
//!
//! A jar so earned is good for a few minutes and a couple of dozen requests,
//! not for a day: it lapses back into `Access Denied` with no warning, which is
//! what the deny-and-rewarm retry in [`crate::Client`] is for.

use net_kit::wreq_util::Profile;
use net_kit::ClientSpec;

/// The browser this client presents as, at every layer.
///
/// `wreq` derives the TLS fingerprint, the HTTP/2 settings and the headers --
/// including the `User-Agent` -- from this one value, which is why nothing here
/// sets a user agent by hand.
///
/// **No longer the lever it was.** Current Firefox was once the one thing that
/// got a client admitted; now admission comes from [`crate::Warmer`] and a
/// browser-earned jar is spent successfully from Chrome and Safari profiles
/// too -- measured. This stays current Firefox anyway, because it is what the
/// browser buying that jar presents as and a matching profile is the one that
/// needs no explaining.
///
/// [`profile`] is the escape hatch for when this ages out, so that costs a
/// variable rather than a release. Reach for it second, though: a refusal is
/// now far more likely to be a lapsed jar than a fingerprint.
pub const EMULATION: Profile = Profile::Firefox139;

/// A profile by name, for the `FMNZ_EMULATION` escape hatch.
///
/// Matched against the enum's own variant names rather than a table written
/// here, so upgrading `wreq-util` brings its new browsers with it. Punctuation
/// and case are ignored.
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
/// Two reasons, and either alone would be enough. The sign-in POST answers
/// `302` to `/account`, and that redirect *is* the success signal -- following
/// it would land on a page and lose the evidence. And this crate carries its
/// cookies by hand rather than in a jar, so a followed redirect would replay
/// the request without the cookies the first hop set.
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
    }

    #[test]
    fn an_unknown_name_is_none_rather_than_a_silent_default() {
        // The override exists to pick a profile that gets past the bot
        // manager, so a typo quietly falling back to the one that does not
        // would be the worst possible answer.
        assert_eq!(profile("netscape"), None);
        assert_eq!(profile("firefox"), None);
        assert_eq!(profile(""), None);
    }
}
