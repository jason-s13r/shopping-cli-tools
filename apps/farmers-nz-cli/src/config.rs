//! The config file: what a bare command does when no flag says otherwise.
//!
//! The credentials are *not* here -- those live in the credential store.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// The region a bare `farmers stock` reports first, by code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// How many products a listing shows when `--limit` is not given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_size: Option<u64>,
    #[serde(skip_serializing_if = "Output::is_default")]
    pub output: Output,
    #[serde(skip_serializing_if = "Auth::is_default")]
    pub auth: Auth,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Auth {
    /// A shell command that prints the password, for a password manager.
    /// Beats the stored copy, and leaves the manager as the only place it
    /// lives.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password_command: Option<String>,
    /// Whether `auth login` keeps the password.
    ///
    /// On by default, because this session *can* be renewed -- the sign-in is
    /// an ordinary form POST that can simply be re-run -- but re-running it
    /// still takes a password, and there is no token to do it with.
    pub store_password: bool,
}

impl Default for Auth {
    fn default() -> Auth {
        Auth {
            password_command: None,
            store_password: true,
        }
    }
}

impl Auth {
    fn is_default(&self) -> bool {
        *self == Auth::default()
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorChoice {
    #[default]
    Auto,
    Always,
    Never,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Output {
    pub color: ColorChoice,
}

impl Output {
    fn is_default(&self) -> bool {
        *self == Output::default()
    }
}

/// The settings `config set` accepts, and what each is for.
pub const KEYS: [&str; 5] = [
    "region",
    "page_size",
    "output.color",
    "auth.password_command",
    "auth.store_password",
];

pub fn describe(key: &str) -> &'static str {
    match key {
        "region" => "the region `farmers stock` reports first, by code",
        "page_size" => "how many products a listing shows without --limit",
        "output.color" => "auto, always or never",
        "auth.password_command" => "a command that prints the password, for a password manager",
        "auth.store_password" => {
            "keep the password at login, so a lapsed session can sign itself in again"
        }
        _ => "",
    }
}

/// `true`/`false`, however a person writes it in a config file.
fn boolean(value: &str) -> AppResult<bool> {
    match value.trim().to_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" => Ok(false),
        _ => Err(AppError::usage(format!("{value:?} is not true or false"))),
    }
}

/// The most a listing will ask the search index for in one page.
///
/// Not a limit the service enforces -- it is this tool being frugal with a host
/// whose bot protection escalates on volume, and a thousand-row table is not
/// something anyone reads anyway.
const MAX_PAGE_SIZE: u64 = 100;

impl Config {
    pub fn load(file: &Path) -> AppResult<Config> {
        Ok(net_kit::config::load_toml(file)?)
    }

    pub fn save(&self, file: &Path) -> AppResult<()> {
        Ok(net_kit::config::save_toml(file, self)?)
    }

    pub fn get(&self, key: &str) -> AppResult<Option<String>> {
        Ok(match key {
            "region" => self.region.clone(),
            "page_size" => self.page_size.map(|n| n.to_string()),
            "output.color" => Some(
                match self.output.color {
                    ColorChoice::Auto => "auto",
                    ColorChoice::Always => "always",
                    ColorChoice::Never => "never",
                }
                .to_string(),
            ),
            "auth.password_command" => self.auth.password_command.clone(),
            "auth.store_password" => Some(self.auth.store_password.to_string()),
            _ => return Err(unknown(key)),
        })
    }

    pub fn set(&mut self, key: &str, value: &str) -> AppResult<()> {
        match key {
            "region" => {
                // Stored as the code the endpoint takes, whichever of the two
                // was typed -- so a config file written from a name does not
                // fail later as "no such region".
                let code = farmers_api::region(value).ok_or_else(|| {
                    AppError::usage(format!(
                        "{value:?} is not a Farmers region; run `farmers regions` for the {} there are",
                        farmers_api::REGIONS.len()
                    ))
                })?;
                self.region = Some(code.to_string());
            }
            "page_size" => {
                let size: u64 = value.trim().parse().map_err(|_| {
                    AppError::usage(format!("{value:?} is not a number of products"))
                })?;
                if size == 0 || size > MAX_PAGE_SIZE {
                    return Err(AppError::usage(format!(
                        "a page holds between 1 and {MAX_PAGE_SIZE} products"
                    )));
                }
                self.page_size = Some(size);
            }
            "output.color" => {
                self.output.color = match value.trim().to_lowercase().as_str() {
                    "auto" => ColorChoice::Auto,
                    "always" => ColorChoice::Always,
                    "never" => ColorChoice::Never,
                    _ => {
                        return Err(AppError::usage(format!(
                            "{value:?} is not a colour setting; use auto, always or never"
                        )))
                    }
                }
            }
            "auth.password_command" => {
                let command = value.trim();
                if command.is_empty() {
                    return Err(AppError::usage(
                        "a password command cannot be blank; `config unset` removes it",
                    ));
                }
                self.auth.password_command = Some(command.to_string());
            }
            "auth.store_password" => self.auth.store_password = boolean(value)?,
            _ => return Err(unknown(key)),
        }
        Ok(())
    }

