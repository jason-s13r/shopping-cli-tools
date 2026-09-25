//! The only place in this program that reads the environment.
//!
//! Every library under `packages/` takes plain values; a `clippy.toml` in each
//! forbids `std::env::var` outright. That rule has to end somewhere, and it
//! ends here: one struct, read once, before anything is spawned. Below this
//! boundary the program is a function of its arguments.

use std::path::PathBuf;

/// What the environment says, before flags and config have their turn.
#[derive(Clone, Debug, Default)]
pub struct Overrides {
    pub config_dir: Option<PathBuf>,
    pub state_dir: Option<PathBuf>,
    pub secret_backend: Option<String>,
    pub update_api: Option<String>,
    pub github_token: Option<String>,
    /// Narrate what the client is doing on stderr. Nothing it prints is a
    /// credential: cookies appear by name only and no query string is included,
    /// which matters more here than elsewhere because the query strings carry
    /// signed action tokens.
    pub debug: bool,
    pub no_color: bool,
    /// The login shell's path, which is how `completions` guesses which script
    /// to write when none is named.
    pub shell: Option<String>,
    /// The storefront. Everything is served from this one host, so unlike the
    /// Woolworths tool there is no second origin to override. It exists so the
    /// integration suite can point the binary at a mock server, and so a broken
    /// default can be worked around without a release.
    pub origin: Option<String>,
    /// The browser the client presents as, by `wreq-util` profile name.
    ///
    /// The escape hatch for the one setting on this storefront that stops
    /// working on its own: Cloudflare scores the fingerprint, which profile it
    /// accepts has changed once already, and a refused one 403s every request
    /// rather than degrading. Without this, that costs a release.
    pub emulation: Option<String>,
    /// Seconds between requests, beating `network.request_interval`. Parsed
    /// in `App`, where a bad value can be refused as usage.
    pub request_interval: Option<String>,
    /// The Python to run the challenge browser with, when the `camoufox`
    /// launcher's own shebang is not the right one.
    pub browser_python: Option<String>,
    /// Show the challenge browser's window. `--headful` is the usual way.
    pub headful: bool,
}

impl Overrides {
    /// Read once, and shared.
    ///
    /// `--version` needs the state directory to say how this binary was
    /// installed, and clap builds that string before `App` exists. Memoising
    /// keeps "the environment is read once" literally true rather than nearly
    /// true.
    pub fn get() -> &'static Overrides {
        static CELL: std::sync::OnceLock<Overrides> = std::sync::OnceLock::new();
        CELL.get_or_init(Overrides::read)
    }

    pub fn read() -> Overrides {
        Overrides {
            config_dir: path("TWLNZ_CONFIG_DIR"),
            state_dir: path("TWLNZ_STATE_DIR"),
            secret_backend: var("TWLNZ_SECRET_BACKEND"),
            update_api: var("TWLNZ_UPDATE_API"),
            // `gh` writes one and the Actions runner the other; either lifts
            // the anonymous rate limit on the release list.
            github_token: var("GITHUB_TOKEN").or_else(|| var("GH_TOKEN")),
            debug: flag("TWLNZ_DEBUG"),
            // Set at all, to anything, means no colour. That is what the
            // convention says, so an empty value is not an override.
            no_color: std::env::var_os("NO_COLOR").is_some(),
            shell: var("SHELL"),
            origin: var("TWLNZ_ORIGIN"),
            emulation: var("TWLNZ_EMULATION"),
            request_interval: var("TWLNZ_REQUEST_INTERVAL"),
            browser_python: var("TWLNZ_BROWSER_PYTHON"),
            headful: flag("TWLNZ_HEADFUL"),
        }
    }
}

/// An empty variable is treated as unset: `TWLNZ_STATE_DIR=` in a shell script
/// means "I did not set this", not "use the current directory".
fn var(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn path(name: &str) -> Option<PathBuf> {
    var(name).map(PathBuf::from)
}

/// Set to anything but a denial means on: `TWLNZ_DEBUG=1` and `TWLNZ_DEBUG=yes`
/// should not need to be told apart.
fn flag(name: &str) -> bool {
    var(name).is_some_and(|v| !matches!(v.to_lowercase().as_str(), "0" | "false" | "no"))
}
