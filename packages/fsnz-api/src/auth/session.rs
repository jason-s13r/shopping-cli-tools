//! The Club Plus session as this crate keeps it: the device identity the login
//! API insists on, the credential blob, and renewal on demand.

use net_kit::{Paths, Secrets};
use serde::{Deserialize, Serialize};

use crate::auth::clubplus::{self, Config, Session};
use crate::error::{Error, Result};
use crate::token::SKEW;

/// One Club Plus account serves both banners, so there is only ever one.
pub const ACCOUNT: &str = "clubplus";

/// A stable per-installation device identifier.
///
/// `POST /user/login` rejects a request without `x-device-id`. The browser
/// generates one and keeps it, so this does the same rather than presenting a
/// brand new device on every login -- which is what would trigger a
/// verification code every time.
pub fn device_id(paths: &Paths) -> Result<String> {
    let file = paths.state_file("device-id");
    if let Ok(existing) = std::fs::read_to_string(&file) {
        let existing = existing.trim().to_string();
        if !existing.is_empty() {
            return Ok(existing);
        }
    }
    let fresh = uuid::Uuid::new_v4().to_string();
    std::fs::create_dir_all(&paths.state_dir)
        .map_err(|e| net_kit::Error::io(format!("creating {}", paths.state_dir.display()), e))?;
    std::fs::write(&file, &fresh)
        .map_err(|e| net_kit::Error::io(format!("writing {}", file.display()), e))?;
    Ok(fresh)
}

#[derive(Clone, Serialize, Deserialize)]
pub struct StoredLogin {
    pub email: String,
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// When the session was last minted or renewed. Optional rather than
    /// defaulted to zero: "unknown" and "the epoch" are different things to
    /// report.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refreshed_at_ms: Option<u64>,
}

/// Redacted for the same reason as [`Session`]: this is what gets written to
/// the credential store, and it must not reach a log by being formatted.
impl std::fmt::Debug for StoredLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredLogin")
            .field("email", &self.email)
            .field("refreshed_at_ms", &self.refreshed_at_ms)
            .finish_non_exhaustive()
    }
}

impl StoredLogin {
    pub fn session(&self) -> Session {
        Session {
            access_token: self.access_token.clone(),
            refresh_token: self.refresh_token.clone(),
        }
    }

    pub fn expires_at_ms(&self) -> Option<u64> {
        net_kit::jwt::expiry_ms(&self.access_token)
    }

    /// Usable for a little longer yet. An unparseable token counts as stale, so
    /// the renewal path runs rather than a doomed request.
    pub fn is_fresh(&self) -> bool {
        self.expires_at_ms()
            .is_some_and(|at| net_kit::jwt::fresh(at, SKEW))
    }

    pub fn can_renew(&self) -> bool {
        self.refresh_token.is_some()
    }
}

/// A Club Plus session known to be usable, and how it was obtained.
pub struct ActiveSession {
    pub session: Session,
    /// True when this call had to renew to get here. A caller that then gets a
    /// 401 can tell "the session was stale" from "renewing did not help".
    pub renewed: bool,
}

/// Where an unattended renewal gets a password, when the refresh token is gone.
pub type PasswordSource = net_kit::password::Source;

/// The stored session, renewed first if it is at or past expiry.
///
/// This is what keeps a login working past its half-hour: every command needing
/// an account token comes through here, so the session is renewed on demand
/// rather than only when someone remembers to log in again.
///
/// Renewal is tried from the refresh token first and, when that is gone or has
/// been spent, from a password -- which is what makes an unattended run survive
/// longer than one refresh token. Neither path can answer a verification code,
/// so a device Club Plus wants to challenge still needs an interactive login.
///
/// The rotated refresh token is saved **before** the session is handed back. If
/// that write were skipped and the process then died, the token just spent
/// would be gone and the only way back would be a password.
pub async fn active_session(
    cfg: &Config<'_>,
    secrets: &Secrets,
    password: Option<&PasswordSource>,
    force: bool,
) -> Result<ActiveSession> {
    let stored = load(secrets)?.ok_or(Error::NotLoggedIn)?;

    if !force && stored.is_fresh() {
        return Ok(ActiveSession {
            session: stored.session(),
            renewed: false,
        });
    }

    // Kept rather than reported: a refusal here is only the end of the road if
    // there turns out to be no password, and the reason Club Plus gave is a
    // better answer then than a generic one.
    let mut refused = None;
    if let Some(refresh_token) = stored.refresh_token.as_deref() {
        match clubplus::refresh(cfg, refresh_token).await {
            Ok(session) => {
                persist(secrets, &stored.email, &session)?;
                return Ok(ActiveSession {
                    session,
                    renewed: true,
                });
            }
            // A refresh token already spent, or a session ended elsewhere.
            // Signing in again is the only way past it.
            Err(e) => refused = Some(e),
        }
    }

    // The password is fetched here and nowhere earlier: on a Mac that is a
    // keychain prompt, and the refresh above usually makes it unnecessary.
    let password = match password {
        Some(source) => source.password().await?,
        None => None,
    };
    let Some(password) = password else {
        return Err(refused.unwrap_or(Error::SessionUnrenewable));
    };
    let session = sign_in(cfg, &stored.email, &password).await?;
    persist(secrets, &stored.email, &session)?;
    Ok(ActiveSession {
        session,
        renewed: true,
    })
}