    pub fn unset(&mut self, key: &str) -> AppResult<()> {
        match key {
            "region" => self.region = None,
            "page_size" => self.page_size = None,
            "output.color" => self.output.color = ColorChoice::Auto,
            // Removes the command only. The stored password is a credential,
            // not a setting, and `auth logout` is what forgets it.
            "auth.password_command" => self.auth.password_command = None,
            "auth.store_password" => self.auth.store_password = Auth::default().store_password,
            _ => return Err(unknown(key)),
        }
        Ok(())
    }
}

fn unknown(key: &str) -> AppError {
    AppError::usage(format!(
        "there is no setting called {key:?}; try one of {}",
        KEYS.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_region_is_stored_as_the_code_whichever_way_it_was_typed() {
        // The endpoint takes `AUK`. Filing "Auckland" verbatim would fail much
        // later, as "no such region", with nothing pointing back to here.
        let mut c = Config::default();
        c.set("region", "Auckland").expect("a region");
        assert_eq!(c.region.as_deref(), Some("AUK"));
        c.set("region", "hawkes bay").expect("no macron needed");
        assert_eq!(c.region.as_deref(), Some("HKB"));
    }

    #[test]
    fn a_place_with_no_farmers_store_is_refused_with_a_way_to_find_out() {
        let mut c = Config::default();
        let e = c.set("region", "Tasman").expect_err("no store there");
        assert!(e.to_string().contains("farmers regions"), "{e}");
    }

    #[test]
    fn a_page_size_is_bounded_at_both_ends() {
        // Zero would ask for an empty page forever; the ceiling is this tool
        // being frugal with a host that escalates on volume.
        let mut c = Config::default();
        c.set("page_size", "48").expect("a size");
        assert_eq!(c.page_size, Some(48));
        assert!(c.set("page_size", "0").is_err());
        assert!(c.set("page_size", "1000").is_err());
        assert!(c.set("page_size", "lots").is_err());
    }

    #[test]
    fn an_unknown_setting_says_what_the_settings_are() {
        let mut c = Config::default();
        let e = c.set("colour", "always").expect_err("no such setting");
        assert!(e.to_string().contains("output.color"), "{e}");
    }

    #[test]
    fn a_saved_config_round_trips() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let file = dir.path().join("config.toml");
        let mut c = Config::default();
        c.set("region", "Canterbury").expect("sets");
        c.set("page_size", "12").expect("sets");
        c.save(&file).expect("saves");

        let text = std::fs::read_to_string(&file).expect("reads");
        assert!(text.contains("region = \"CAN\""), "{text}");
        let back = Config::load(&file).expect("loads");
        assert_eq!(back.region.as_deref(), Some("CAN"));
        assert_eq!(back.page_size, Some(12));
    }

    #[test]
    fn keeping_the_password_is_the_default_because_there_is_no_token_to_renew_with() {
        // Turning it off means every lapsed session needs a person, which is
        // the thing unattended use is trying to avoid.
        assert!(Config::default().auth.store_password);
        assert_eq!(Config::default().auth.password_command, None);
    }

    #[test]
    fn a_boolean_setting_takes_the_words_people_actually_write() {
        let mut c = Config::default();
        for yes in ["true", "yes", "on", "1", "TRUE"] {
            c.set("auth.store_password", yes).expect(yes);
            assert!(c.auth.store_password, "{yes}");
        }
        for no in ["false", "no", "off", "0"] {
            c.set("auth.store_password", no).expect(no);
            assert!(!c.auth.store_password, "{no}");
        }
        assert!(c.set("auth.store_password", "maybe").is_err());
    }

    #[test]
    fn a_blank_password_command_is_refused_rather_than_stored_as_one() {
        // An empty command would be run and would fail, at the exact moment
        // nobody is watching.
        let mut c = Config::default();
        let e = c.set("auth.password_command", "   ").expect_err("blank");
        assert!(e.to_string().contains("config unset"), "{e}");
    }

    #[test]
    fn unsetting_the_command_does_not_pretend_to_forget_the_password() {
        // The stored password is a credential rather than a setting; only
        // `auth logout` removes it, and implying otherwise here would leave a
        // password behind that someone believed was gone.
        let mut c = Config::default();
        c.set("auth.password_command", "pass show farmers")
            .expect("sets");
        c.unset("auth.password_command").expect("unsets");
        assert_eq!(c.auth.password_command, None);
        assert!(c.auth.store_password, "untouched");
    }

    #[test]
    fn a_missing_config_is_the_default_rather_than_a_failure() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let c = Config::load(&dir.path().join("nothing.toml")).expect("a first run has no config");
        assert_eq!(c.region, None);
    }
}
