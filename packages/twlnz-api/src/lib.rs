//! The Warehouse New Zealand storefront.
//!
//! A Salesforce Commerce Cloud site, not an API: there is no GraphQL endpoint
//! and no product service. Cart, stores, variations and the wishlist's writes
//! answer clean JSON, but everything that lists products is server-rendered
//! HTML -- the wishlist included -- and this crate reads it: see [`extract`],
//! which is the only module that does.
//!
//! Two things shape everything else here. Writes are a **two-step**: cart,
//! wishlist and stock endpoints need a `verify` token that is minted into a
//! page and expires, so no such call can be made cold. [`Pdp`] is that
//! two-step made into a type. And a product id is **not a leaf**: a listing
//! links to a variation group, the cart takes a variant, and availability is
//! per channel rather than a boolean -- an item can be sold out online and on a
//! shelf at the same time.
//!
//! Everything arrives optional. The HTML half is scraped and the JSON half is
//! undocumented, so a field The Warehouse renames should degrade to a missing
//! column, not a failed command.
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
mod cart;
mod client;
mod domain;
mod endpoints;
mod error;
pub mod extract;
pub mod http;
mod listing;
mod product;
pub mod session;
mod stores;
mod wire;

pub use client::{Client, Reauth, Warmer, REQUEST_INTERVAL};
pub use domain::{
    Availability, Cart, CartLine, Category, Island, Price, Product, ProductDetail, ShippingOption,
    Store, StoreStock, VariationAxis, VariationValue, Wishlist, WishlistItem,
};
pub use endpoints::{Endpoints, SITE};
pub use error::{Error, Result};
pub use http::{client_spec, client_spec_for, profile, EMULATION};
pub use listing::{Facet, Listing, Query, Sort, DEFAULT_SORT, FACETS, PAGE_SIZE, SORTS};
pub use product::{Action, Pdp};
pub use session::{Session, StoredSession};
pub use stores::{is_region, region, REGIONS};
