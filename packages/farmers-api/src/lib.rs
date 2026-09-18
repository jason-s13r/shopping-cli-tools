//! Farmers New Zealand.
//!
//! An Intershop ICM storefront behind Akamai Bot Manager, with its search
//! farmed out to a third party. Three backends, and the split is the shape of
//! this crate:
//!
//! | what | who answers | module |
//! |---|---|---|
//! | products, prices, categories, variations | Intershop REST, JSON | [`Client`] |
//! | search, autocomplete, browse | Constructor.io, JSON | [`search`] |
//! | per-store stock, sign-in, the account header | Intershop `ViewX-` pipelines, HTML | [`extract`] |
//!
//! Four things shape everything else here.
//!
//! **A denial is an HTTP 200.** Akamai serves its refusal as a success with an
//! HTML body, so nothing about the status says anything is wrong and a JSON
//! decode against it fails in a way that reads like a schema change. Every
//! response is checked for it; see [`Error::Denied`].
//!
//! **The session must be warmed.** A cold REST call is denied. One fetch of the
//! home page earns the cookies, and [`Client`] does it for itself. What it
//! earns is an *unvalidated* `_abck` -- no JavaScript sensor is ever posted --
//! and the REST API accepts that, which is why there is no headless browser
//! anywhere in this crate. See [`http`].
//!
//! **The REST API cannot search.** `searchTerm` is accepted and silently
//! ignored: the answer is the whole catalogue, in its natural order, with a
//! plausible-looking total. Nothing here sends it. See [`search`].
//!
//! **A product code is not a leaf.** `6867065` is a master with a price range
//! and no stock of its own; `6867065002` is the size that can be bought. The
//! search index returns the first and the basket takes the second, so
//! [`Client::price_hits`] joins on the variant rather than on what it was
//! given.
//!
//! Everything arrives optional. Half of this is scraped and the rest is
//! undocumented, so a field Farmers renames should cost a column rather than a
//! command.
//!
//! This crate speaks its own vendor-shaped types and does not depend on a
//! shared domain crate.

/// This crate's own version, for a consumer that reports what it was
/// built against.
///
/// `env!` expands where it is written, so this is the one place it can be
/// read from: a consumer writing `env!("CARGO_PKG_VERSION")` would get its
/// own version back, not this one.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod auth;
mod client;
mod domain;
mod endpoints;
mod error;
pub mod extract;
pub mod http;
pub mod search;
pub mod session;
pub mod stores;
pub mod wire;

pub use client::{Client, Reauth, SessionStore, Warmer, DEFAULT_DEPTH};
pub use domain::{
    Account, Cart, CartLine, Category, CategoryRef, Facet, FacetOption, Hit, Image, Listing, Money,
    Order, Product, Store, StoreStock, Suggestion, Variant, VariationValue, Wishlist, WishlistItem,
};
pub use endpoints::{Endpoints, CONSTRUCTOR_KEY, LOCALE, SITE};
pub use error::{from_http, is_challenge_page, is_deny_page, Error, Result};
pub use http::{client_spec, client_spec_for, profile, EMULATION};
pub use search::{Query, SortOrder, Subject, PAGE_SIZE, SORTS};
pub use session::{Session, StoredSession};
pub use stores::{is_region, region, region_name, REGIONS};
