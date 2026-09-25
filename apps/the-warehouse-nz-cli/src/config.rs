//! `~/.config/the-warehouse-nz-cli/config.toml` -- the settings worth keeping.
//!
//! Everything here is optional and everything has a flag or an environment
//! variable that beats it. The file exists so that `twlnz island set` is
//! remembered, not as a second way to configure the program.

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::error::{AppError, AppResult};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Which island stock is quoted for.
    ///
    /// Not a display preference: The Warehouse ranges differently north and
    /// south, so this changes what a listing *contains*. Kept here because it
    /// belongs to a person rather than to a query.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub island: Option<twlnz_api::Island>,
    /// The store to care about.
    ///
    /// A local preference, not a binding: `store set` does not tell the site,
    /// because the controller that would needs a basket to bind a collection
    /// point to. What it does do is fix [`Config::region`] alongside, which is
    /// what `stock` and `stores` then default to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store_id: Option<String>,
    /// One of the sixteen ISO regions the store finder is queried by, e.g.
    /// `NZ-CAN`. A different idea from [`Config::island`], which is a listing
    /// filter -- the two are only both called "region" by the site.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    // Only written when changed. A file listing every default is one nobody can
    // skim, and this is still a file people edit by hand.
    #[serde(skip_serializing_if = "is_default")]
    pub auth: Auth,
    #[serde(skip_serializing_if = "is_default")]
    pub output: Output,
    #[serde(skip_serializing_if = "is_default")]
    pub network: Network,
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Auth {
    /// A shell command that prints the password, for a password manager. Beats
    /// the stored one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password_command: Option<String>,
    /// Whether `auth login` keeps the password.
    ///
    /// Worth keeping here: unlike Woolworths, this session *can* be renewed --
    /// the form login is an ordinary POST that can simply be re-run -- but
    /// re-running it still takes a password.
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Output {
    pub color: ColorChoice,
}

impl Default for Output {
    fn default() -> Output {
        Output {
            color: ColorChoice::Auto,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Network {
    /// Seconds between requests; `0` turns the spacing off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_interval: Option<f64>,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ColorChoice {
    #[default]
    Auto,
    Always,
    Never,
}

impl Config {
    pub fn load(file: &Path) -> AppResult<Config> {
        Ok(net_kit::config::load_toml(file)?)
    }

    pub fn save(&self, file: &Path) -> AppResult<()> {
        Ok(net_kit::config::save_toml(file, self)?)
    }
}

/// Every setting, as a dotted key.
///
/// The list is explicit rather than derived so that `config list` has an order
/// worth reading, and so a key that is no longer real fails loudly instead of
/// writing a field nothing reads.
pub const KEYS: [&str; 7] = [
    "island",
    "store_id",
    "region",
    "auth.password_command",
    "auth.store_password",
    "output.color",
    "network.request_interval",
];

/// What a key means, for `config list`.
pub fn describe(key: &str) -> &'static str {
    match key {
        "island" => "north or south; changes which products a listing contains",
        "store_id" => "the store to care about; `store set` fixes `region` alongside it",
        "region" => "which of the sixteen regions `stores` and `stock` default to, e.g. NZ-AUK",
        "auth.password_command" => "a command that prints the password, for a password manager",
        "auth.store_password" => {
            "keep the password at login, so a lapsed session can sign itself in again"
        }
        "network.request_interval" => {
            "seconds between requests to the site, 0 for none; retries on a 429 back off from it"
        }
        _ => "auto, always or never",
    }
}

impl Config {
    /// The value as it would be written, or `None` when nothing is set.
    pub fn get(&self, key: &str) -> AppResult<Option<String>> {
        Ok(match key {
            "island" => self.island.map(|i| i.to_string()),
            "store_id" => self.store_id.clone(),
            "region" => self.region.clone(),
            "auth.password_command" => self.auth.password_command.clone(),
            "auth.store_password" => Some(self.auth.store_password.to_string()),
            "output.color" => Some(
                match self.output.color {
                    ColorChoice::Auto => "auto",
                    ColorChoice::Always => "always",
                    ColorChoice::Never => "never",
                }
                .into(),
            ),
            "network.request_interval" => self.network.request_interval.map(|s| s.to_string()),
            _ => return Err(unknown(key)),
        })
    }

    /// Parse and store a value, so a bad one is refused now rather than at the
    /// next command that reads it.
    pub fn set(&mut self, key: &str, value: &str) -> AppResult<()> {
        let value = value.trim();
        match key {
            "island" => {
                self.island = Some(
                    twlnz_api::Island::parse(value)
                        .ok_or_else(|| AppError::usage("island takes `north` or `south`"))?,
                )
            }
            "store_id" => self.store_id = Some(value.to_string()),
            "region" => {
                // Resolved rather than stored as typed, so `stores` never has to
                // guess what `canterbury` meant.
                self.region = Some(
                    twlnz_api::region(value)
                        .ok_or_else(|| {
                            AppError::usage(format!(
                                "{value:?} is not a region. Run `twlnz stores --regions` for the list."
                            ))
                        })?
                        .to_string(),
                )
            }
            "auth.password_command" => self.auth.password_command = Some(value.to_string()),
            "auth.store_password" => self.auth.store_password = boolean(value)?,
            "output.color" => {
                self.output.color = match value.to_lowercase().as_str() {
                    "auto" => ColorChoice::Auto,
                    "always" | "yes" | "on" => ColorChoice::Always,
                    "never" | "no" | "off" => ColorChoice::Never,
                    _ => return Err(AppError::usage("color takes `auto`, `always` or `never`")),
                }
            }
            "network.request_interval" => self.network.request_interval = Some(seconds(value)?),
            _ => return Err(unknown(key)),
        }
        Ok(())
    }

    /// Back to the default. Not the same as setting an empty string: an empty
    /// `password_command` would be run and would fail.
    pub fn unset(&mut self, key: &str) -> AppResult<()> {
        match key {
            "island" => self.island = None,
            "store_id" => self.store_id = None,
            "region" => self.region = None,
            "auth.password_command" => self.auth.password_command = None,
            "auth.store_password" => self.auth.store_password = Auth::default().store_password,
            "output.color" => self.output.color = ColorChoice::default(),
            "network.request_interval" => self.network.request_interval = None,
            _ => return Err(unknown(key)),
        }
        Ok(())
    }
}

fn boolean(value: &str) -> AppResult<bool> {
    match value.to_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" => Ok(false),
        _ => Err(AppError::usage(format!("{value:?} is not true or false"))),
    }
}

/// A request interval, in seconds. Shared with `TWLNZ_REQUEST_INTERVAL` so the
/// file and the variable refuse the same things.
pub fn seconds(value: &str) -> AppResult<f64> {
    match value.trim().parse::<f64>() {
        Ok(s) if s.is_finite() && s >= 0.0 => Ok(s),
        _ => Err(AppError::usage(format!(
            "{value:?} is not a number of seconds; use e.g. `1`, `0.5`, or `0` for no spacing"
        ))),
    }
}

fn unknown(key: &str) -> AppError {
    AppError::usage(format!(
        "no setting called {key:?}. Run `twlnz config list` for the {} there are.",
        KEYS.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_is_a_default_config() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.island, None);
        assert!(cfg.auth.store_password);
    }

    #[test]
    fn a_saved_config_round_trips() {
        let cfg = Config {
            island: Some(twlnz_api::Island::South),
            store_id: Some("116".into()),
            ..Config::default()
        };
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.island, Some(twlnz_api::Island::South));
        assert_eq!(back.store_id.as_deref(), Some("116"));
    }

    #[test]
    fn a_typo_in_a_key_is_reported_rather_than_ignored() {
        // `deny_unknown_fields` is the point: a config that silently does
        // nothing is worse than one that refuses to load.
        let err = toml::from_str::<Config>("islnad = \"north\"").unwrap_err();
        assert!(err.to_string().contains("islnad"), "{err}");
    }

    #[test]
    fn a_region_is_resolved_when_it_is_written_not_when_it_is_read() {
        // Storing `canterbury` verbatim would leave every later command to
        // guess what it meant.
        let mut config = Config::default();
        config.set("region", "Canterbury").unwrap();
        assert_eq!(config.get("region").unwrap().as_deref(), Some("NZ-CAN"));
        assert!(config.set("region", "Chatham Islands").is_err());
    }

    #[test]
    fn a_bad_value_is_refused_at_the_point_of_writing_it() {
        let mut config = Config::default();
        assert!(config.set("island", "east").is_err());
        assert!(config.set("output.color", "purple").is_err());
        assert!(config.set("network.request_interval", "-1").is_err());
        assert!(config.set("network.request_interval", "soon").is_err());
        assert!(config.set("network.request_interval", "0.5").is_ok());
        assert!(config.set("island", "south").is_ok());
    }

    #[test]
    fn unset_restores_the_default_rather_than_emptying_it() {
        let mut config = Config::default();
        config.set("auth.store_password", "false").unwrap();
        config.unset("auth.store_password").unwrap();
        assert_eq!(
            config.get("auth.store_password").unwrap().as_deref(),
            Some("true")
        );
    }

    #[test]
    fn every_listed_key_can_be_read_and_described() {
        // The list is written by hand, so this is what stops it drifting from
        // the match arms beside it.
        let config = Config::default();
        for key in KEYS {
            config.get(key).unwrap_or_else(|e| panic!("{key}: {e}"));
            assert!(!describe(key).is_empty(), "{key}");
        }
    }
}
