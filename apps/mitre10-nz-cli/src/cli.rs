//! The flags, and nothing else. Parsing is separated from doing so that
//! `--help` is readable as one file and no command function has to know how it
//! was reached.

use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "mitre10",
    about = "Search and shop Mitre 10 New Zealand",
    version = crate::build::short_version(),
    long_version = crate::build::long_version(),
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Print machine-readable JSON instead of a table.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

/// The command that would fix a failure, spelled for this binary.
pub fn advice(error: &crate::error::AppError) -> Option<String> {
    use mitre10_api::Error as Api;
    let crate::error::AppError::Api(api) = error else {
        return None;
    };
    Some(match api {
        Api::NotSignedIn | Api::SessionExpired => "run `mitre10 auth login`".into(),
        // The one failure where signing in again is the *only* move: a refresh
        // has already been tried and refused.
        Api::LoginLapsed => "the stored login has expired; run `mitre10 auth login` again".into(),
        Api::BadCredentials => "check the email and password, then `mitre10 auth login`".into(),
        Api::LoginUnavailable => "signing in is not working from a bare client at the moment; \
             everything except the wishlist and orders works signed out"
            .into(),
        Api::NoSuchStore(_) => "run `mitre10 stores` for the stores there are".into(),
        Api::NoSuchCategory(_) => "run `mitre10 categories` for the ones there are".into(),
        // Not "try again": the point of the message is that trying again
        // sooner is the wrong move.
        Api::RateLimited { .. } => "wait a few minutes before running this again".into(),
        Api::Cors => {
            "this is a client fault rather than a sign-in one; `mitre10 doctor` checks it".into()
        }
        _ => return None,
    })
}

/// The parser, in one place so `completions` generates for exactly what runs.
pub fn command() -> clap::Command {
    <Cli as clap::CommandFactory>::command()
}

/// The flags every listing shares.
#[derive(Args, Debug, Clone, Default)]
pub struct Listing {
    /// Which page, from 0.
    #[arg(long, default_value_t = 0)]
    pub page: u64,

    /// How many products a page holds.
    #[arg(long, value_name = "N")]
    pub limit: Option<u64>,

    /// Only products this store has, by its id.
    #[arg(long, value_name = "STORE")]
    pub store: Option<String>,

    /// Only what can be delivered to this postcode.
    #[arg(long, value_name = "POSTCODE")]
    pub postcode: Option<String>,

    /// Only this brand.
    #[arg(long)]
    pub brand: Option<String>,

    /// Only this colour.
    #[arg(long)]
    pub colour: Option<String>,

    /// Only this size.
    #[arg(long)]
    pub size: Option<String>,

    /// Cheapest price to show.
    #[arg(long, value_name = "DOLLARS")]
    pub min: Option<f64>,

    /// Dearest price to show.
    #[arg(long, value_name = "DOLLARS")]
    pub max: Option<f64>,

    /// The index to order by. Only `relevance` is confirmed to exist; any
    /// other name is passed through as a replica.
    #[arg(long)]
    pub sort: Option<String>,

    /// Print the facets the index offered alongside the results.
    #[arg(long)]
    pub facets: bool,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Search the catalogue.
    Search {
        query: String,
        #[command(flatten)]
        listing: Listing,
    },

    /// List a category's products.
    ///
    /// Takes a category code; `mitre10 categories` lists them.
    Browse {
        category: String,
        #[command(flatten)]
        listing: Listing,
    },

    /// The category tree.
    Categories {
        /// Show only categories whose name or path contains this.
        query: Option<String>,
        /// How deep to go. 1 is the top of the menu.
        #[arg(long, default_value_t = 3)]
        depth: usize,
    },

    /// What the site would suggest for a partial search term.
    Suggest {
        term: String,
        #[arg(long, default_value_t = 10)]
        limit: u64,
    },

    /// One product: its price, its stock and what it is made of.
    Product {
        /// A product code, or a product URL pasted from the site.
        code: String,
        /// Price it for this store, by its id.
        #[arg(long, value_name = "STORE")]
        store: Option<String>,
    },

    /// Which stores have a product, nationwide.
    Stock {
        /// A product code, or a product URL pasted from the site.
        code: String,
        /// Show only stores whose name contains this.
        #[arg(long)]
        near: Option<String>,
        /// Show only stores that have it.
        #[arg(long)]
        available: bool,
    },

    /// Price several products at once.
    Prices {
        /// Product codes. Reads them from stdin, one per line, when none are
        /// given.
        codes: Vec<String>,
        #[arg(long, value_name = "STORE")]
        store: Option<String>,
    },

