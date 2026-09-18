//! What the storefront is called with, and what is worth keeping between runs.
//!
//! Intershop authorises entirely by cookie, and there is no token to decode:
//! nothing here is a JWT, so no expiry can be read off anything. Four names
//! matter:
//!
//! - `sid`, the Intershop session. Held by anyone, and what a basket hangs off
//!   before there is an account.
//! - `pgid-Farmers-Shop-Site`, the page group, which the pipelines want
//!   alongside it.
//! - `__Host-SecureSessionID-p…`, the secure session. **The suffix after the
//!   dash is the tell**: a signed-out browser carries `-t…` ones and a
//!   successful sign-in adds a `-p…`. See [`Session::account`].
//! - the Akamai set -- `_abck`, `bm_sz`, `ak_bmsc` and friends -- which is what
//!   a warm-up buys and what a cold start has to buy again.
//!
//! Keeping the Akamai cookies is the point of persisting anything at all: they
//! are what lets the *next* run skip its warm-up, and a warm-up now costs a
//! browser launch and ten seconds rather than one request. See
//! [`crate::Warmer`].

use std::collections::BTreeMap;

use net_kit::{wreq, Secrets};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The Intershop session cookie.
pub const SESSION_COOKIE: &str = "sid";

/// The page group cookie the `ViewX-` pipelines expect beside it.
pub const PAGE_GROUP_COOKIE: &str = "pgid-Farmers-Shop-Site";

/// The prefix every Intershop secure-session cookie shares. What follows it is
/// a kind letter and a hash, and the kind letter is the interesting part.
pub const SECURE_SESSION_PREFIX: &str = "__Host-SecureSessionID-";

/// The kind letter on a secure-session cookie that belongs to a signed-in
/// person, as against the `t` ones every visitor carries.
const PERSONALISED: char = 'p';

/// Akamai's bot-manager cookies. `_abck` is the one that carries the validation
/// flag; the rest travel with it and are refused as a set when split up.
const AKAMAI_COOKIES: [&str; 8] = [
    "_abck", "bm_sz", "ak_bmsc", "bm_s", "bm_so", "bm_mi", "bm_sv", "bm_lso",
];

/// Whether a cookie belongs to the bot manager rather than to the storefront.
///
/// The line a warm-up may not cross: these are what warmth *is*, and
/// everything else in a jar -- the Intershop session, the page group, the
/// secure-session cookies -- speaks for the visit or the person and is not a
/// warm-up's to replace. See [`crate::Client::warm`].
pub fn is_warmth(name: &str) -> bool {
    AKAMAI_COOKIES.contains(&name) || name == "AKA_A2"
}

/// Where a stored session is filed in the credential store.
pub const ACCOUNT: &str = "session";

/// Which cookies are worth writing down.
///
/// Analytics and UI state are not: `ssViewedProducts` and the Constructor.io
/// client id say what someone looked at, and a credential store is the wrong
/// place for that.
pub fn keep(name: &str) -> bool {
    name.starts_with(SECURE_SESSION_PREFIX)
        || name == SESSION_COOKIE
        || name == PAGE_GROUP_COOKIE
        || name == "AKA_A2"
        || AKAMAI_COOKIES.contains(&name)
}

/// The cookies one request is made with.
#[derive(Clone, Default)]
pub struct Session {
    cookies: BTreeMap<String, String>,
    /// The email a sign-in was made with, so a status command can name it.
    /// Not evidence of anything -- [`Session::account`] is.
    pub email: Option<String>,
}

/// Names only. The values are credentials.
impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("cookies", &self.cookies.keys().collect::<Vec<_>>())
            .field("email", &self.email)
            .field("account", &self.account())
            .finish()
    }
}

impl Session {
    pub fn from_cookies(cookies: BTreeMap<String, String>) -> Session {
        Session {
            cookies,
            email: None,
        }
    }

    pub fn with_email(mut self, email: Option<String>) -> Session {
        self.email = email;
        self
    }

    /// Whether these cookies speak for a person rather than a browser.
    ///
    /// A `__Host-SecureSessionID-p…` cookie, which only a successful sign-in
    /// sets. Testing for the prefix alone would be wrong: every visitor
    /// carries two or three `-t…` cookies of the same shape from the first
    /// page they load, so a browser that has never signed in would report as
    /// signed in and every account command would then fail obscurely instead
    /// of saying to log in.
    ///
    /// This is a local guess, and a cheap one. [`crate::Client::whoami`] asks
    /// the storefront, which is the answer that counts.
    pub fn account(&self) -> bool {
        self.cookies
            .iter()
            .any(|(name, value)| is_personalised(name) && !value.is_empty())
    }

