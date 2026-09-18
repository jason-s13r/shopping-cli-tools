//! Everything a command needs, assembled once.
//!
//! The precedence rule is the same for every setting and is applied here so no
//! command has to remember it: **flag, then environment, then config file, then
//! the library default**. Below this file nothing consults any of the three.

use std::path::PathBuf;
use std::sync::Arc;

use cli_kit::{Format, Out};
use gsnz_core::{Result, RetailerId};
use net_kit::{Backend, Paths, Secrets};

use crate::cli::Cli;
use crate::config::{ColorChoice, Config};
use crate::env::Overrides;
use crate::error::{AppError, AppResult};
use crate::retailers::{foodstuffs, woolworths, Handle, Registry};

/// The name the platform files this tool's config and state under. Its own,
/// not shared with `fsnz` or `wwnz`: three tools reading one another's tokens
/// would be a surprise the first time a logout took two of them down.
pub const APP: &str = "grocery-nz-cli";

pub struct App {
    pub config: Config,
    pub config_file: PathBuf,
    pub paths: Paths,
    pub env: Overrides,
    pub registry: Registry,
    /// What `-b` named. A list, because `compare` spans several; every other
    /// command insists on exactly one.
    pub selected: Vec<RetailerId>,
    pub format: Format,
    pub color: bool,
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

        let factory = Factory {
            env: env.clone(),
            config: config.clone(),
            paths: paths.clone(),
            backend: backend(env.secret_backend.as_deref()),
            store: cli.store().map(str::to_string),
        };

        Ok(App {
            registry: Registry::new(move |id| factory.build(id)),
            selected: cli.retailer.clone(),
            config,
            config_file,
            paths,
            env,
            format,
            color,
        })
    }

    /// The credential store one shop's secrets are filed in. Per family, not
    /// per shop: one Club Plus login covers both Foodstuffs banners.
    pub fn secrets(&self, id: RetailerId) -> Secrets {
        Secrets::new(
            format!("{APP}.{}", family(id)),
            backend(self.env.secret_backend.as_deref()),
            &self.paths.state_dir,
        )
    }

    pub fn out(&self) -> Out {
        Out::stdout(self.format, !self.color)
    }

    /// The one shop a per-retailer command talks to.
    pub fn retailer(&self) -> AppResult<RetailerId> {
        match self.selected.as_slice() {
            [one] => Ok(*one),
            [] => self.config.retailer.ok_or_else(|| {
                AppError::usage(
                    "no shop selected: pass `-b nw`, `-b pns` or `-b ww`, or set one for good \
                     with `gsnz -b <shop> store set <store>`",
                )
            }),
            many => Err(AppError::usage(format!(
                "-b names {} shops, and only `compare` can span more than one",
                many.len()
            ))),
        }
    }

    /// The shops `compare` puts side by side.
    ///
    /// A `-b` list narrows it; otherwise the config decides, and its default is
    /// all three. The single-shop default is deliberately *not* consulted: a
    /// comparison with one column is not a comparison.
    pub fn compare_span(&self) -> Vec<RetailerId> {
        if !self.selected.is_empty() {
            return self.selected.clone();
        }
        if self.config.compare.retailers.is_empty() {
            return RetailerId::ALL.to_vec();
        }
        self.config.compare.retailers.clone()
    }

    /// Every retailer asked for, with the ones that could not be built reported
    /// rather than raised.
    ///
    /// One lapsed session must not hide the other two shops' prices: a compare
    /// with a gap in it is worth more than no compare at all.
    pub fn handles(
        &self,
        ids: &[RetailerId],
    ) -> (Vec<Handle>, Vec<(RetailerId, gsnz_core::Error)>) {
        let mut handles = Vec::new();
        let mut failures = Vec::new();
        for &id in ids {
            match self.registry.get(id) {
                Ok(handle) => handles.push(handle),
                Err(e) => failures.push((id, e)),
            }
        }
        (handles, failures)
    }

    pub fn handle(&self) -> AppResult<Handle> {
        Ok(self.registry.get(self.retailer()?)?)
    }

    /// One entry per credential the requested shops need, each with a shop to
    /// act through.
    ///
    /// This is what stops `auth login` asking for the same Club Plus password
    /// twice: `-b nw,pns` is one credential, not two. With no `-b` it is every
    /// credential, which is the whole of "set this tool up".
    pub fn auth_targets(&self) -> Vec<AuthTarget> {
        let requested = if self.selected.is_empty() {
            RetailerId::ALL.to_vec()
        } else {
            self.selected.clone()
        };
        let mut seen: Vec<&'static str> = Vec::new();
        let mut targets = Vec::new();
        for id in requested {
            let family = family(id);
            if seen.contains(&family) {
                continue;
            }
            seen.push(family);
            targets.push(AuthTarget {
                through: id,
                covers: family_shops(family),
            });
        }
        targets
    }
}

/// One credential, and the shops it speaks for.
pub struct AuthTarget {
    /// The adapter to run the command through. Any shop in the family will do:
    /// they share the credential store.
    pub through: RetailerId,
    pub covers: Vec<RetailerId>,
}

/// What builds an adapter. Cloned into the registry's closure so the registry
/// does not have to borrow the `App` that owns it.
#[derive(Clone)]
struct Factory {
    env: Overrides,
    config: Config,
    paths: Paths,
    backend: Backend,
    store: Option<String>,
}

