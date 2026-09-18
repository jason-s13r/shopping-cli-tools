//! Everything a command needs, assembled once.
//!
//! The precedence rule is the same for every setting and is applied here so no
//! command has to remember it: **flag, then environment, then config file,
//! then the library default**. Below this file nothing consults any of the
//! three.

use std::path::PathBuf;

use cli_kit::{Format, Out};
use kmart_api::{Client, Country, Endpoints, StoredSession};
use net_kit::{Backend, Paths, Secrets};

use crate::cli::Cli;
use crate::config::{ColorChoice, Config};
use crate::env::Overrides;
use crate::error::{AppError, AppResult};

/// The name the platform files this tool's config and state under. Its own,
/// not shared with the grocery tools: two tools reading one another's tokens
/// would be a surprise the first time a logout took both down.
pub const APP: &str = "kmart-cli";

/// The storefront asked when nothing says otherwise.
///
/// Australia, as the larger of the two shops. Not much of a judgement -- one
/// `kmart use nz` settles it for good, and `auth login` writes the country it
/// signed in to, so an account only ever meets this once.
pub const DEFAULT_COUNTRY: Country = Country::Au;

pub struct App {
    pub config: Config,
    pub config_file: PathBuf,
    pub paths: Paths,
    pub env: Overrides,
    pub format: Format,
    pub color: bool,
    /// Which Kmart this run talks to. Resolved here so no command re-derives
    /// it, and carried on the client as well -- it picks the catalogue index,
    /// the gateway host and the currency.
    pub country: Country,
    /// The island filter this run actually applies.
    ///
    /// `None` in Australia whatever the config or the flag says, because
    /// there is no such facet there. Resolved here rather than at each use so
    /// that nothing downstream can apply it and nothing can *claim* to have
    /// applied it -- a listing that said "ranged for the NI island" over
    /// Australian prices would be quietly wrong.
    pub island: Option<String>,
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

        let country = match cli.country {
            Some(country) => country,
            None => match env.country.as_deref() {
                Some(text) => Country::parse(text).ok_or_else(|| {
                    AppError::usage(format!(
                        "KMART_COUNTRY is {text:?}, which is not a country; use `au` or `nz`"
                    ))
                })?,
                None => config.country.unwrap_or(DEFAULT_COUNTRY),
            },
        };

        let island = match cli.island() {
            // Parsed even in Australia, so a typo is still reported rather
            // than silently ignored.
            Some(text) => Some(crate::config::parse_island(text)?),
            None => config.island.clone(),
        }
        .filter(|_| country == Country::Nz);

