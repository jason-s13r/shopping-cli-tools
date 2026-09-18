//! Everything a command needs, assembled once.
//!
//! The precedence rule is the same for every setting and is applied here so no
//! command has to remember it: **flag, then environment, then config file,
//! then the library default**. Below this file nothing consults any of them.

use std::path::PathBuf;

use cli_kit::{Format, Out};
use farmers_api::{Client, Endpoints, Reauth, SessionStore, StoredSession, Warmer};
use net_kit::wreq_util::Profile;
use net_kit::{Backend, Paths, Secrets};

use crate::browser;
use crate::cli::Cli;
use crate::config::{ColorChoice, Config};
use crate::env::Overrides;
use crate::error::{AppError, AppResult};

/// The name the platform files this tool's config and state under. Its own,
/// not shared with the other retailers' tools: two tools reading one another's
/// sessions would be a surprise the first time a logout took both down.
pub const APP: &str = "farmers-nz-cli";

pub struct App {
    pub config: Config,
    pub config_file: PathBuf,
    pub paths: Paths,
    pub env: Overrides,
    pub format: Format,
    pub color: bool,
    /// The browser every request presents as. Named here rather than looked up
    /// per client so an unusable `FMNZ_EMULATION` is refused once, before any
    /// command has done anything.
    pub emulation: Profile,
    /// Whether the warm-up browser should show its window.
    pub headful: bool,
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
            Some(name) => farmers_api::profile(name).ok_or_else(|| {
                AppError::usage(format!(
                    "{name:?} is not a browser profile; FMNZ_EMULATION takes a wreq-util \
                     name such as `firefox139` or `chrome137`"
                ))
            })?,
            None => farmers_api::EMULATION,
        };

        let headful = cli.headful || env.headful;
        Ok(App {
            config,
            config_file,
            paths,
            env,
            format,
            color,
            emulation,
            headful,
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
        net_kit::http::build(farmers_api::client_spec_for(self.emulation))
            .map_err(|e| AppError::usage(format!("building the HTTP client: {e}")))
    }

    pub fn out(&self) -> Out {
        Out::stdout(self.format, !self.color)
    }

    pub fn endpoints(&self) -> Endpoints {
        let mut endpoints = Endpoints::defaults();
        if let Some(origin) = &self.env.site_origin {
            endpoints = endpoints.with_origin(origin.clone());
        }
        if let Some(origin) = &self.env.search_origin {
            endpoints = endpoints.with_search(origin.clone());
        }
        if let Some(key) = &self.env.search_key {
            endpoints = endpoints.with_key(key.clone());
        }
        endpoints
    }

    /// The client every command talks through.
    ///
    /// The stored session is loaded whether or not anyone has signed in: most
    /// of what it holds is the Akamai warm-up, and carrying that over is what
    /// keeps a second command from paying for a first one's handshake.
    pub fn client(&self) -> AppResult<Client> {
        let secrets = self.secrets();
        let session = StoredSession::load(&secrets)?
            .map(|s| s.session())
            .unwrap_or_default();
        let reauth = self.reauth(session.email.clone());
        Ok(Client::new(self.http()?, self.endpoints(), session)
            .with_session_store(Some(SessionStore { secrets }))
            .with_reauth(reauth)
            .with_warmer(Some(self.warmer()))
            .with_debug(self.env.debug))
    }

    /// How this tool buys admission to the storefront: a browser, because
    /// nothing else is accepted any more. See [`crate::browser`].
    ///
    /// Attached to every client rather than only where a browser is installed.
    /// A machine without one should be told that when a gated call needs it --
    /// naming the thing to install -- and not left to meet `Access Denied`
    /// instead, which says nothing about what would fix it.
    pub fn warmer(&self) -> Warmer {
        let python = self.env.browser_python.clone();
        let state_dir = self.paths.state_dir.clone();
        let origin = self.endpoints().origin;
        let headless = !self.headful;
        let debug = self.env.debug;
        std::sync::Arc::new(move || {
            let (python, state_dir, origin) = (python.clone(), state_dir.clone(), origin.clone());
            Box::pin(async move {
                let warmth = browser::warm(python.as_deref(), &state_dir, &origin, headless, debug)
                    .await
                    // The hook hands back a `farmers_api::Error`, which has no
                    // room for "there is no browser installed". Shape is the
                    // nearest thing and carries the sentence intact, which is
                    // what matters -- it is the only one that says what to do.
                    .map_err(|e| farmers_api::Error::Shape(e.to_string()))?;
                Ok(warmth.cookies)
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = _> + Send>>
        })
    }

    /// Who this client could sign itself in again as, if anyone. The password
    /// is left where it lies until something spends it.
    pub fn reauth(&self, email: Option<String>) -> Option<Reauth> {
        let secrets = self.secrets();
        Some(Reauth {
            email: email?,
            password: net_kit::password::Source::named(
                self.config.auth.password_command.as_deref(),
                &secrets,
            ),
            secrets,
        })
    }

    /// The region a command uses when none was named: the flag, then config.
    pub fn region(&self, flag: Option<String>) -> Option<String> {
        flag.or_else(|| self.config.region.clone())
    }

    /// How many products a listing shows: the flag, then config, then the
    /// library's own page size.
    pub fn page_size(&self, flag: Option<u64>) -> Option<u64> {
        flag.or(self.config.page_size)
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
