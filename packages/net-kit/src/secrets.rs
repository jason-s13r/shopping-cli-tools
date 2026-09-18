//! Somewhere to keep a login that is not a plaintext file in a repo.
//!
//! Preference is the operating system's own credential store -- Keychain,
//! Credential Manager, Secret Service -- via `keyring`, which papers over the
//! differences. Where none is reachable (a headless box with no Secret
//! Service) it falls back to a 0600 file under the state directory, and says
//! so rather than pretending.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use crate::error::{Error, Result};
use crate::paths::restrict;

/// Reduce a name to something that is one path segment and cannot escape it.
fn safe(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Backend {
    /// The platform credential store.
    Keyring,
    /// A 0600 file, for platforms without one.
    File,
}

impl Backend {
    /// What this platform offers. The caller decides whether to override it --
    /// tests always do, to stay off the developer's real credential store.
    pub fn detect() -> Backend {
        if keyring::Entry::store_status().is_ok() {
            Backend::Keyring
        } else {
            Backend::File
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Backend::Keyring => "the system credential store",
            Backend::File => "a 0600 file in the state directory (no system credential store)",
        }
    }
}

/// Every access is a keychain prompt on a Mac, so each value is fetched once
/// per process. Held per store rather than per [`Secrets`]: two of these
/// naming one store are one store, and neither should ask twice.
type Memo = Arc<Mutex<HashMap<String, Option<String>>>>;

/// What identifies a store: the same three, the same secrets.
type Id = (String, Backend, PathBuf);

static MEMOS: LazyLock<Mutex<HashMap<Id, Memo>>> = LazyLock::new(Default::default);

/// `Clone` because a client that can re-authenticate has to carry one.
#[derive(Clone)]
pub struct Secrets {
    service: String,
    backend: Backend,
    dir: PathBuf,
    memo: Memo,
}

/// Written by hand because the derived one would print the memo, and the memo
/// is the secrets.
impl std::fmt::Debug for Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Secrets")
            .field("service", &self.service)
            .field("backend", &self.backend)
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

impl Secrets {
    /// `service` names the tool in the credential store, and is why two CLIs
    /// on one machine do not read each other's logins.
    pub fn new(service: impl Into<String>, backend: Backend, state_dir: &Path) -> Secrets {
        let service = service.into();
        let dir = state_dir.join("secrets");
        let memo = MEMOS
            .lock()
            .expect("secrets memos")
            .entry((service.clone(), backend, dir.clone()))
            .or_default()
            .clone();
        Secrets {
            service,
            backend,
            dir,
            memo,
        }
    }

    pub fn backend(&self) -> Backend {
        self.backend
    }

    pub fn service(&self) -> &str {
        &self.service
    }

    /// Fetched once per process. See [`Memo`].
    pub fn get(&self, account: &str) -> Result<Option<String>> {
        if let Some(known) = self.remembered(account) {
            return Ok(known);
        }
        let found = match self.backend {
            Backend::Keyring => match self.entry(account)?.get_password() {
                Ok(secret) => Ok(Some(secret).filter(|s| !s.trim().is_empty())),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => Err(Error::keyring("reading from the credential store", e)),
            },
            Backend::File => match fs::read_to_string(self.path(account)) {
                Ok(s) => Ok(Some(s.trim().to_string()).filter(|s| !s.is_empty())),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(Error::io("reading the stored secret", e)),
            },
        }?;
        self.remember(account, found.clone());
        Ok(found)
    }

    /// A write that would change nothing is skipped: it costs an access too.
    pub fn set(&self, account: &str, secret: &str) -> Result<()> {
        let readable = self.as_read(secret);
        if self.remembered(account) == Some(readable.clone()) {
            return Ok(());
        }
        match self.backend {
            Backend::Keyring => self
                .entry(account)?
                .set_password(secret)
                .map_err(|e| Error::keyring("writing to the credential store", e))?,
            Backend::File => {
                let path = self.path(account);
                // The service is a directory level, so `self.dir` alone is not
                // enough to create.
                let parent = path.parent().unwrap_or(&self.dir);
                fs::create_dir_all(parent)
                    .map_err(|e| Error::io(format!("creating {}", parent.display()), e))?;
                fs::write(&path, secret)
                    .map_err(|e| Error::io(format!("writing {}", path.display()), e))?;
                restrict(&path);
            }
        }
        self.remember(account, readable);
        Ok(())
    }

    /// Removing something that was never there is a success, not an error.
    pub fn delete(&self, account: &str) -> Result<bool> {
        if self.remembered(account) == Some(None) {
            return Ok(false);
        }
        let removed = match self.backend {
            Backend::Keyring => match self.entry(account)?.delete_credential() {
                Ok(()) => Ok(true),
                Err(keyring::Error::NoEntry) => Ok(false),
                Err(e) => Err(Error::keyring("deleting from the credential store", e)),
            },
            Backend::File => match fs::remove_file(self.path(account)) {
                Ok(()) => Ok(true),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(e) => Err(Error::io("removing the stored secret", e)),
            },
        }?;
        self.remember(account, None);
        Ok(removed)
    }

    /// What a read would give back, which the file backend trims.
    fn as_read(&self, secret: &str) -> Option<String> {
        match self.backend {
            Backend::Keyring => Some(secret.to_string()).filter(|s| !s.trim().is_empty()),
            Backend::File => Some(secret.trim().to_string()).filter(|s| !s.is_empty()),
        }
    }

    /// Outer `Option`: whether this process has looked. Inner: what it found.
    fn remembered(&self, account: &str) -> Option<Option<String>> {
        self.memo
            .lock()
            .expect("secrets memo")
            .get(account)
            .cloned()
    }

    fn remember(&self, account: &str, secret: Option<String>) {
        self.memo
            .lock()
            .expect("secrets memo")
            .insert(account.to_string(), secret);
    }

    fn entry(&self, account: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(&self.service, account)
            .map_err(|e| Error::keyring("opening the credential store", e))
    }

    /// Both names reach the filesystem in the fallback backend, so both are
    /// reduced to something that cannot climb out of the directory.
    ///
    /// The service is part of the path so that two tools pointed at one state
    /// directory cannot read each other's secrets -- the keyring backend
    /// separates them by service, and the fallback should not be weaker.
    fn path(&self, account: &str) -> PathBuf {
        self.dir.join(safe(&self.service)).join(safe(account))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// No `set_var` anywhere: the backend is an argument, so these tests can
    /// run in parallel and never touch a real credential store.
    fn file_store(dir: &TempDir) -> Secrets {
        Secrets::new("net-kit-test", Backend::File, dir.path())
    }

    #[test]
    fn round_trips_a_secret() {
        let dir = TempDir::new().unwrap();
        let s = file_store(&dir);
        assert_eq!(s.get("session").unwrap(), None);
        s.set("session", "a-stored-session").unwrap();
        assert_eq!(
            s.get("session").unwrap().as_deref(),
            Some("a-stored-session")
        );
        assert!(s.delete("session").unwrap());
        assert_eq!(s.get("session").unwrap(), None);
    }

    #[test]
    fn deleting_nothing_is_not_an_error() {
        let dir = TempDir::new().unwrap();
        assert!(!file_store(&dir).delete("absent").unwrap());
    }

    #[test]
    fn account_names_cannot_escape_the_directory() {
        let dir = TempDir::new().unwrap();
        let s = file_store(&dir);
        let inside = s.dir.join("net_kit_test");
        assert_eq!(s.path("../../etc/passwd").parent(), Some(inside.as_path()));
        assert_eq!(s.path("a/b").parent(), Some(inside.as_path()));
    }

    #[test]
    fn two_tools_sharing_a_state_directory_stay_separate() {
        let dir = TempDir::new().unwrap();
        let a = Secrets::new("tool-a", Backend::File, dir.path());
        let b = Secrets::new("tool-b", Backend::File, dir.path());
        a.set("session", "from-a").unwrap();
        assert_eq!(b.get("session").unwrap(), None);
        b.set("session", "from-b").unwrap();
        assert_eq!(a.get("session").unwrap().as_deref(), Some("from-a"));
    }

    /// The file is taken away, so only memory can answer.
    #[test]
    fn a_value_already_read_is_not_fetched_twice() {
        let dir = TempDir::new().unwrap();
        let s = file_store(&dir);
        s.set("session", "a-stored-session").unwrap();
        fs::remove_file(s.path("session")).unwrap();
        assert_eq!(
            s.get("session").unwrap().as_deref(),
            Some("a-stored-session")
        );
    }

    #[test]
    fn clones_share_what_the_original_knows() {
        let dir = TempDir::new().unwrap();
        let s = file_store(&dir);
        s.set("session", "a-stored-session").unwrap();
        let carried = s.clone();
        fs::remove_file(s.path("session")).unwrap();
        assert_eq!(
            carried.get("session").unwrap().as_deref(),
            Some("a-stored-session")
        );
    }

    /// Naming the same store twice is not a second store, and must not be a
    /// second prompt.
    #[test]
    fn a_store_named_again_knows_what_the_first_one_read() {
        let dir = TempDir::new().unwrap();
        let s = file_store(&dir);
        s.set("session", "a-stored-session").unwrap();
        fs::remove_file(s.path("session")).unwrap();
        assert_eq!(
            file_store(&dir).get("session").unwrap().as_deref(),
            Some("a-stored-session")
        );
    }

    /// The memo is keyed by what tells stores apart, so it cannot leak across
    /// them.
    #[test]
    fn a_different_store_knows_nothing() {
        let dir = TempDir::new().unwrap();
        file_store(&dir).set("session", "from-the-first").unwrap();
        let elsewhere = TempDir::new().unwrap();
        assert_eq!(file_store(&elsewhere).get("session").unwrap(), None);
        let other_tool = Secrets::new("net-kit-test-other", Backend::File, dir.path());
        assert_eq!(other_tool.get("session").unwrap(), None);
    }

    #[test]
    fn writing_the_same_value_again_does_not_write() {
        let dir = TempDir::new().unwrap();
        let s = file_store(&dir);
        s.set("session", "a-stored-session").unwrap();
        fs::remove_file(s.path("session")).unwrap();
        s.set("session", "a-stored-session").unwrap();
        assert!(!s.path("session").exists(), "the store was reached again");
    }

    #[test]
    fn writing_a_different_value_does() {
        let dir = TempDir::new().unwrap();
        let s = file_store(&dir);
        s.set("session", "the-first").unwrap();
        s.set("session", "the-second").unwrap();
        assert_eq!(fs::read_to_string(s.path("session")).unwrap(), "the-second");
        assert_eq!(s.get("session").unwrap().as_deref(), Some("the-second"));
    }

    #[test]
    fn what_is_remembered_is_what_a_read_would_give_back() {
        let dir = TempDir::new().unwrap();
        let s = file_store(&dir);
        s.set("session", " spaced ").unwrap();
        assert_eq!(s.get("session").unwrap().as_deref(), Some("spaced"));
    }

    #[test]
    fn deleting_something_known_absent_does_not_reach_the_store() {
        let dir = TempDir::new().unwrap();
        let s = file_store(&dir);
        assert_eq!(s.get("absent").unwrap(), None);
        assert!(!s.delete("absent").unwrap());
    }

    #[test]
    fn a_deleted_value_is_gone_from_memory_too() {
        let dir = TempDir::new().unwrap();
        let s = file_store(&dir);
        s.set("session", "a-stored-session").unwrap();
        assert!(s.delete("session").unwrap());
        assert_eq!(s.get("session").unwrap(), None);
    }

    #[test]
    fn debug_does_not_print_the_secrets() {
        let dir = TempDir::new().unwrap();
        let s = file_store(&dir);
        s.set("session", "a-stored-session").unwrap();
        assert!(!format!("{s:?}").contains("a-stored-session"));
    }

    #[test]
    fn the_fallback_is_named_honestly() {
        assert!(Backend::File
            .describe()
            .contains("no system credential store"));
    }
}
