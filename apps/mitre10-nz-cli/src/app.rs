//! Everything a command needs, assembled once.
//!
//! The precedence rule is the same for every setting and is applied here so no
//! command has to remember it: **flag, then environment, then config file,
//! then the library default**. Below this file nothing consults any of them.

use std::path::PathBuf;

use cli_kit::{Format, Out};
use mitre10_api::{Client, Endpoints, SessionStore, StoredSession};
use net_kit::wreq_util::Profile;
use net_kit::{Backend, Paths, Secrets};

use crate::cli::Cli;
use crate::config::{ColorChoice, Config};
use crate::env::Overrides;
use crate::error::{AppError, AppResult};

/// The name the platform files this tool's config and state under. Its own,
/// not shared with the other retailers' tools: two tools reading one another's
/// tokens would be a surprise the first time a logout took both down.
pub const APP: &str = "mitre10-nz-cli";

pub struct App {
    pub config: Config,
    pub config_file: PathBuf,
    pub paths: Paths,
    pub env: Overrides,
    pub format: Format,
    pub color: bool,
    /// The browser every request presents as. Named here rather than looked up
    /// per client so an unusable `M10_EMULATION` is refused once, before any
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

        let emulation = match &env.emulation {
            Some(name) => mitre10_api::profile(name).ok_or_else(|| {
                AppError::usage(format!(
                    "{name:?} is not a browser profile; M10_EMULATION takes a wreq-util \
                     name such as `firefox139` or `chrome137`"
                ))
            })?,
            None => mitre10_api::EMULATION,
        };

        Ok(App {
            config,
            config_file,
            paths,
            env,
            format,
            color,
            emulation,
        })
    }

    pub fn secrets(&self) -> Secrets {
        Secrets::new(
            APP,
            backend(self.env.secret_backend.as_deref()),
            &self.paths.state_dir,
        )
    }

    /// The HTTP client, presenting as this run's browser profile.
    pub fn http(&self) -> AppResult<net_kit::wreq::Client> {
        net_kit::http::build(mitre10_api::client_spec_for(self.emulation))
            .map_err(|e| AppError::usage(format!("building the HTTP client: {e}")))
    }

    pub fn out(&self) -> Out {
        Out::stdout(self.format, !self.color)
    }

    pub fn endpoints(&self) -> Endpoints {
        let mut endpoints = Endpoints::defaults();
        if let Some(origin) = &self.env.api_origin {
            endpoints = endpoints.with_api(origin.clone());
        }
        if let Some(origin) = &self.env.site_origin {
            endpoints = endpoints.with_site(origin.clone());
        }
        if let Some(origin) = &self.env.search_origin {
            endpoints = endpoints.with_search(origin.clone());
        }
        endpoints
    }

    /// The client every command talks through.
    ///
    /// Built per call rather than cached: it is one HTTP client and a session,
    /// no command needs two, and a lazy cell here would only exist to hide a
    /// cost that is not there.
    pub fn client(&self) -> AppResult<Client> {
        let secrets = self.secrets();
        let session = StoredSession::load(&secrets)?
            .map(|s| s.session())
            .unwrap_or_default();
        Ok(Client::new(self.http()?, self.endpoints(), session)
            // Worth setting even with nothing to sign in with: renewal works
            // from the stored refresh token alone, and it is the *saving* that
            // keeps it working for the next command.
            .with_session_store(Some(SessionStore { secrets }))
            .with_debug(self.env.debug))
    }

    /// The store a command uses when none was named: the flag, then config.
    pub fn store(&self, flag: Option<String>) -> Option<String> {
        flag.or_else(|| self.config.store.clone())
    }

    /// The postcode a delivery listing is priced for: the flag, then config.
    pub fn postcode(&self, flag: Option<String>) -> Option<String> {
        flag.or_else(|| self.config.postcode.clone())
    }

    /// Write the config back, having changed it.
    pub fn save(&self, config: &Config) -> AppResult<()> {
        config.save(&self.config_file)
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
