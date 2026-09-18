//! The only place in this program that reads the environment.
//!
//! Every library under `packages/` takes plain values; a `clippy.toml` in each
//! forbids `std::env::var` outright. That rule ends here: one struct, read
//! once. Below this boundary the program is a function of its arguments.

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
    /// credential: URLs and step names only, no headers and no bodies.
    pub debug: bool,
    pub no_color: bool,
    /// The login shell's path, which is how `completions` guesses which script
    /// to write when none is named.
    pub shell: Option<String>,
    /// The storefront, for pointing an integration suite at a mock.
    pub site_origin: Option<String>,
    /// The search backend, which is a different company's host.
    pub search_origin: Option<String>,
    /// The Constructor.io index key. Overridable because it is Farmers' to
    /// rotate, so a stale build is fixable without a release.
    pub search_key: Option<String>,
    /// The browser the client presents as, by `wreq-util` profile name.
    pub emulation: Option<String>,
    /// The Python that can `import camoufox`, when the one named by the
    /// `camoufox` launcher's shebang is not the right one.
    pub browser_python: Option<String>,
    /// Show the warm-up browser's window. The flag is the usual way; this is
    /// for setting it once in a shell that keeps needing it.
    pub headful: bool,
}

impl Overrides {
    /// Read once, and shared.
    ///
    /// `--version` needs the state directory to say how this binary was
    /// installed, and clap builds that string before `App` exists.
    pub fn get() -> &'static Overrides {
        static CELL: std::sync::OnceLock<Overrides> = std::sync::OnceLock::new();
        CELL.get_or_init(Overrides::read)
    }

    pub fn read() -> Overrides {
        Overrides {
            config_dir: path("FMNZ_CONFIG_DIR"),
            state_dir: path("FMNZ_STATE_DIR"),
            secret_backend: var("FMNZ_SECRET_BACKEND"),
            update_api: var("FMNZ_UPDATE_API"),
            // `gh` writes one and the Actions runner the other; either lifts
            // the anonymous rate limit on the release list.
            github_token: var("GITHUB_TOKEN").or_else(|| var("GH_TOKEN")),
            debug: flag("FMNZ_DEBUG"),
            // Set at all, to anything, means no colour. That is what the
            // convention says, so an empty value is not an override.
            no_color: std::env::var_os("NO_COLOR").is_some(),
            shell: var("SHELL"),
            site_origin: var("FMNZ_SITE_ORIGIN"),
            search_origin: var("FMNZ_SEARCH_ORIGIN"),
            search_key: var("FMNZ_SEARCH_KEY"),
            emulation: var("FMNZ_EMULATION"),
            browser_python: var("FMNZ_BROWSER_PYTHON"),
            headful: flag("FMNZ_HEADFUL"),
        }
    }
}

/// An empty variable is treated as unset: `FMNZ_STATE_DIR=` in a shell script
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

/// Set to anything but a denial means on: `FMNZ_DEBUG=1` and `FMNZ_DEBUG=yes`
/// should not need to be told apart.
fn flag(name: &str) -> bool {
    var(name).is_some_and(|v| !matches!(v.to_lowercase().as_str(), "0" | "false" | "no"))
}