    /// Whether this session has cookies an Akamai-gated call can use.
    ///
    /// `_abck` alone: it is the one the bot manager actually reads, and the
    /// others are worthless without it.
    pub fn warmed(&self) -> bool {
        self.cookies.get("_abck").is_some_and(|v| !v.is_empty())
    }

    /// Akamai's validation flag, from the second `~`-separated field of
    /// `_abck`.
    ///
    /// `0` means a JavaScript sensor ran and posted back; `-1` means nothing
    /// did. **This crate expects `-1` and works anyway**, and so does the
    /// browser that now buys its warmth -- a camoufox jar reads `-1` too and
    /// is served. So the sensor is not what the bot manager is holding out
    /// for, which is why nothing here tries to run one. Exposed so `doctor`
    /// can show it rather than because anything branches on it.
    pub fn validation(&self) -> Option<i32> {
        self.cookies
            .get("_abck")?
            .split('~')
            .nth(1)?
            .parse::<i32>()
            .ok()
    }

    pub fn cookies(&self) -> BTreeMap<String, String> {
        self.cookies.clone()
    }

    /// The names only, for narration and for `doctor`. Never the values.
    pub fn names(&self) -> Vec<String> {
        self.cookies.keys().cloned().collect()
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.cookies.get(name).map(String::as_str)
    }

    /// Fold in what a response set, so a session picks up cookies as it goes.
    ///
    /// An empty value is a deletion, which is how Intershop signs someone out:
    /// the logout page expires the personalised cookie rather than sending a
    /// new one.
    pub fn absorb(&mut self, headers: &wreq::header::HeaderMap) {
        for (name, value) in net_kit::cookies::set_cookies(headers) {
            if value.is_empty() || value == "\"\"" {
                self.cookies.remove(&name);
            } else {
                self.cookies.insert(name, value);
            }
        }
    }

    /// Replace the warmth with a browser's, and touch nothing else.
    ///
    /// The filter is the point, not tidiness. A browser that has just loaded
    /// the home page hands back an anonymous `sid` and two or three
    /// `-t…` secure-session cookies of its own, and adopting those wholesale
    /// would overwrite the session cookie of somebody who is *signed in* --
    /// a warm-up would quietly sign them out. Only the bot manager's cookies
    /// are a warm-up's to move, and they are sufficient on their own:
    /// measured, a jar of those eight alone was served the whole category
    /// tree. See [`is_warmth`].
    ///
    /// Replaced rather than merged, because they are a matched set. A
    /// leftover `_abck` beside a fresh `bm_sz` is the half-written jar that
    /// made the stale-cookie failure so hard to read.
    pub fn rewarm(&mut self, cookies: BTreeMap<String, String>) {
        self.forget_warmth();
        for (name, value) in cookies {
            if is_warmth(&name) && !value.is_empty() {
                self.cookies.insert(name, value);
            }
        }
    }

    /// Drop what speaks for a browser, keeping what speaks for a person.
    ///
    /// The opposite of [`Session::sign_out`], and the recovery for a stored
    /// `_abck` the site has since decided against. Such a cookie does not fail
    /// quietly -- it is refused on every request made with it, *including the
    /// warm-up meant to replace it*, so a client that keeps presenting it can
    /// never recover. Measured 2026-09-18: the same binary on the same profile
    /// in the same minute was refused carrying a stored session and served on
    /// a fresh jar.
    ///
    /// The account cookies stay. They are a separate grant and the bot manager
    /// has no opinion about them.
    pub fn forget_warmth(&mut self) {
        self.cookies
            .retain(|name, _| !AKAMAI_COOKIES.contains(&name.as_str()));
    }

    /// Drop everything that speaks for a person, keeping what speaks for a
    /// browser.
    ///
    /// Signing out should not cost the Akamai warm-up: it was earned by a
    /// request, it has nothing to do with the account, and throwing it away
    /// makes the next command pay for it again.
    pub fn sign_out(&mut self) {
        self.cookies
            .retain(|name, _| !is_personalised(name) && name != SESSION_COOKIE);
        self.email = None;
    }

