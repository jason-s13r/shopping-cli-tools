//! The config file: what a bare command does when no flag says otherwise.
//!
//! The credentials are *not* here -- those live in the credential store.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// The store a bare `mitre10 stock` reports first and `mitre10 cart add` collects
    /// from, by numeric store code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<String>,
    /// The postcode a delivery-filtered listing is priced for. Resolved to a
    /// delivery group at run time, because the group is what the index takes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub postcode: Option<String>,
    #[serde(skip_serializing_if = "Output::is_default")]
    pub output: Output,
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
pub const KEYS: [&str; 3] = ["store", "postcode", "output.color"];

pub fn describe(key: &str) -> &'static str {
    match key {
        "store" => "the store commands use when --store is not given, by store code",
        "postcode" => "the postcode a delivery listing is priced for",
        "output.color" => "auto, always or never",
        _ => "",
    }
}

impl Config {
    pub fn load(file: &Path) -> AppResult<Config> {
        Ok(net_kit::config::load_toml(file)?)
    }

    pub fn save(&self, file: &Path) -> AppResult<()> {
        Ok(net_kit::config::save_toml(file, self)?)
    }

    pub fn get(&self, key: &str) -> AppResult<Option<String>> {
        Ok(match key {
            "store" => self.store.clone(),
            "postcode" => self.postcode.clone(),
            "output.color" => Some(
                match self.output.color {
                    ColorChoice::Auto => "auto",
                    ColorChoice::Always => "always",
                    ColorChoice::Never => "never",
                }
                .to_string(),
            ),
            _ => return Err(unknown(key)),
        })
    }

    pub fn set(&mut self, key: &str, value: &str) -> AppResult<()> {
        match key {
            "store" => {
                let code = value.trim();
                // The numeric code, not the SAP one: every call takes `66`
                // and none of them takes `X57`, which fails as "no such store"
                // rather than as a wrong spelling.
                if code.is_empty() || !code.chars().all(|c| c.is_ascii_digit()) {
                    return Err(AppError::usage(format!(
                        "{value:?} is not a store code; they are numeric, and `mitre10 stores` lists them"
                    )));
                }
                self.store = Some(code.to_string());
            }
            "postcode" => {
                let postcode = value.trim();
                if postcode.len() != 4 || !postcode.chars().all(|c| c.is_ascii_digit()) {
                    return Err(AppError::usage(format!(
                        "{value:?} is not a New Zealand postcode; they are four digits"
                    )));
                }
                self.postcode = Some(postcode.to_string());
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
            _ => return Err(unknown(key)),
        }
        Ok(())
    }

    pub fn unset(&mut self, key: &str) -> AppResult<()> {
        match key {
            "store" => self.store = None,
            "postcode" => self.postcode = None,
            "output.color" => self.output.color = ColorChoice::Auto,
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
    fn a_store_must_be_the_numeric_code_the_api_takes() {
        // `X57` is the SAP code and appears on the site, but no endpoint here
        // accepts it -- it fails later as "no such store".
        let mut c = Config::default();
        c.set("store", "66").expect("a store code");
        assert_eq!(c.store.as_deref(), Some("66"));

        let e = c.set("store", "X57").expect_err("not the numeric code");
        assert!(e.to_string().contains("numeric"), "{e}");
        assert!(c.set("store", "Whangarei").is_err());
    }

    #[test]
    fn a_postcode_is_four_digits() {
        let mut c = Config::default();
        c.set("postcode", "0110").expect("a postcode");
        assert_eq!(c.postcode.as_deref(), Some("0110"));
        assert!(c.set("postcode", "011").is_err());
        assert!(c.set("postcode", "Kensington").is_err());
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
        c.set("store", "66").expect("sets");
        c.set("postcode", "0110").expect("sets");
        c.save(&file).expect("saves");

        let text = std::fs::read_to_string(&file).expect("reads");
        assert!(text.contains("store = \"66\""), "{text}");
        let back = Config::load(&file).expect("loads");
        assert_eq!(back.store.as_deref(), Some("66"));
        assert_eq!(back.postcode.as_deref(), Some("0110"));
    }

    #[test]
    fn a_missing_config_is_the_default_rather_than_a_failure() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let c = Config::load(&dir.path().join("nothing.toml")).expect("a first run has no config");
        assert_eq!(c.store, None);
    }
}
