//! The flags, and nothing else. Parsing is separated from doing so that
//! `--help` is readable as one file and no command function has to know how it
//! was reached.

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "twlnz",
    about = "Search and shop The Warehouse New Zealand",
    version = crate::build::short_version(),
    long_version = crate::build::long_version(),
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Print machine-readable JSON instead of a table.
    #[arg(long, global = true)]
    pub json: bool,

    /// Show the window when a browser has to pass Cloudflare's challenge.
    ///
    /// Headless usually passes; a window is the thing to try when it does not.
    #[arg(long, global = true)]
    pub headful: bool,

    #[command(subcommand)]
    pub command: Command,
}

/// The command that would fix a failure, spelled for this binary.
pub fn advice(error: &crate::error::AppError) -> Option<&'static str> {
    let crate::error::AppError::Api(api) = error else {
        return None;
    };
    Some(match api {
        twlnz_api::Error::NotSignedIn => "run `twlnz auth login`",
        twlnz_api::Error::SessionExpired => "run `twlnz auth login` again",
        twlnz_api::Error::LoginRefused { .. } => "check the email and password, then try again",
        twlnz_api::Error::NoSuchStore(_) => "run `twlnz region list` for the regions there are",
        // Not "try again": the point of the message is that trying again
        // sooner is the wrong move.
        twlnz_api::Error::RateLimited { .. } => "wait a few minutes before running this again",
        twlnz_api::Error::Challenged => "run it again with --headful",
        _ => return None,
    })
}

/// The parser, in one place so `completions` generates for exactly what runs.
pub fn command() -> clap::Command {
    <Cli as clap::CommandFactory>::command()
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Search the catalogue.
    Search {
        query: String,
        #[command(flatten)]
        listing: Listing,
    },

    /// List a department's products.
    ///
    /// Takes a category id; `twlnz departments` lists them.
    Browse {
        category: String,
        #[command(flatten)]
        listing: Listing,
    },

    /// Everything currently reduced.
    Specials {
        #[command(flatten)]
        listing: Listing,
    },

    /// The department tree.
    Departments {
        /// Show only the subtree under a department.
        query: Option<String>,
        /// How many levels to fetch. Each costs a request.
        #[arg(long, default_value_t = 1)]
        depth: u32,
    },

    /// One product: its price, its variations and where it is in stock.
    Product {
        /// A product id, e.g. `R3059518`.
        pid: String,
        /// Choose a variation, as `axis=value`. Repeatable.
        #[arg(long = "select", value_name = "AXIS=VALUE")]
        select: Vec<String>,
    },

    /// Which stores have a product on the shelf.
    Stock {
        pid: String,
        /// Look in this region for this command only, e.g. `NZ-CAN` or
        /// `Canterbury`. Defaults to the one `twlnz region` holds.
        #[arg(long)]
        region: Option<String>,
    },

    /// Find a store.
    ///
    /// With a name to search for this looks nationwide, off a cached directory;
    /// with nothing to search for it lists one region, because two hundred
    /// stores is not a listing anyone reads.
    Stores {
        /// Filter by name, suburb or city. Searched across the whole country
        /// unless `--region` says otherwise.
        query: Option<String>,
        /// Look only in this region, e.g. `NZ-CAN` or `Canterbury`. Without a
        /// query this is where the listing comes from, and defaults to the one
        /// `twlnz region` holds.
        #[arg(long)]
        region: Option<String>,
        /// Fetch the store directory again rather than using the cached copy.
        #[arg(long)]
        refresh: bool,
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },

    /// Show or change the store to care about.
    ///
    /// Kept locally, and it fixes `--region` for `stock` and `stores`.
    Store {
        #[command(subcommand)]
        action: StoreAction,
    },

    /// Show or change which island stock is quoted for.
    ///
    /// The Warehouse ranges differently north and south, so this changes what a
    /// listing contains rather than only how it is shown.
    Island {
        #[command(subcommand)]
        action: IslandAction,
    },

    /// Show or change which region `stores` and `stock` look in.
    ///
    /// One of the sixteen `NZ-` regions. A different idea from `island`, which
    /// filters listings -- the site calls both of them "region"; this does not.
    Region {
        #[command(subcommand)]
        action: RegionAction,
    },

    /// What is in the shopping cart, and changing it.
    Cart {
        #[command(subcommand)]
        action: CartAction,
    },

    /// What is saved for later, and changing it. Needs an account.
    ///
    /// With nothing after it, this shows the list -- the reading is what a
    /// wishlist is for, so it does not need naming.
    Wishlist {
        #[command(subcommand)]
        action: Option<WishlistAction>,
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
        /// A specific version, rather than the newest.
        version: Option<String>,
        /// Report what is available, and its release notes, without installing
        /// it.
        #[arg(long)]
        check: bool,
        /// Consider pre-releases.
        #[arg(long)]
        pre_release: bool,
    },

    /// Print a shell completion script.
    Completions {
        /// bash, zsh, fish, elvish or powershell. Guessed from $SHELL if left
        /// off.
        shell: Option<String>,
    },
}

