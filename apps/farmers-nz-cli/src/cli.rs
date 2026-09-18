//! The flags, and nothing else. Parsing is separated from doing so that
//! `--help` is readable as one file and no command function has to know how it
//! was reached.

use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "farmers",
    about = "Search and shop Farmers New Zealand",
    version = crate::build::short_version(),
    long_version = crate::build::long_version(),
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Print machine-readable JSON instead of a table.
    #[arg(long, global = true)]
    pub json: bool,

    /// Show the window when a browser has to buy admission to the storefront.
    ///
    /// The warm-up is headless by default and usually passes that way; a
    /// window gives the bot manager more to score and is the thing to reach
    /// for when a headless run is refused.
    #[arg(long, global = true)]
    pub headful: bool,

    #[command(subcommand)]
    pub command: Command,
}

/// The command that would fix a failure, spelled for this binary.
pub fn advice(error: &crate::error::AppError) -> Option<String> {
    use farmers_api::Error as Api;
    let crate::error::AppError::Api(api) = error else {
        return None;
    };
    Some(match api {
        Api::NotSignedIn | Api::SessionExpired => "run `farmers auth login`".into(),
        Api::LoginRefused { .. } | Api::NoSession { .. } => {
            "check the email and password, then `farmers auth login`".into()
        }
        // Names the browser, because that is the lever that actually moves.
        // Admission is only granted to cookies a browser earned, so neither
        // of the obvious suggestions is any use: no credential opens this,
        // and retrying does not change where a cookie came from.
        Api::Denied { .. } | Api::Challenged => {
            "Farmers only admits cookies a real browser earned. Check `camoufox` is \
             installed (`farmers doctor`), then try again with --headful. Search and \
             suggest go elsewhere and keep working regardless"
                .into()
        }
        Api::NoSuchRegion(_) => "run `farmers regions` for the ones there are".into(),
        Api::NoSuchCategory(_) => "run `farmers categories` for the ones there are".into(),
        Api::NoSuchProduct(_) => "product codes are the digits on the end of a product URL".into(),
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

    /// Only this brand.
    #[arg(long)]
    pub brand: Option<String>,

    /// Only this colour.
    #[arg(long)]
    pub colour: Option<String>,

    /// Only this size.
    #[arg(long)]
    pub size: Option<String>,

    /// Only products in stock.
    #[arg(long)]
    pub in_stock: bool,

    /// Narrow by any other facet, as `name=value`. Repeatable.
    ///
    /// `--facets` lists the names a search offered.
    #[arg(long, value_name = "NAME=VALUE")]
    pub filter: Vec<String>,

    /// How to order: relevance, newest, name, name-desc, price, price-desc.
    #[arg(long)]
    pub sort: Option<String>,

    /// Print the facets the index offered alongside the results.
    #[arg(long)]
    pub facets: bool,

    /// Look up each result's price, which the search index does not carry.
    ///
    /// One extra request per product, so it is off by default.
    #[arg(long)]
    pub prices: bool,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Search the catalogue.
    Search {
        query: String,
        #[command(flatten)]
        listing: Listing,
    },

    /// List a category's or a brand's products.
    ///
    /// Takes a category id, or `Brands/<name>`; `farmers categories` lists
    /// them.
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
        #[arg(long, default_value_t = 2)]
        depth: usize,
        /// Include the 1,354 brands, which the tree files as a category and
        /// which are left out unless asked for.
        #[arg(long)]
        brands: bool,
    },

    /// What the site would suggest for a partial search term.
    Suggest {
        term: String,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },

    /// One product: its price, its stock and what it is made of.
    Product {
        /// A product code, or a product URL pasted from the site.
        code: String,
        /// Also list what can be bought under it.
        #[arg(long)]
        variants: bool,
    },

    /// The buyable sizes and colours under a product code.
    Variants {
        /// A product code, or a product URL pasted from the site.
        code: String,
    },

    /// Which stores have a product.
    Stock {
        /// A product code, or a product URL pasted from the site.
        code: String,
        /// One region rather than all thirteen, by code or by name.
        #[arg(long)]
        region: Option<String>,
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
    },

    /// The regions `stock` can be narrowed to.
    Regions,

    /// The basket.
    Cart {
        #[command(subcommand)]
        action: CartAction,
    },

    /// Saved lists. Needs an account.
    Wishlist {
        #[command(subcommand)]
        action: WishlistAction,
    },

    /// What has been bought. Needs an account.
    Orders,

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
pub enum CartAction {
    /// What is in the basket.
    List,
    /// Put something in.
    ///
    /// Takes a buyable code, not a master: `farmers product <code>` lists the
    /// variants under one.
    Add {
        /// A product code, or a product URL pasted from the site.
        code: String,
        #[arg(long, short, default_value_t = 1)]
        quantity: i64,
    },
    /// Set a line to an exact quantity, by the number `cart list` shows.
    Set { item: usize, quantity: i64 },
    /// Take a line out, by the number `cart list` shows.
    Remove { item: usize },
    /// Apply a promotion code.
    Promo { code: String },
}

#[derive(Subcommand, Debug)]
pub enum WishlistAction {
    /// What is saved.
    List,
    /// Save a product to the preferred list.
    Add {
        /// A product code, or a product URL pasted from the site.
        code: String,
    },
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
        /// A command that prints the password, for a password manager.
        #[arg(long, conflicts_with = "stdin")]
        password_command: Option<String>,
        /// Do not keep the password. A lapsed session then has to be signed in
        /// by hand.
        #[arg(long)]
        no_store_password: bool,
    },
    /// Sign in again from the stored credentials, with nobody watching.
    ///
    /// Checks whether it needs to run at all first, so this is safe to put on
    /// a timer.
    Refresh {
        /// Sign in again even if the current session still works.
        #[arg(long)]
        force: bool,
    },
    /// Whether this tool holds a usable session.
    Status,
    /// Give the session back and forget it, password included.
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
        use farmers_api::Error as Api;
        let advice = advice(&crate::error::AppError::Api(Api::NotSignedIn)).expect("has advice");
        assert!(advice.contains("farmers auth login"), "{advice}");
    }

    #[test]
    fn being_refused_by_the_bot_manager_points_at_the_browser_not_a_credential() {
        // No credential opens it and no amount of retrying changes where a
        // cookie came from, so both of the obvious suggestions are actively
        // wrong. The browser is the one lever that was measured to move this.
        use farmers_api::Error as Api;
        let advice = advice(&crate::error::AppError::Api(Api::Challenged)).expect("has advice");
        assert!(!advice.contains("auth login"), "{advice}");
        assert!(advice.contains("camoufox"), "{advice}");
        assert!(advice.contains("--headful"), "{advice}");
        assert!(advice.contains("Search and suggest"), "{advice}");
    }

    #[test]
    fn a_usage_failure_has_no_retailer_advice_to_give() {
        assert_eq!(advice(&crate::error::AppError::usage("bad flag")), None);
    }
}
