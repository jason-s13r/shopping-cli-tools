//! What the storefront is called with.
//!
//! Salesforce Commerce Cloud authorises entirely by cookie. Three matter:
//!
//! - `dwsid`, the storefront session. Present for anyone, and what a cart is
//!   hung off before there is an account.
//! - `usid_twl`, the shopper id that survives sign-in.
//! - `cc-at_twl`, a Salesforce SLAS shopper JWT. Readable, unlike the
//!   Woolworths equivalent, so its expiry can be checked rather than guessed
//!   at -- but note that **the storefront issues one to anyone**, signed in or
//!   not. Its presence is not a sign-in; its `isb` claim is what says whose it
//!   is.
//!
//! Unlike Woolworths, whose session cookie is encrypted and unrenewable, this
//! one is bought with an ordinary form POST that can simply be re-run.

use std::collections::BTreeMap;

use net_kit::{wreq, Secrets};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The storefront session, held by anyone.
pub const SESSION_COOKIE: &str = "dwsid";

/// The shopper token: a SLAS JWT, held by guests as well as by account
/// holders.
pub const ACCOUNT_COOKIE: &str = "cc-at_twl";

/// The claim naming the shopper the token was issued for, as a `::`-separated
/// list of `key:value` pairs. A guest's reads `upn:Guest::uidn:Guest User`.
const SHOPPER_CLAIM: &str = "isb";

/// The shopper id, which outlives a session.
pub const SHOPPER_COOKIE: &str = "usid_twl";

/// The refresh token for a **registered** shopper, set only by a successful
/// sign-in.
///
/// Its guest counterpart is `cc-nx-g_twl`, and the login response expires that
/// one as it sets this. The pair is the clearest signal there is that a session
/// belongs to a person: it needs no JWT decoding and no claim parsing.
pub const REFRESH_COOKIE: &str = "cc-nx_twl";

/// The guest refresh token, which a signed-in session does not have.
pub const GUEST_REFRESH_COOKIE: &str = "cc-nx-g_twl";

/// Where a stored session is filed in the credential store.
pub const ACCOUNT: &str = "session";

/// Where Cloudflare's clearance is filed, apart from the session: it belongs to
/// the browser that earned it, and outlives a sign-out.
pub const CLEARANCE: &str = "clearance";

/// Cloudflare's cookies: `cf_clearance`, which a browser earns by passing the
/// challenge, and the `__cf_*` bot-management cookies beside it.
pub fn is_clearance(name: &str) -> bool {
    name == "cf_clearance" || name.starts_with("__cf")
}

/// Re-sign-in this long before the token expires, so one does not lapse midway
/// through a command that makes several calls.
const EXPIRY_MARGIN_SECS: u64 = 60;

/// The cookies one request is made with.
#[derive(Clone, Default)]
pub struct Session {
    cookies: BTreeMap<String, String>,
}

/// Names only. The values are credentials.
impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("cookies", &self.cookies.keys().collect::<Vec<_>>())
            .field("account", &self.account())
            .finish()
    }
}

impl Session {
    pub fn from_cookies(cookies: BTreeMap<String, String>) -> Session {
        Session { cookies }
    }

    /// Whether these cookies speak for a person rather than a browser.
    ///
    /// Two independent tells, because each covers the other's gap:
    ///
    /// - `cc-nx_twl`, the registered refresh token, which only a successful
    ///   sign-in sets. Cheap and needs no parsing.
    /// - the access token's `isb` claim naming a shopper. This survives a
    ///   refresh token that was dropped, and it is what [`Session::shopper`]
    ///   reads to say *who*.
    ///
    /// Testing for `cc-at_twl` alone would be wrong: the storefront hands one
    /// of those to every visitor, signed in or not.
    pub fn account(&self) -> bool {
        self.cookies
            .get(REFRESH_COOKIE)
            .is_some_and(|t| !t.is_empty())
            || self.shopper().is_some()
    }

    /// Who the token was issued for, or `None` for a guest.
    ///
    /// The username out of the `isb` claim -- an email for a signed-in shopper.
    /// Worth reading rather than trusting the stored email: a session that has
    /// been re-issued as a guest still sits next to the email it was obtained
    /// with, and only this can tell them apart.
    pub fn shopper(&self) -> Option<String> {
        let token = self.cookies.get(ACCOUNT_COOKIE)?;
        let isb = net_kit::jwt::claim_str(token, SHOPPER_CLAIM)?;
        let upn = isb
            .split("::")
            .find_map(|part| part.strip_prefix("upn:"))?
            .trim();
        (!upn.is_empty() && upn != "Guest").then(|| upn.to_string())
    }