/// A whole new login, for when renewal is no longer possible.
///
/// Fails rather than prompts on a verification code: this runs underneath
/// ordinary commands, where there is not necessarily anyone to type one.
async fn sign_in(cfg: &Config<'_>, email: &str, password: &str) -> Result<Session> {
    match clubplus::login(cfg, email, password).await? {
        clubplus::Login::Complete(session) => Ok(session),
        clubplus::Login::ChallengeRequired(challenge) => Err(Error::VerificationRequired {
            method: Some(challenge.method.clone()),
            phv_token: String::new(),
        }),
    }
}

fn persist(secrets: &Secrets, email: &str, session: &Session) -> Result<()> {
    save(
        secrets,
        &StoredLogin {
            email: email.to_string(),
            access_token: session.access_token.clone(),
            refresh_token: session.refresh_token.clone(),
            refreshed_at_ms: Some(net_kit::jwt::now_ms()),
        },
    )
}

pub fn load(secrets: &Secrets) -> Result<Option<StoredLogin>> {
    match secrets.get(ACCOUNT)? {
        // A stored blob that will not parse is treated as absent: the fix is to
        // log in again, not to make every command fail.
        Some(raw) => Ok(serde_json::from_str(&raw).ok()),
        None => Ok(None),
    }
}

pub fn save(secrets: &Secrets, login: &StoredLogin) -> Result<()> {
    let raw =
        serde_json::to_string(login).map_err(|e| Error::decode("serialising the login", e))?;
    Ok(secrets.set(ACCOUNT, &raw)?)
}

pub fn clear(secrets: &Secrets) -> Result<bool> {
    Ok(secrets.delete(ACCOUNT)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use net_kit::Backend;

    fn store(dir: &tempfile::TempDir) -> Secrets {
        Secrets::new("fsnz-api-test", Backend::File, dir.path())
    }

    fn jwt(exp_secs: u64) -> String {
        let payload = serde_json::json!({ "exp": exp_secs }).to_string();
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload);
        format!("header.{encoded}.signature")
    }

    fn login(access_token: String, refresh: Option<&str>) -> StoredLogin {
        StoredLogin {
            email: "shopper@example.test".into(),
            access_token,
            refresh_token: refresh.map(str::to_string),
            refreshed_at_ms: None,
        }
    }

    #[test]
    fn a_device_id_is_generated_once_and_reused() {
        let dir = tempfile::TempDir::new().unwrap();
        let paths = Paths::new(dir.path().join("cfg"), dir.path().join("state"));
        let first = device_id(&paths).unwrap();
        assert!(!first.is_empty());
        // A new id on every login presents a new device, which triggers a
        // verification code every time.
        assert_eq!(device_id(&paths).unwrap(), first);
    }

    #[test]
    fn a_login_round_trips_through_the_credential_store() {
        let dir = tempfile::TempDir::new().unwrap();
        let s = store(&dir);
        assert!(load(&s).unwrap().is_none());
        save(&s, &login(jwt(1), Some("r1"))).unwrap();
        let back = load(&s).unwrap().unwrap();
        assert_eq!(back.email, "shopper@example.test");
        assert!(back.can_renew());
        assert!(clear(&s).unwrap());
        assert!(load(&s).unwrap().is_none());
    }

    #[test]
    fn an_unparseable_blob_reads_as_not_logged_in() {
        let dir = tempfile::TempDir::new().unwrap();
        let s = store(&dir);
        s.set(ACCOUNT, "{ truncated").unwrap();
        // The fix is to log in again, not to make every command fail.
        assert!(load(&s).unwrap().is_none());
    }

    #[test]
    fn freshness_is_read_off_the_token_and_an_unreadable_one_is_stale() {
        let future = net_kit::jwt::now_ms() / 1000 + 3600;
        assert!(login(jwt(future), None).is_fresh());
        assert!(!login(jwt(1), None).is_fresh(), "expired");
        assert!(
            !login("not-a-jwt".into(), None).is_fresh(),
            "unreadable counts as stale so the renewal path runs"
        );
    }
}

#[cfg(test)]
mod redaction_tests {
    use super::*;

    #[test]
    fn a_stored_login_does_not_print_its_tokens() {
        let login = StoredLogin {
            email: "shopper@example.test".into(),
            access_token: "secret-access".into(),
            refresh_token: Some("secret-refresh".into()),
            refreshed_at_ms: None,
        };
        let text = format!("{login:?}");
        assert!(text.contains("shopper@example.test"));
        assert!(!text.contains("secret-access"), "{text}");
        assert!(!text.contains("secret-refresh"), "{text}");
    }

    #[test]
    fn a_session_does_not_print_its_tokens() {
        let session = Session {
            access_token: "secret-access".into(),
            refresh_token: Some("secret-refresh".into()),
        };
        let text = format!("{session:?}");
        assert!(!text.contains("secret-access"), "{text}");
        assert!(!text.contains("secret-refresh"), "{text}");
        assert!(text.contains("redacted"), "{text}");
    }
}