    /// The `Cookie` header value, or `None` when there is nothing to send.
    pub fn header(&self) -> Option<String> {
        if self.cookies.is_empty() {
            return None;
        }
        Some(
            self.cookies
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("; "),
        )
    }
}

fn is_personalised(name: &str) -> bool {
    name.strip_prefix(SECURE_SESSION_PREFIX)
        .and_then(|rest| rest.chars().next())
        == Some(PERSONALISED)
}

/// A session as it is filed.
#[derive(Clone, Serialize, Deserialize)]
pub struct StoredSession {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub cookies: BTreeMap<String, String>,
    #[serde(default)]
    pub obtained_at: u64,
}

impl std::fmt::Debug for StoredSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredSession")
            .field("email", &self.email)
            .field("cookies", &self.cookies.keys().collect::<Vec<_>>())
            .field("obtained_at", &self.obtained_at)
            .finish()
    }
}

impl StoredSession {
    /// File a session, keeping only the cookies [`keep`] accepts.
    pub fn of(session: &Session) -> StoredSession {
        StoredSession {
            email: session.email.clone(),
            cookies: session
                .cookies
                .iter()
                .filter(|(name, _)| keep(name))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            obtained_at: net_kit::jwt::now_secs(),
        }
    }

    pub fn load(secrets: &Secrets) -> Result<Option<StoredSession>> {
        let Some(text) = secrets.get(ACCOUNT)? else {
            return Ok(None);
        };
        // A session written by an older build, or corrupted, is worth
        // discarding rather than failing every command until it is removed by
        // hand. The cost of being wrong is one warm-up.
        Ok(serde_json::from_str(&text).ok())
    }

    pub fn save(&self, secrets: &Secrets) -> Result<()> {
        let text =
            serde_json::to_string(self).map_err(|e| Error::decode("filing the session", e))?;
        Ok(secrets.set(ACCOUNT, &text)?)
    }

    pub fn clear(secrets: &Secrets) -> Result<bool> {
        Ok(secrets.delete(ACCOUNT)?)
    }

