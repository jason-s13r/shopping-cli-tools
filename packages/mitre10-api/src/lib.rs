//! Mitre 10 New Zealand.
//!
//! The storefront is SAP Commerce Cloud (Hybris) with the OCC v2 REST API on
//! basesite `mitre10`, so most of this crate is conventional. Two things are
//! not, and they shape the module split:
//!
//! * **Search and browse are Algolia, not OCC.** The catalogue endpoints
//!   answer one product at a time; every grid on the site is an Algolia query.
//!   See [`search`].
//! * **The category tree lives in the CMS, not in `/categories`.** OCC's
//!   `/categories/{code}` returns one category and its breadcrumbs but no
//!   children. The tree is a navigation component inside a page response. See
//!   [`catalogue`].
//!
//! Nothing here needs credentials except the account surface: search, browse,
//! product, per-store stock, stores and the whole cart are anonymous.

/// This crate's own version, for a consumer that reports what it was
/// built against.
///
/// `env!` expands where it is written, so this is the one place it can be
/// read from: a consumer writing `env!("CARGO_PKG_VERSION")` would get its
/// own version back, not this one.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod auth;
pub mod catalogue;
mod client;
mod domain;
mod endpoints;
mod error;
pub mod http;
pub mod search;
pub mod session;
pub mod wire;

pub use client::{Client, SessionStore};
pub use domain::{
    Address, Cart, CartChange, CartLine, Category, CategoryNode, Customer, Delivery, Facet,
    FacetOption, Fulfilment, Listing, Money, Order, OrderLine, OrderPage, Product, ProductDetail,
    Promotion, Stock, StockLevel, Store, Variant, Wishlist, WishlistItem,
};
pub use endpoints::{Algolia, Endpoints, ALGOLIA_CONFIG_KEY, BASE_SITE};
pub use error::{Error, Result};
pub use http::{client_spec, client_spec_for, profile, EMULATION};
pub use search::{Availability, Query, Sort, PAGE_SIZE, SORTS};
pub use session::{Session, StoredSession};