        Ok(App {
            config,
            config_file,
            paths,
            env,
            format,
            color,
            country,
            island,
        })
    }

    /// The credential store the account's secrets are filed in.
    pub fn secrets(&self) -> Secrets {
        Secrets::new(APP, self.backend(), &self.paths.state_dir)
    }

    /// Which store that is.
    ///
    /// Resolved rather than re-detected, so `doctor` reports the one actually
    /// in use: with `KMART_SECRET_BACKEND=file` set, saying "the system
    /// credential store" would send someone looking in the wrong place.
    pub fn backend(&self) -> Backend {
        backend(self.env.secret_backend.as_deref())
    }

    pub fn out(&self) -> Out {
        Out::stdout(self.format, !self.color)
    }

    pub fn endpoints(&self) -> Endpoints {
        let mut endpoints = Endpoints::of(self.country);
        if let Some(api) = &self.env.api {
            endpoints = endpoints.with_api(api.clone());
        }
        if let Some(search) = &self.env.search {
            endpoints = endpoints.with_search(search.clone());
        }
        if let Some(auth) = &self.env.auth {
            endpoints = endpoints.with_auth(auth.clone());
        }
        endpoints
    }

    /// [`App::endpoints`], with whatever the storefront is serving today.
    ///
    /// Two countries are in play at once here, and they are not the same
    /// question. The catalogue key belongs to the country being *asked*; the
    /// Auth0 application belongs to the country that *signed in*, because a
    /// refresh token cannot be renewed under any other. They are usually the
    /// same country, and when they are not this is the difference between a
    /// session that renews and one that is refused.
    pub async fn live_endpoints(
        &self,
        http: &net_kit::wreq::Client,
        auth_country: Option<Country>,
    ) -> Endpoints {
        let plain = self.endpoints();
        let (vendor, _) = crate::vendor::resolve(
            http,
            &self.paths.state_dir,
            self.country,
            &plain.origin,
            |m| {
                if self.env.debug {
                    eprintln!("kmart: {m}");
                }
            },
        )
        .await;
        let mut endpoints = plain.with_vendor(&vendor);
        if let Some(minted_by) = auth_country.filter(|c| *c != self.country) {
            let auth = crate::vendor::cached(&self.paths.state_dir, minted_by);
            endpoints.auth_client_id = auth.auth_client_id;
            endpoints.auth_redirect = auth.auth_redirect;
        }
        endpoints
    }

    /// The Constructor.io visitor id, minted on first use and kept.
    ///
    /// The index personalises on it, so a new one per run is a new visitor per
    /// run -- worse ranking, and a louder pattern than a returning shopper.
    /// Written back into the config the first time, which is the only setting
    /// this program sets without being asked to.
    pub fn visitor_id(&self) -> AppResult<String> {
        if let Some(id) = &self.config.visitor_id {
            return Ok(id.clone());
        }
        let id = new_visitor_id();
        let mut config = self.config.clone();
        config.visitor_id = Some(id.clone());
        // A read-only config directory should not stop a search working, so a
        // failure to remember it is not a failure to run.
        let _ = self.save(&config);
        Ok(id)
    }

    /// The client every command talks through.
    ///
    /// Built per call rather than cached: it is one HTTP client and a small
    /// map, no command needs two, and a lazy cell here would only exist to
    /// hide a cost that is not there.
    pub async fn client(&self) -> AppResult<Client> {
        let http = net_kit::http::build(kmart_api::client_spec())
            .map_err(|e| AppError::usage(format!("building the HTTP client: {e}")))?;
        let secrets = self.secrets();
        let stored = StoredSession::load(&secrets)?;
        let session = stored
            .as_ref()
            .map(StoredSession::session)
            .unwrap_or_default();

        let auth_country = stored
            .as_ref()
            .and_then(kmart_api::StoredSession::auth_country);
        let email = stored.and_then(|s| s.email);

        // Always, and not only when there is a password: Auth0 rotates the
        // refresh token on every use, so a renewal that is not written back
        // works once and leaves the next command replaying a spent token.
        let store = Some(kmart_api::SessionStore {
            secrets: self.secrets(),
            email: email.clone(),
            auth_country,
        });

        // Only offered when there is an email to sign in *as*. The password is
        // named, not read: a command that renews from the refresh token, or
        // never renews at all, should not pay a keychain prompt for one.
        let reauth = email.map(|email| kmart_api::Reauth {
            email,
            password: net_kit::password::Source::named(
                self.config.auth.password_command.as_deref(),
                &secrets,
            ),
        });

        Ok(Client::new(
            http.clone(),
            self.live_endpoints(&http, auth_country).await,
            self.country,
            session,
            self.visitor_id()?,
        )
        .with_session_store(store)
        .with_reauth(reauth))
    }

    /// A postcode to spend a throwaway gateway call on.
    ///
    /// Not for quoting stock -- that has no sensible default and says so
    /// below. This is for the calls whose *answer* is beside the point: what
    /// is being asked is whether the gateway answers at all. The one in the
    /// config where there is one, so the query looks like the rest of this
    /// session's traffic, and a capital city where there is not.
    pub fn probe_postcode(&self) -> &str {
        self.config
            .postcode
            .as_deref()
            .unwrap_or(match self.country {
                Country::Nz => "1010",
                Country::Au => "3000",
            })
    }

    /// The postcode a stock question is asked about.
    ///
    /// There is no sensible default: every availability answer is relative to
    /// one, and guessing a city would quote stock for somewhere the person is
    /// not. So this is an error with a way out rather than a fallback.
    pub fn postcode(&self, flag: Option<&str>) -> AppResult<String> {
        flag.map(str::to_string)
            .or_else(|| self.config.postcode.clone())
            .ok_or_else(|| {
                AppError::usage(
                    "no postcode set; run `kmart postcode set <POSTCODE>` or pass --postcode"
                        .to_string(),
                )
            })
    }

    /// Write the config back, having changed it.
    pub fn save(&self, config: &Config) -> AppResult<()> {
        config.save(&self.config_file)
    }
}

impl Cli {
    /// The `--island` this run was given, wherever it was given.
    ///
    /// Every listing takes one and the client is built before the command
    /// runs, so it has to be found from up here.
    pub fn island(&self) -> Option<&str> {
        use crate::cli::Command;
        match &self.command {
            Command::Search { listing, .. } | Command::Browse { listing, .. } => {
                listing.island.as_deref()
            }
            _ => None,
        }
    }
}

/// A UUID-shaped visitor id.
///
/// Shaped like the one the site's own script writes, because a value that does
/// not look like the others is itself a signal. Not a real v4 -- nothing reads
/// the version nibble -- but it is random where it matters.
fn new_visitor_id() -> String {
    let mut bytes = [0u8; 16];
    // The same source `kmart_api` uses for PKCE; there is no separate
    // randomness crate here for the sake of one id.
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
            >> (i % 4 * 8)) as u8
            ^ (i as u8).wrapping_mul(31);
    }
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-4{}-8{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[13..16],
        &hex[17..20],
        &hex[20..32]
    )
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

    #[test]
    fn a_visitor_id_is_shaped_like_the_one_the_site_writes() {
        let id = new_visitor_id();
        assert_eq!(id.len(), 36, "{id}");
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            [8, 4, 4, 4, 12]
        );
        assert!(
            id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
            "{id}"
        );
    }
}