/// The flags every product listing takes. `search`, `browse` and `specials`
/// differ only in what selects the products, so they share this.
#[derive(clap::Args, Debug, Clone)]
pub struct Listing {
    /// How many products to return.
    #[arg(long, default_value_t = 20)]
    pub limit: u32,

    /// How to order the results: `price-low-to-high`, `best-sellers`,
    /// `new-arrivals`, `top-rated`, `product-name-ascending` and so on. An
    /// unfamiliar value is passed through rather than refused, because the site
    /// publishes its own list per listing.
    #[arg(long)]
    pub sort: Option<String>,

    /// Keep only one brand.
    #[arg(long)]
    pub brand: Option<String>,

    /// Keep only one colour.
    #[arg(long)]
    pub color: Option<String>,

    /// Keep only one size.
    #[arg(long)]
    pub size: Option<String>,

    /// Keep only clearance items.
    #[arg(long)]
    pub clearance: bool,

    /// Leave out marketplace items, which are sold by third parties and ship
    /// separately.
    #[arg(long)]
    pub no_marketplace: bool,

    /// Use this island for this command only, without saving it. See
    /// `twlnz island`.
    #[arg(long, value_name = "north|south")]
    pub island: Option<String>,
}

impl Listing {
    /// The refinements these flags amount to, in the order they are sent.
    pub fn facets(&self) -> Vec<twlnz_api::Facet> {
        let mut facets = Vec::new();
        for (name, value) in [
            ("brand", self.brand.as_deref()),
            ("color", self.color.as_deref()),
            ("size", self.size.as_deref()),
        ] {
            if let Some(value) = value {
                facets.push(twlnz_api::Facet::new(name, value));
            }
        }
        if self.clearance {
            facets.push(twlnz_api::Facet::new("clearance", "true"));
        }
        if self.no_marketplace {
            facets.push(twlnz_api::Facet::new("marketplaceItem", "false"));
        }
        facets
    }
}

#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// Every setting, its value, and what it does.
    List,
    /// Print one value, and nothing else.
    Get { key: String },
    /// Change one value. Refused now if it will not parse.
    Set { key: String, value: String },
    /// Put one setting back to its default.
    Unset { key: String },
    /// Where the file is.
    Path,
}