    pub fn session(&self) -> Session {
        Session::from_cookies(self.cookies.clone()).with_email(self.email.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use net_kit::Backend;

    fn with(pairs: &[(&str, &str)]) -> Session {
        Session::from_cookies(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    /// A transient secure-session cookie, of the kind every visitor gets.
    ///
    /// The real suffix is a 32-character hash, and it is a session identifier,
    /// so these say what they are for instead. Nothing reads past the kind
    /// letter -- which is the whole point of the pair.
    const GUEST_SECURE: &str = "__Host-SecureSessionID-tvisitor";
    /// The personalised one, which only a sign-in sets.
    const ACCOUNT_SECURE: &str = "__Host-SecureSessionID-paccount";

    #[test]
    fn a_visitors_secure_session_is_not_an_account_even_though_it_looks_like_one() {
        // Every visitor carries two or three of these from the first page
        // load. Matching the prefix rather than the kind letter would report
        // a browser that has never signed in as signed in.
        assert!(!with(&[(GUEST_SECURE, "abc"), (SESSION_COOKIE, "s")]).account());
        assert!(with(&[(ACCOUNT_SECURE, "abc")]).account());
        assert!(
            !with(&[(ACCOUNT_SECURE, "")]).account(),
            "an empty value is a deletion"
        );
    }

    #[test]
    fn the_akamai_flag_is_read_but_not_enforced() {
        // -1 is what a warm-up without a browser earns, and the REST API takes
        // it. Rejecting it would turn a working client into a broken one.
        // Shaped like the real cookie -- an opaque hash, the flag, then more
        // opaque -- but invented, because the real one is a live credential.
        let s = with(&[("_abck", "0123456789ABCDEF0123456789ABCDEF~-1~opaque")]);
        assert!(s.warmed());
        assert_eq!(s.validation(), Some(-1));
        assert_eq!(with(&[("_abck", "ABC~0~xyz")]).validation(), Some(0));
        assert_eq!(with(&[("_abck", "nonsense")]).validation(), None);
        assert!(!Session::default().warmed());
    }

    #[test]
    fn signing_out_keeps_the_warm_up_it_did_not_pay_for() {
        // The Akamai cookies were bought by a request and have nothing to do
        // with the account; dropping them would make the next command pay for
        // them again.
        let mut s = with(&[
            (ACCOUNT_SECURE, "abc"),
            (GUEST_SECURE, "def"),
            (SESSION_COOKIE, "s"),
            ("_abck", "ABC~-1~xyz"),
            ("bm_sz", "zzz"),
        ]);
        s.email = Some("shopper@example.invalid".into());
        s.sign_out();

        assert!(!s.account());
        assert_eq!(s.email, None);
        assert_eq!(s.get(SESSION_COOKIE), None);
        assert!(s.warmed(), "the warm-up survives a logout");
        assert_eq!(s.get("bm_sz"), Some("zzz"));
    }

    #[test]
    fn a_warm_up_replaces_the_warmth_and_touches_nothing_else() {
        // The browser hands back a whole visit -- its own `sid` and its own
        // transient secure-session cookies -- and taking those would sign out
        // whoever was signed in. Only the bot manager's cookies are a
        // warm-up's to move.
        let mut s = with(&[
            (ACCOUNT_SECURE, "the-account"),
            (SESSION_COOKIE, "the-signed-in-session"),
            ("_abck", "LAPSED~-1~x"),
            ("bm_mi", "stale"),
        ]);
        s.rewarm(
            [
                ("_abck", "FRESH~-1~x"),
                ("bm_sz", "fresh"),
                (SESSION_COOKIE, "the-browsers-own-visit"),
                (GUEST_SECURE, "the-browsers-own-visitor"),
            ]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        );

        assert!(s.account(), "still signed in");
        assert_eq!(s.get(SESSION_COOKIE), Some("the-signed-in-session"));
        assert_eq!(s.get(GUEST_SECURE), None, "the browser's visit stays there");
        assert_eq!(s.get("_abck"), Some("FRESH~-1~x"));
        assert_eq!(s.get("bm_sz"), Some("fresh"));
        // A matched set: the old cookies go even when the new jar has no
        // replacement for them.
        assert_eq!(s.get("bm_mi"), None);
    }

    #[test]
    fn an_expiring_cookie_is_removed_rather_than_stored_empty() {
        // Intershop signs someone out by expiring the cookie, not by sending
        // a new one -- so an empty value has to mean gone.
        let mut s = with(&[(ACCOUNT_SECURE, "abc")]);
        let mut headers = wreq::header::HeaderMap::new();
        headers.append(
            wreq::header::SET_COOKIE,
            format!("{ACCOUNT_SECURE}=; Path=/; Max-Age=0")
                .parse()
                .unwrap(),
        );
        s.absorb(&headers);
        assert!(!s.account());
    }

    #[test]
    fn cookies_are_sent_as_one_header_and_never_printed() {
        let s = with(&[(SESSION_COOKIE, "abc"), ("_abck", "secret-value")]);
        assert_eq!(s.header().as_deref(), Some("_abck=secret-value; sid=abc"));
        let text = format!("{s:?}");
        assert!(text.contains("_abck"), "names are fine");
        assert!(!text.contains("secret-value"), "{text}");
        assert!(Session::default().header().is_none());
    }

    #[test]
    fn only_the_cookies_worth_keeping_reach_the_credential_store() {
        // What someone browsed is not a credential and does not belong in one.
        let s = with(&[
            ("_abck", "a"),
            (SESSION_COOKIE, "b"),
            (ACCOUNT_SECURE, "c"),
            ("ssViewedProducts", "6867065002"),
            ("ConstructorioID_client_id", "an-id"),
        ]);
        let stored = StoredSession::of(&s);
        assert_eq!(stored.cookies.len(), 3, "{:?}", stored.cookies.keys());
        assert!(!stored.cookies.contains_key("ssViewedProducts"));
    }

    #[test]
    fn a_stored_session_round_trips_and_a_corrupt_one_reads_as_absent() {
        let dir = tempfile::TempDir::new().unwrap();
        let secrets = Secrets::new("farmers-api-test", Backend::File, dir.path());
        assert!(StoredSession::load(&secrets).unwrap().is_none());

        let session = with(&[(ACCOUNT_SECURE, "abc"), ("_abck", "ABC~-1~x")])
            .with_email(Some("shopper@example.invalid".into()));
        StoredSession::of(&session).save(&secrets).unwrap();

        let back = StoredSession::load(&secrets).unwrap().unwrap().session();
        assert!(back.account());
        assert!(back.warmed());
        assert_eq!(back.email.as_deref(), Some("shopper@example.invalid"));

        secrets.set(ACCOUNT, "{ truncated").unwrap();
        assert!(StoredSession::load(&secrets).unwrap().is_none());
    }
}
