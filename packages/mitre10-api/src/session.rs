//! What is worth keeping between runs.
//!
//! Two tokens, earned once and refreshed forever:
//!
//! | | where it comes from | how long it lasts |
//! |---|---|---|
//! | refresh token | one sign-in with a password | until it is revoked |
//! | access token | minted from the above | `expires_in`, three hours |
//!
//! Only the refresh token is precious, which is why this tool stores one and
//! never a password. There is no captcha anywhere in the flow, so a lapsed
//! refresh token costs a password prompt and nothing more.

use std::time::Duration;

use net_kit::{jwt, Secrets};
use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Renew rather than send a token this close to expiry. A token that expires
/// during the request it authorises fails in a way that reads as bad
/// credentials.
pub const SKEW: Duration = Duration::from_secs(60);

/// What the storefront's `expires_in` says, for a token whose `exp` will not
/// parse: 10799 seconds, three hours less a second.
const ASSUMED_LIFETIME: Duration = Duration::from_secs(3 * 60 * 60);

/// The account name the session is filed under. One storefront, so one entry.
const ACCOUNT: &str = "session";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Session {
    /// The bearer, a JWT.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    /// The thing worth keeping.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// When the bearer expires, in milliseconds. Read off the JWT where it
    /// parses, assumed where it does not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    /// Whose account this is, for `auth status` to have something to show.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

impl Session {
    /// Whether the bearer is good for the next request.
    pub fn token_fresh(&self) -> bool {
        match (&self.access_token, self.expires_at) {
            (Some(_), Some(at)) => jwt::fresh(at, SKEW),
            // A token with no readable expiry is worth one attempt: the call
            // that fails is cheaper than a sign-in that was not needed.
            (Some(_), None) => true,
            _ => false,
        }
    }

    /// Whether there is anything to refresh *from*.
    pub fn can_refresh(&self) -> bool {
        self.refresh_token.is_some()
    }

    pub fn bearer(&self) -> Option<String> {
        Some(format!("Bearer {}", self.access_token.as_ref()?))
    }

    /// File a newly minted pair, reading the expiry off the JWT where it
    /// parses and falling back to what the storefront said.
    pub fn set_tokens(&mut self, access: String, refresh: Option<String>, expires_in: Option<u64>) {
        self.expires_at = jwt::expiry_ms(&access).or_else(|| {
            let secs = expires_in.unwrap_or(ASSUMED_LIFETIME.as_secs());
            Some(jwt::now_ms() + secs.saturating_mul(1000))
        });
        // The storefront reissues a refresh token on every exchange, but a
        // refresh that answers without one must not clear the one that worked.
        if refresh.is_some() {
            self.refresh_token = refresh;
        }
        self.email = jwt::claim_str(&access, "sub").or_else(|| self.email.clone());
        self.access_token = Some(access);
    }

    /// Forget the bearer but keep the refresh token, which is what a `401`
    /// should cost: the next command mints a new one silently.
    pub fn clear_token(&mut self) {
        self.access_token = None;
        self.expires_at = None;
    }
}

/// A session as it is filed.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StoredSession {
    #[serde(flatten)]
    pub session: Session,
}

impl StoredSession {
    pub fn load(secrets: &Secrets) -> Result<Option<StoredSession>> {
        let Some(raw) = secrets.get(ACCOUNT)? else {
            return Ok(None);
        };
        // A credential store holding something this version cannot read is not
        // worth failing over: the fix is to sign in again, and saying "not
        // signed in" leads there.
        Ok(serde_json::from_str(&raw).ok())
    }

    pub fn save(secrets: &Secrets, session: &Session) -> Result<()> {
        let stored = StoredSession {
            session: session.clone(),
        };
        let raw = serde_json::to_string(&stored)
            .map_err(|e| net_kit::Error::decode("filing the session", e))?;
        secrets.set(ACCOUNT, &raw)?;
        Ok(())
    }

    pub fn delete(secrets: &Secrets) -> Result<bool> {
        Ok(secrets.delete(ACCOUNT)?)
    }

    pub fn session(&self) -> Session {
        self.session.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(dir: &std::path::Path) -> Secrets {
        Secrets::new("mitre10-test", net_kit::Backend::File, dir)
    }

    #[test]
    fn a_session_with_no_bearer_is_not_fresh_but_may_still_refresh() {
        let mut s = Session {
            refresh_token: Some("NotARealToken".into()),
            ..Default::default()
        };
        assert!(!s.token_fresh());
        assert!(s.can_refresh());

        s.set_tokens("not.a.jwt".into(), None, Some(10_799));
        assert!(s.token_fresh(), "an unreadable expiry is worth one attempt");
        assert!(s.expires_at.is_some(), "and gets one from expires_in");
    }

    #[test]
    fn a_refresh_that_returns_no_new_token_keeps_the_one_that_worked() {
        // The exchange usually reissues, but an answer without a refresh token
        // must not log someone out.
        let mut s = Session {
            refresh_token: Some("the-good-one".into()),
            ..Default::default()
        };
        s.set_tokens("access".into(), None, Some(10_799));
        assert_eq!(s.refresh_token.as_deref(), Some("the-good-one"));

        s.set_tokens("access2".into(), Some("a-newer-one".into()), Some(10_799));
        assert_eq!(s.refresh_token.as_deref(), Some("a-newer-one"));
    }

    #[test]
    fn an_expired_bearer_is_not_fresh() {
        let mut s = Session::default();
        s.set_tokens("x".into(), None, Some(10_799));
        s.expires_at = Some(jwt::now_ms().saturating_sub(1));
        assert!(!s.token_fresh());
    }

    #[test]
    fn clearing_the_bearer_keeps_the_token_that_can_mint_another() {
        let mut s = Session {
            refresh_token: Some("NotARealToken".into()),
            ..Default::default()
        };
        s.set_tokens("x".into(), None, None);
        s.clear_token();
        assert!(s.access_token.is_none());
        assert!(s.can_refresh(), "a 401 must not cost the sign-in");
    }

    #[test]
    fn a_session_survives_a_round_trip_through_the_store() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let secrets = store(dir.path());
        assert!(StoredSession::load(&secrets).expect("loads").is_none());

        let session = Session {
            refresh_token: Some("NotARealToken".into()),
            email: Some("shopper@example.invalid".into()),
            ..Default::default()
        };
        StoredSession::save(&secrets, &session).expect("saves");

        let back = StoredSession::load(&secrets)
            .expect("loads")
            .expect("was saved");
        assert_eq!(back.session.refresh_token.as_deref(), Some("NotARealToken"));
        assert!(StoredSession::delete(&secrets).expect("deletes"));
        assert!(StoredSession::load(&secrets).expect("loads").is_none());
    }

    #[test]
    fn an_unreadable_stored_session_reads_as_absent_rather_than_failing() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let secrets = store(dir.path());
        secrets.set(ACCOUNT, "{not json").expect("writes");
        assert!(StoredSession::load(&secrets)
            .expect("does not fail")
            .is_none());
    }
}