    pub fn cookies(&self) -> BTreeMap<String, String> {
        self.cookies.clone()
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.cookies.get(name).map(String::as_str)
    }

    /// Fold in what a response set, so a session picks up the cookies the
    /// storefront hands out as it goes.
    pub fn absorb(&mut self, headers: &wreq::header::HeaderMap) {
        for (name, value) in net_kit::cookies::set_cookies(headers) {
            if value.is_empty() {
                self.cookies.remove(&name);
            } else {
                self.cookies.insert(name, value);
            }
        }
    }

    /// Only the Cloudflare cookies. See [`is_clearance`].
    pub fn clearance(&self) -> BTreeMap<String, String> {
        self.cookies
            .iter()
            .filter(|(name, _)| is_clearance(name))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Take Cloudflare's cookies from a jar, and nothing else from it: a
    /// browser's own `dwsid` and shopper tokens would overwrite this session's.
    pub fn adopt_clearance(&mut self, jar: BTreeMap<String, String>) {
        self.cookies
            .extend(jar.into_iter().filter(|(name, _)| is_clearance(name)));
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

    /// When the account token expires, when it is a readable JWT.
    pub fn expires_at(&self) -> Option<u64> {
        net_kit::jwt::expiry_ms(self.cookies.get(ACCOUNT_COOKIE)?).map(|ms| ms / 1000)
    }

    /// Whether the shopper token has run out, or is about to.
    ///
    /// A token with no readable expiry is treated as good: guessing it lapsed
    /// would sign in again on every command.
    pub fn lapsed(&self) -> bool {
        self.expires_at()
            .is_some_and(|exp| exp <= net_kit::jwt::now_secs() + EXPIRY_MARGIN_SECS)
    }
}

/// A stored account session.
#[derive(Clone, Serialize, Deserialize)]
pub struct StoredSession {
    /// The email it was obtained for, so a status command can name it.
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
    pub fn of(session: &Session, email: Option<String>) -> StoredSession {
        StoredSession {
            email,
            cookies: session.cookies(),
            obtained_at: net_kit::jwt::now_secs(),
        }
    }

    pub fn load(secrets: &Secrets) -> Result<Option<StoredSession>> {
        let Some(text) = secrets.get(ACCOUNT)? else {
            return Ok(None);
        };
        // A session written by an older build, or corrupted, is worth
        // discarding rather than failing every command until it is removed by
        // hand.
        Ok(serde_json::from_str(&text).ok())
    }

    pub fn save(&self, secrets: &Secrets) -> Result<()> {
        let text =
            serde_json::to_string(self).map_err(|e| Error::decode("serialising the session", e))?;
        Ok(secrets.set(ACCOUNT, &text)?)
    }

    pub fn clear(secrets: &Secrets) -> Result<bool> {
        Ok(secrets.delete(ACCOUNT)?)
    }

    pub fn session(&self) -> Session {
        Session::from_cookies(self.cookies.clone())
    }
}

/// The stored Cloudflare clearance, or nothing. Unreadable is nothing too: the
/// cost is one browser launch.
pub fn load_clearance(secrets: &Secrets) -> Result<BTreeMap<String, String>> {
    let Some(text) = secrets.get(CLEARANCE)? else {
        return Ok(BTreeMap::new());
    };
    Ok(serde_json::from_str(&text).unwrap_or_default())
}

pub fn save_clearance(secrets: &Secrets, jar: &BTreeMap<String, String>) -> Result<()> {
    let jar: BTreeMap<_, _> = jar.iter().filter(|(name, _)| is_clearance(name)).collect();
    let text =
        serde_json::to_string(&jar).map_err(|e| Error::decode("serialising the clearance", e))?;
    Ok(secrets.set(CLEARANCE, &text)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use net_kit::Backend;

    fn jwt(exp: u64) -> String {
        token(exp, "uido:slas::upn:shopper@example.test::uidn:A Shopper")
    }

    fn token(exp: u64, isb: &str) -> String {
        use base64::Engine;
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::json!({ "exp": exp, "isb": isb }).to_string());
        format!("header.{payload}.signature")
    }

    fn with(pairs: &[(&str, &str)]) -> Session {
        Session::from_cookies(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    #[test]
    fn only_cloudflares_cookies_are_adopted_from_a_browser() {
        let mut session = with(&[(SESSION_COOKIE, "mine")]);
        session.adopt_clearance(BTreeMap::from([
            ("cf_clearance".to_string(), "c".to_string()),
            ("__cf_bm".to_string(), "b".to_string()),
            (SESSION_COOKIE.to_string(), "the-browsers".to_string()),
        ]));
        assert_eq!(session.get(SESSION_COOKIE), Some("mine"));
        assert_eq!(
            session.clearance().keys().collect::<Vec<_>>(),
            ["__cf_bm", "cf_clearance"]
        );
    }

    #[test]
    fn a_guests_token_is_not_an_account_even_though_it_is_a_real_token() {
        // The storefront hands a SLAS token to every visitor, so testing for
        // the cookie would report a browser that has never signed in as signed
        // in -- and every account-only command would then fail obscurely
        // instead of saying to log in.
        let guest = token(
            net_kit::jwt::now_secs() + 3600,
            "uido:slas::upn:Guest::uidn:Guest User::gcid:abc",
        );
        let session = with(&[(ACCOUNT_COOKIE, &guest)]);
        assert!(!session.account());
        assert_eq!(session.shopper(), None);
    }

    #[test]
    fn the_registered_refresh_token_is_enough_on_its_own() {
        // The login response sets this and expires the guest one beside it, so
        // it is the plainest signal there is -- and it holds even when the
        // access token has not been reissued yet.
        let guest_at = token(
            net_kit::jwt::now_secs() + 3600,
            "uido:slas::upn:Guest::uidn:Guest User",
        );
        let session = with(&[(ACCOUNT_COOKIE, &guest_at), (REFRESH_COOKIE, "r")]);
        assert!(session.account());
    }

    #[test]
    fn the_guest_refresh_token_is_not_the_registered_one() {
        // One character apart in the name, opposite in meaning.
        assert!(!with(&[(GUEST_REFRESH_COOKIE, "r")]).account());
    }

    #[test]
    fn a_signed_in_token_names_the_shopper_it_was_issued_for() {
        let session = with(&[(ACCOUNT_COOKIE, &jwt(net_kit::jwt::now_secs() + 3600))]);
        assert!(session.account());
        assert_eq!(session.shopper().as_deref(), Some("shopper@example.test"));
    }

    #[test]
    fn a_storefront_session_alone_is_not_an_account() {
        assert!(!with(&[(SESSION_COOKIE, "abc")]).account());
        assert!(!with(&[(ACCOUNT_COOKIE, "")]).account());
        assert!(!with(&[(ACCOUNT_COOKIE, "not-a-jwt")]).account());
    }

    #[test]
    fn cookies_are_sent_as_one_header_and_never_printed() {
        let s = with(&[(SESSION_COOKIE, "abc"), (ACCOUNT_COOKIE, "secret-value")]);
        assert_eq!(
            s.header().as_deref(),
            Some("cc-at_twl=secret-value; dwsid=abc")
        );
        let text = format!("{s:?}");
        assert!(text.contains(ACCOUNT_COOKIE));
        assert!(!text.contains("secret-value"), "{text}");
        assert!(Session::default().header().is_none());
    }

    #[test]
    fn an_expiring_token_is_lapsed_before_it_actually_expires() {
        assert!(!with(&[(ACCOUNT_COOKIE, &jwt(net_kit::jwt::now_secs() + 3600))]).lapsed());
        // Inside the margin: treated as gone, so a command making several calls
        // does not have one lapse midway.
        assert!(with(&[(ACCOUNT_COOKIE, &jwt(net_kit::jwt::now_secs() + 30))]).lapsed());
    }

    #[test]
    fn a_token_with_no_readable_expiry_is_taken_at_face_value() {
        // Guessing it had lapsed would sign in again on every single command.
        let s = with(&[(ACCOUNT_COOKIE, "not-a-jwt")]);
        assert!(!s.lapsed());
        assert_eq!(s.expires_at(), None);
    }

    #[test]
    fn a_stored_session_round_trips_and_a_corrupt_one_reads_as_absent() {
        let dir = tempfile::TempDir::new().unwrap();
        let secrets = Secrets::new("twlnz-api-test", Backend::File, dir.path());
        assert!(StoredSession::load(&secrets).unwrap().is_none());

        StoredSession::of(
            &with(&[(ACCOUNT_COOKIE, &jwt(net_kit::jwt::now_secs() + 3600))]),
            Some("shopper@example.test".into()),
        )
        .save(&secrets)
        .unwrap();
        assert!(StoredSession::load(&secrets)
            .unwrap()
            .unwrap()
            .session()
            .account());

        secrets.set(ACCOUNT, "{ truncated").unwrap();
        assert!(StoredSession::load(&secrets).unwrap().is_none());
    }
}
