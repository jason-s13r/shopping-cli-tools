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
use crate::error::AppResult;
use crate::retailers::{woolworths, Handle, Lazy};

/// The name the platform files this tool's config and state under. Its own,
/// not shared with `fsnz` or the combined `gsnz`: two tools reading one
/// another's tokens would be a surprise the first time a logout took both down.
pub const APP: &str = "woolworths-nz-cli";

/// The one shop this tool speaks for. Named rather than written out at each
/// call site, because `gsnz-core` types carry it and none of them assume it.
pub const RETAILER: RetailerId = RetailerId::Woolworths;

pub struct App {
    pub config: Config,
    pub config_file: PathBuf,
    pub paths: Paths,
    pub env: Overrides,
    adapter: Lazy,
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
            adapter: Lazy::new(move || factory.build()),
            config,
            config_file,
            paths,
            env,
            format,
            color,
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

    pub fn out(&self) -> Out {
        Out::stdout(self.format, !self.color)
    }

    /// The adapter every command talks through.
    pub fn handle(&self) -> AppResult<Handle> {
        Ok(self.adapter.get()?)
    }
}

/// What builds the adapter. Cloned into the lazy cell's closure so it does not
/// have to borrow the `App` that owns it.
#[derive(Clone)]
struct Factory {
    env: Overrides,
    config: Config,
    paths: Paths,
    backend: Backend,
    store: Option<String>,
}

impl Factory {
    fn build(&self) -> Result<Handle> {
        let secrets = Secrets::new(APP, self.backend, &self.paths.state_dir);
        let password = net_kit::password::Source::named(
            self.config.auth.password_command.as_deref(),
            &secrets,
        );

        let mut endpoints = wwnz_api::Endpoints::default();
        if let Some(origin) = &self.env.origin {
            endpoints = endpoints.with_origin(origin.clone());
        }
        // Auth0 is a separate host from the storefront, which is where the
        // GraphQL endpoint lives.
        if let Some(auth) = &self.env.auth_origin {
            endpoints = endpoints.with_auth(auth.clone());
        }
        Ok(Arc::new(woolworths::Woolworths::new(woolworths::Setup {
            endpoints,
            paths: self.paths.clone(),
            secrets,
            password,
            // The *flag* only. The store is bound to the cart server-side, so a
            // saved one is already in effect and a per-run override is a thing
            // this site cannot do -- which the adapter says outright rather
            // than ignoring.
            store_override: self.store.clone(),
            debug: self.env.debug_auth,
        })?))
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