    /// The stores.
    Stores {
        /// Show only stores whose name or town contains this.
        query: Option<String>,
        /// Stores near a postcode rather than all of them.
        #[arg(long)]
        postcode: Option<String>,
        /// Stores near a point, as `lat,lon`.
        #[arg(long, value_name = "LAT,LON")]
        near: Option<String>,
    },

    /// Set or show the store commands use when none is named.
    Store {
        #[command(subcommand)]
        action: StoreAction,
    },

    /// The basket.
    Cart {
        #[command(subcommand)]
        action: CartAction,
    },

    /// Saved items.
    Wishlist {
        #[command(subcommand)]
        action: WishlistAction,
    },

    /// What has been bought.
    Orders {
        /// Which page, from 0.
        #[arg(long, default_value_t = 0)]
        page: u64,
        #[arg(long, default_value_t = 20)]
        limit: u64,
    },

    /// Signing in, and signing out.
    Auth {
        #[command(subcommand)]
        action: AuthAction,
    },

    /// Read and change the settings file.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// What is set up, and what works.
    Doctor,

    /// Replace this binary with a newer release.
    Update {
        /// A version to install, rather than the newest.
        version: Option<String>,
        /// Report what would be installed and stop.
        #[arg(long)]
        check: bool,
        /// Consider pre-releases.
        #[arg(long)]
        pre_release: bool,
    },

    /// Print a shell completion script.
    Completions {
        /// bash, zsh, fish, elvish or powershell. Guessed from $SHELL when not
        /// given.
        shell: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum StoreAction {
    /// Print the store commands use when none is named.
    Show,
    /// Use this store from now on, by its id.
    Set { id: String },
    /// Forget it.
    Clear,
}

#[derive(Subcommand, Debug)]
pub enum CartAction {
    /// What is in the basket.
    List,
    /// Start a new basket.
    New,
    /// Put something in.
    Add {
        code: String,
        #[arg(long, default_value_t = 1)]
        quantity: i64,
        /// Collect from this store, by its id.
        #[arg(long, value_name = "STORE")]
        store: Option<String>,
        /// Have it delivered rather than collected.
        #[arg(long)]
        deliver: bool,
    },
    /// Set a line to an exact quantity, by the item number `cart list` shows.
    Set { item: i64, quantity: i64 },
    /// Take a line out, by the item number `cart list` shows.
    Remove { item: i64 },
    /// Collect the basket or have it delivered.
    Fulfilment {
        /// `collect`, `delivery` or `express`.
        how: String,
    },
    /// Apply a voucher code.
    Voucher { code: String },
}

#[derive(Subcommand, Debug)]
pub enum WishlistAction {
    /// What is saved.
    List,
    /// Save a product.
    Add { code: String },
    /// Empty the list.
    ///
    /// All of it: the storefront has no call that takes one item back off,
    /// so there is no `remove` to go with this.
    Clear,
}

#[derive(Subcommand, Debug)]
pub enum AuthAction {
    /// Sign in with an email and password.
    Login {
        #[arg(long)]
        email: Option<String>,
        /// Read the password from stdin rather than prompting.
        #[arg(long)]
        stdin: bool,
    },
    /// Whether this tool holds usable credentials.
    Status,
    /// Mint a fresh access token from the stored refresh token.
    Refresh,
    /// Give the tokens back and forget them.
    Logout,
}

#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// Every setting and its value.
    List,
    /// One setting's value.
    Get { key: String },
    /// Change a setting.
    Set { key: String, value: String },
    /// Put a setting back to its default.
    Unset { key: String },
    /// Where the config file is.
    Path,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_parser_is_well_formed() {
        // clap panics on a malformed command tree, and only at run time --
        // this is what turns that into a test failure.
        command().debug_assert();
    }

    #[test]
    fn advice_names_this_binary_rather_than_the_library() {
        use mitre10_api::Error as Api;
        let advice = advice(&crate::error::AppError::Api(Api::NotSignedIn)).expect("has advice");
        assert!(advice.contains("mitre10 auth login"), "{advice}");
    }

    #[test]
    fn a_cors_failure_does_not_advise_signing_in() {
        // It arrives as a 403 and looks like an auth problem; sending someone
        // to re-enter a password that was never wrong is the thing to avoid.
        use mitre10_api::Error as Api;
        let advice = advice(&crate::error::AppError::Api(Api::Cors)).expect("has advice");
        assert!(!advice.contains("auth login"), "{advice}");
    }

    #[test]
    fn a_usage_failure_has_no_retailer_advice_to_give() {
        assert_eq!(advice(&crate::error::AppError::usage("bad flag")), None);
    }
}