impl Factory {
    fn build(&self, id: RetailerId) -> Result<Handle> {
        let family = family(id);
        // Config and state directories are shared; the credential *service* is
        // per family, because both halves file a password under the same
        // account name and one would otherwise overwrite the other.
        let secrets = Secrets::new(
            format!("{APP}.{family}"),
            self.backend,
            &self.paths.state_dir,
        );
        let paths = self.paths.scoped(family);
        let password = net_kit::password::Source::named(
            self.config.auth.password_command.as_deref(),
            &secrets,
        );
        match id {
            RetailerId::NewWorld | RetailerId::PaknSave => {
                let banner = foodstuffs::convert::banner(id).expect("a Foodstuffs banner");
                let ov = self.retailer_env(id);
                let mut endpoints = fsnz_api::Endpoints::defaults(banner);
                if let Some(origin) = &ov.origin {
                    endpoints = endpoints.with_origin(origin.clone());
                }
                if let Some(api) = &ov.api {
                    endpoints = endpoints.with_api(api.clone());
                }
                let mut clubplus = fsnz_api::ClubPlusEndpoints::default();
                if let Some(origin) = &self.env.clubplus.origin {
                    clubplus = clubplus.with_login(origin.clone());
                }
                if let Some(api) = &self.env.clubplus.api {
                    clubplus = clubplus.with_api(api.clone());
                }
                Ok(Arc::new(foodstuffs::Foodstuffs::new(foodstuffs::Setup {
                    id,
                    endpoints,
                    clubplus,
                    paths,
                    secrets,
                    token_command: self.config.retailer(id).token_command.clone(),
                    explicit_token: ov.token.clone(),
                    password,
                    store_id: self.store_id(id),
                })?))
            }
            RetailerId::Woolworths => {
                let ov = &self.env.woolworths;
                let mut endpoints = wwnz_api::Endpoints::default();
                if let Some(origin) = &ov.origin {
                    endpoints = endpoints.with_origin(origin.clone());
                }
                // Woolworths' second host is Auth0, not an API; `Retailer.api`
                // carries it because the shape is the same.
                if let Some(auth) = &ov.api {
                    endpoints = endpoints.with_auth(auth.clone());
                }
                Ok(Arc::new(woolworths::Woolworths::new(woolworths::Setup {
                    endpoints,
                    paths,
                    secrets,
                    password,
                    // The *flag* only. A Woolworths store is bound to the cart
                    // server-side, so a saved one is already in effect and a
                    // per-run override is a thing this shop cannot do -- which
                    // the adapter says outright rather than ignoring.
                    store_override: self.store.clone(),
                    debug: self.env.debug_auth,
                })?))
            }
        }
    }

    fn retailer_env(&self, id: RetailerId) -> &crate::env::Retailer {
        match id {
            RetailerId::NewWorld => &self.env.newworld,
            RetailerId::PaknSave => &self.env.paknsave,
            RetailerId::Woolworths => &self.env.woolworths,
        }
    }

    fn store_id(&self, id: RetailerId) -> Option<String> {
        self.store
            .clone()
            .or_else(|| self.retailer_env(id).store_id.clone())
            .or_else(|| self.config.retailer(id).store_id.clone())
    }
}

/// The shops one credential covers.
///
/// New World and PAK'nSAVE are one Club Plus account, so signing into either
/// signs into both -- and signing out of either signs out of both. Every auth
/// command works in these units, and names them, because a user who is not
/// told this signs in twice with the same password.
pub fn family_shops(family: &str) -> Vec<RetailerId> {
    RetailerId::ALL
        .into_iter()
        .filter(|id| self::family(*id) == family)
        .collect()
}

/// How to say what a login covers: "New World and PAK'nSAVE".
pub fn name_shops(ids: &[RetailerId]) -> String {
    match ids {
        [] => String::new(),
        [one] => one.name().to_string(),
        [rest @ .., last] => format!(
            "{} and {}",
            rest.iter().map(|r| r.name()).collect::<Vec<_>>().join(", "),
            last.name()
        ),
    }
}

/// Which credential namespace a shop belongs to.
///
/// New World and PAK'nSAVE share one Club Plus login, so they share a
/// namespace; Woolworths has its own. [`RetailerId::catalogue`] already draws
/// exactly that line, for the same underlying reason.
pub fn family(id: RetailerId) -> &'static str {
    id.catalogue().unwrap_or_else(|| id.id())
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
    fn shops_are_named_the_way_a_sentence_would() {
        assert_eq!(name_shops(&[RetailerId::Woolworths]), "Woolworths");
        assert_eq!(
            name_shops(&[RetailerId::NewWorld, RetailerId::PaknSave]),
            "New World and PAK'nSAVE"
        );
    }

    #[test]
    fn one_club_plus_account_covers_both_banners() {
        assert_eq!(
            family_shops("foodstuffs"),
            vec![RetailerId::NewWorld, RetailerId::PaknSave]
        );
        assert_eq!(family_shops("woolworths"), vec![RetailerId::Woolworths]);
    }

    #[test]
    fn the_two_foodstuffs_banners_share_a_credential_namespace() {
        // One Club Plus login covers both, so filing them apart would mean
        // logging in twice for one account.
        assert_eq!(family(RetailerId::NewWorld), "foodstuffs");
        assert_eq!(family(RetailerId::PaknSave), "foodstuffs");
        assert_eq!(family(RetailerId::Woolworths), "woolworths");
    }

    #[test]
    fn an_unknown_backend_name_falls_back_rather_than_failing() {
        assert_eq!(backend(Some("file")), Backend::File);
        assert_eq!(backend(Some("keyring")), Backend::Keyring);
        assert_eq!(backend(Some("nonsense")), Backend::detect());
    }
}
