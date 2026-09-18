//! A password kept beside a session, so a login that has lapsed can be renewed
//! with nobody at the keyboard.
//!
//! A password is a worse thing to hold than a session: it does not expire, and
//! it is the whole account rather than one device's access to it. So it is only
//! written when there is nothing better -- a configured command wins where one
//! is set, leaving a password manager as the single copy -- and logging out
//! removes it along with everything else.

use crate::error::Result;
use crate::run;
use crate::secrets::Secrets;

/// Filed apart from the session on purpose: renewing a session rewrites the
/// session blob, and a password living inside it would be dropped the first
/// time a token was refreshed.
pub const ACCOUNT: &str = "password";

/// Stored as a JSON string rather than raw. The file backend trims what it
/// reads back, which would quietly corrupt a password with leading or trailing
/// space.
pub fn save(secrets: &Secrets, password: &str) -> Result<()> {
    let raw = serde_json::to_string(password).expect("a string always serialises");
    secrets.set(ACCOUNT, &raw)
}

pub fn load(secrets: &Secrets) -> Result<Option<String>> {
    Ok(secrets
        .get(ACCOUNT)?
        .and_then(|raw| serde_json::from_str::<String>(&raw).ok())
        .filter(|p| !p.is_empty()))
}

pub fn clear(secrets: &Secrets) -> Result<bool> {
    secrets.delete(ACCOUNT)
}

/// Where an unattended login gets its password. Nothing is fetched until
/// [`Source::password`] is called.
#[derive(Clone, Debug)]
pub enum Source {
    /// A configured command that prints it on stdout.
    Command(String),
    /// The copy in the credential store.
    Stored(Secrets),
}

impl Source {
    /// Where the password would come from. The command first: where one is
    /// configured it is the account's real source of truth, and a login keeps
    /// no copy alongside it.
    pub fn named(command: Option<&str>, secrets: &Secrets) -> Source {
        match command.map(str::trim).filter(|c| !c.is_empty()) {
            Some(cmd) => Source::Command(cmd.to_string()),
            None => Source::Stored(secrets.clone()),
        }
    }

    /// The same, `None` where there is nothing to sign in with. Costs a store
    /// access to find out, so [`Source::named`] is the one to build a client
    /// with.
    pub fn resolve(command: Option<&str>, secrets: &Secrets) -> Result<Option<Source>> {
        let source = Source::named(command, secrets);
        Ok(source.exists()?.then_some(source))
    }

    /// Whether there is really a password here -- the question `auth status`
    /// answers. A configured command is taken at its word rather than run.
    pub fn exists(&self) -> Result<bool> {
        match self {
            Source::Command(_) => Ok(true),
            Source::Stored(secrets) => Ok(load(secrets)?.is_some()),
        }
    }

    /// The password, or `None` where there is none to be had -- which the
    /// caller reports, because only it knows what that means.
    pub async fn password(&self) -> Result<Option<String>> {
        match self {
            Source::Command(cmd) => run::capturing("password_command", cmd).await.map(Some),
            Source::Stored(secrets) => load(secrets),
        }
    }

    pub fn describe(&self) -> &'static str {
        match self {
            Source::Command(_) => "the configured password_command",
            Source::Stored(_) => "the stored password",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::Backend;
    use tempfile::TempDir;

    fn store(dir: &TempDir) -> Secrets {
        Secrets::new("net-kit-test", Backend::File, dir.path())
    }

    #[test]
    fn round_trips_a_password() {
        let dir = TempDir::new().unwrap();
        let s = store(&dir);
        assert_eq!(load(&s).unwrap(), None);
        save(&s, "hunter2").unwrap();
        assert_eq!(load(&s).unwrap().as_deref(), Some("hunter2"));
        assert!(clear(&s).unwrap());
        assert_eq!(load(&s).unwrap(), None);
    }

    #[test]
    fn surrounding_space_survives_the_round_trip() {
        let dir = TempDir::new().unwrap();
        let s = store(&dir);
        save(&s, "  spaced  ").unwrap();
        assert_eq!(load(&s).unwrap().as_deref(), Some("  spaced  "));
    }

    #[test]
    fn a_configured_command_beats_the_stored_copy() {
        let dir = TempDir::new().unwrap();
        let s = store(&dir);
        save(&s, "stored").unwrap();
        assert!(matches!(
            Source::resolve(Some("pass show clubplus"), &s).unwrap(),
            Some(Source::Command(_))
        ));
        // Blank is not configured.
        assert!(matches!(
            Source::resolve(Some("   "), &s).unwrap(),
            Some(Source::Stored(_))
        ));
    }

    #[test]
    fn nothing_stored_and_no_command_is_no_source() {
        let dir = TempDir::new().unwrap();
        assert!(Source::resolve(None, &store(&dir)).unwrap().is_none());
    }

    /// The point of naming one: a client that never renews never reads it.
    #[tokio::test]
    async fn a_named_source_reads_nothing_until_it_is_spent() {
        let dir = TempDir::new().unwrap();
        let s = store(&dir);
        let source = Source::named(None, &s);
        save(&s, "hunter2").unwrap();
        assert_eq!(source.password().await.unwrap().as_deref(), Some("hunter2"));
    }

    #[tokio::test]
    async fn a_named_source_with_nothing_behind_it_is_not_an_error() {
        let dir = TempDir::new().unwrap();
        assert_eq!(
            Source::named(None, &store(&dir)).password().await.unwrap(),
            None
        );
    }
}