#[derive(Subcommand, Debug)]
pub enum AuthAction {
    /// Sign in.
    Login {
        #[arg(long)]
        email: Option<String>,
        /// A command that prints the password, for a password manager.
        #[arg(long)]
        password_command: Option<String>,
        /// Do not keep the password. A lapsed session then has to be signed in
        /// by hand.
        #[arg(long)]
        no_store_password: bool,
    },
    /// Sign in again with nobody watching, for a cron job or a wrapper.
    ///
    /// Signs in as the email the session was obtained with, using the password
    /// already on hand: `auth.password_command` where one is set, otherwise the
    /// copy `auth login` kept. With neither, it exits 3 rather than reporting a
    /// success nobody is reading.
    ///
    /// A session that still works is left alone. The shopper token carries its
    /// own expiry, so a lapsed one costs no request to spot, and one that looks
    /// good is put to the storefront once -- a session can be dropped at the
    /// other end without its token knowing.
    Refresh {
        /// Sign in again whatever state the session is in.
        ///
        /// The default stops as soon as the session is proven good, which is
        /// what a scheduled run wants. This is for one that is misbehaving in
        /// some way the account page does not report.
        #[arg(long)]
        force: bool,
    },
    /// Who is signed in, and until when.
    Status,
    /// Forget the session and any stored password.
    Logout,
}

#[derive(Subcommand, Debug)]
pub enum CartAction {
    /// What is in it.
    List,
    /// Add a product.
    Add {
        pid: String,
        #[arg(default_value_t = 1)]
        quantity: u32,
    },
    /// Set a line to an exact quantity. Zero removes it.
    Set { pid: String, quantity: u32 },
    /// Take a line out.
    Remove { pid: String },
}

#[derive(Subcommand, Debug)]
pub enum WishlistAction {
    /// What is saved. The same as naming nothing at all.
    List,
    /// Save a product.
    Add { pid: String },
    /// Stop saving a product.
    Remove { pid: String },
    /// Note how many of something is wanted. Zero stops saving it.
    ///
    /// A note to self: nothing is reserved and the site quotes no price for it.
    Set { pid: String, quantity: u32 },
    /// Put a saved product in the cart and stop saving it.
    ///
    /// Two changes, in that order, because the site's own button is two
    /// requests -- so a failure in between leaves the product in the cart and
    /// still on the list rather than in neither.
    MoveToCart {
        pid: String,
        /// How many, when not the number the wishlist has against it.
        quantity: Option<u32>,
    },
}

#[derive(Subcommand, Debug)]
pub enum StoreAction {
    /// What is selected now.
    Show,
    /// Select a store by id, or by enough of its name to be unambiguous.
    ///
    /// Looks in the configured region first, then everywhere else -- so a store
    /// id from any `twlnz stores` listing works without saying where it was.
    Set {
        store: String,
        /// Look only in this region, e.g. `NZ-NTL` or `Northland`.
        #[arg(long)]
        region: Option<String>,
    },
    /// Forget the selected store.
    Clear,
}

#[derive(Subcommand, Debug)]
pub enum IslandAction {
    /// Which island is selected now.
    Show,
    /// The islands there are.
    List,
    /// north or south.
    Set { island: String },
    /// Forget it, and let the site decide.
    Clear,
}

#[derive(Subcommand, Debug)]
pub enum RegionAction {
    /// Which region is selected now.
    Show,
    /// The regions there are.
    List,
    /// A code or a name: `NZ-CAN` or `Canterbury`.
    Set { region: String },
    /// Forget it, and fall back to Auckland.
    Clear,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_parser_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn listing_flags_become_refinements_in_the_order_they_are_sent() {
        let listing = Listing {
            limit: 20,
            sort: None,
            brand: Some("Example Brand".into()),
            color: Some("Blue".into()),
            size: None,
            clearance: true,
            no_marketplace: false,
            island: None,
        };
        let facets = listing.facets();
        assert_eq!(facets.len(), 3);
        assert_eq!(facets[0].name, "brand");
        assert_eq!(facets[1].name, "color");
        assert_eq!(facets[2].name, "clearance");
    }

    #[test]
    fn no_flags_means_no_refinements() {
        let listing = Listing {
            limit: 20,
            sort: None,
            brand: None,
            color: None,
            size: None,
            clearance: false,
            no_marketplace: false,
            island: None,
        };
        assert!(listing.facets().is_empty());
    }
}
