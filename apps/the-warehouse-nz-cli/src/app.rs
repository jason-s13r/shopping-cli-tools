//! Everything a command needs, assembled once.
//!
//! The precedence rule is the same for every setting and is applied here so no
//! command has to remember it: **flag, then environment, then config file, then
//! the library default**. Below this file nothing consults any of the three.

use std::path::PathBuf;

use cli_kit::{Format, Out};
use net_kit::wreq_util::Profile;
use net_kit::{Backend, Paths, Secrets};
use twlnz_api::{Client, Endpoints, Island, StoredSession};

use crate::cli::Cli;
use crate::config::{ColorChoice, Config};
use crate::env::Overrides;
use crate::error::{AppError, AppResult};

/// The name the platform files this tool's config and state under. Its own, not
/// shared with the grocery tools: two tools reading one another's tokens would
/// be a surprise the first time a logout took both down.
pub const APP: &str = "the-warehouse-nz-cli";

pub struct App {
    pub config: Config,
    pub config_file: PathBuf,
    pub paths: Paths,
    pub env: Overrides,
    pub format: Format,
    pub color: bool,
    /// The island this run uses: the flag if one was given, otherwise the
    /// config. Resolved here so no command re-derives it.
    pub island: Option<Island>,
    /// The browser every request presents as. Named here rather than looked up
    /// per client so an unusable `TWLNZ_EMULATION` is refused once, before any
    /// command has done anything.
    pub emulation: Profile,
}

impl App {
    pub fn new(cli: &Cli) -> AppResult<App> {
        let env = Overrides::get().clone();
        let mut paths = Paths::defaults(APP)?;
        if let Some(dir) = &env.config_dir {
            paths = paths.with_config_dir(dir.clone());
        }
        if let Some(dir) = &env.state_dir {
            paths = paths.with_state_dir(dir.clone());
        }
        let config_file = paths.config_file();
        let config = Config::load(&config_file)?;

        let format = if cli.json { Format::Json } else { Format::Text };
        let color = match config.output.color {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            // `NO_COLOR` is honoured whatever the config says: it is set by a
            // person's environment, which is more specific than a file they
            // wrote once.
            ColorChoice::Auto => !env.no_color,
        };

        let island = match cli.island() {
            Some(text) => Some(Island::parse(text).ok_or_else(|| {
                AppError::usage(format!("{text:?} is not an island; use `north` or `south`"))
            })?),
            None => config.island,
        };

        let emulation = match &env.emulation {
            Some(name) => twlnz_api::profile(name).ok_or_else(|| {
                AppError::usage(format!(
                    "{name:?} is not a browser profile; TWLNZ_EMULATION takes a wreq-util \
                     name such as `safari26_4` or `firefox151`"
                ))
            })?,
            None => twlnz_api::EMULATION,
        };

        Ok(App {
            config,
            config_file,
            paths,
            env,
            format,
            color,
            island,
            emulation,
        })
    }

    /// The credential store the account's secrets are filed in.
    pub fn secrets(&self) -> Secrets {
        Secrets::new(
            APP,
            backend(self.env.secret_backend.as_deref()),
            &self.paths.state_dir,
        )
    }

    /// The HTTP client, presenting as this run's browser profile.
    ///
    /// Every request in the program goes through one of these, so the profile
    /// is applied here rather than at each call site -- there is no useful
    /// state in which one command is a Safari and the next is not.
    pub fn http(&self) -> AppResult<net_kit::wreq::Client> {
        net_kit::http::build(twlnz_api::client_spec_for(self.emulation))
            .map_err(|e| AppError::usage(format!("building the HTTP client: {e}")))
    }

    pub fn out(&self) -> Out {
        Out::stdout(self.format, !self.color)
    }

    pub fn endpoints(&self) -> Endpoints {
        let endpoints = Endpoints::default();
        match &self.env.origin {
            Some(origin) => endpoints.with_origin(origin.clone()),
            None => endpoints,
        }
    }

    /// The client every command talks through.
    ///
    /// Built per call rather than cached: it is one HTTP client and a cookie
    /// map, no command needs two, and a lazy cell here would only exist to hide
    /// a cost that is not there.
    pub fn client(&self) -> AppResult<Client> {
        let http = self.http()?;
        let secrets = self.secrets();
        let stored = StoredSession::load(&secrets)?;
        let session = stored
            .as_ref()
            .map(StoredSession::session)
            .unwrap_or_default();

        // Only offered when there is an email to sign in *as*. The password is
        // named, not read: a command that never renews should not pay a
        // keychain prompt for one.
        let reauth = stored.and_then(|s| s.email).map(|email| twlnz_api::Reauth {
            email,
            password: net_kit::password::Source::named(
                self.config.auth.password_command.as_deref(),
                &secrets,
            ),
            secrets: self.secrets(),
        });

        Ok(Client::new(http, self.endpoints(), session)
            .with_reauth(reauth)
            .with_island(self.island)
            .with_debug(self.env.debug))
    }

    /// Write the config back, having changed it.
    pub fn save(&self, config: &Config) -> AppResult<()> {
        config.save(&self.config_file)
    }
}

impl Cli {
    /// The `--island` this run was given, wherever it was given.
    ///
    /// Every listing takes one and the client is built before the command runs,
    /// so it has to be found from up here.
    pub fn island(&self) -> Option<&str> {
        use crate::cli::Command;
        match &self.command {
            Command::Search { listing, .. }
            | Command::Browse { listing, .. }
            | Command::Specials { listing } => listing.island.as_deref(),
            _ => None,
        }
    }
}

fn backend(override_: Option<&str>) -> Backend {
    match override_.map(str::to_lowercase).as_deref() {
        Some("file") => Backend::File,
        Some("keyring") => Backend::Keyring,
        // Anything else, including nothing, means "whatever this machine has".
        _ => Backend::detect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_backend_name_falls_back_rather_than_failing() {
        assert_eq!(backend(Some("file")), Backend::File);
        assert_eq!(backend(Some("keyring")), Backend::Keyring);
        assert_eq!(backend(Some("nonsense")), Backend::detect());
    }
}
