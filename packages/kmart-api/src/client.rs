//! One client over four surfaces.
//!
//! The surfaces do not need the same things, and the client's job is mostly to
//! keep that straight:
//!
//! | | needs a token | needs cookies |
//! |---|---|---|
//! | catalogue (Constructor) | no | no |
//! | `postcode`, `stock`, `stores` | no | **yes** |
//! | cart, wishlist, orders, customer | **yes** | **yes** |
//!
//! So a failure is diagnosed before it is reported. A challenge is not a
//! lapsed session, a lapsed session is not a missing one, and a signed-out
//! `me` query answers `200` with `null` rather than an error -- three ways to
//! fail that would otherwise all read as "something went wrong".
//!
//! Renewal is cheap here, unlike the Woolworths client this is modelled on:
//! `offline_access` means a lapsed token is one POST away from a fresh one,
//! with no password and no forms. The password is only needed when the refresh
//! token itself is refused.

use std::sync::Mutex;

use net_kit::wreq;
use serde::de::DeserializeOwned;

use crate::catalog::{self, Query};
use crate::country::Country;
use crate::domain::{
    Availability, Cart, CartLine, Category, Customer, Listing, Order, OrderPage, Postcode, Price,
    Product, Store, StoreStock, Tracking, TradingDay, Wishlist, WishlistItem,
};
use crate::endpoints::Endpoints;
use crate::error::{is_challenge_body, Error, Result};
use crate::session::{Session, StoredSession, Tokens};
use crate::{gql, wire};

/// What the client needs to sign in again from nothing.
///
/// Only reached when the refresh token is gone or refused -- which is why the
/// password source is lazy: most runs never ask it for anything.
pub struct Reauth {
    pub email: String,
    pub password: net_kit::password::Source,
}

/// Where a renewed session is filed.
///
/// Deliberately not part of [`Reauth`]: filing a renewal and being able to
/// sign in again are unrelated, and tying them together is a bug rather than a
/// simplification. Auth0 rotates the refresh token on use, so a renewal that
/// is only held in memory leaves the *next* command replaying a token that has
/// already been spent -- one command works, and every one after it is refused.
pub struct SessionStore {
    pub secrets: net_kit::Secrets,
    /// Carried through so a save does not drop the account's name, which is
    /// all `auth status` has to show for who is signed in.
    pub email: Option<String>,
    /// Likewise the storefront that signed in, which is what a later renewal
    /// needs to pick the right Auth0 application.
    pub auth_country: Option<Country>,
}

pub struct Client {
    http: wreq::Client,
    endpoints: Endpoints,
    country: Country,
    /// The Constructor visitor id. Stable across runs so the index sees a
    /// returning visitor rather than a new one every time.
    client_id: String,
    /// Replaced in place by [`Client::renew`], so one command's later calls
    /// use the token its earlier ones bought.
    session: Mutex<Session>,
    store: Option<SessionStore>,
    reauth: Option<Reauth>,
}

impl Client {
    pub fn new(
        http: wreq::Client,
        endpoints: Endpoints,
        country: Country,
        session: Session,
        client_id: impl Into<String>,
    ) -> Client {
        Client {
            http,
            endpoints,
            country,
            client_id: client_id.into(),
            session: Mutex::new(session),
            store: None,
            reauth: None,
        }
    }

    /// Where to file a session this client renews.
    ///
    /// Worth setting even when there is no password to fall back on: renewal
    /// works without one, and it is the saving that keeps it working.
    pub fn with_session_store(mut self, store: Option<SessionStore>) -> Client {
        self.store = store;
        self
    }

    /// Let this client renew by itself when the token it holds has lapsed.
    pub fn with_reauth(mut self, reauth: Option<Reauth>) -> Client {
        self.reauth = reauth;
        self
    }

    pub fn endpoints(&self) -> &Endpoints {
        &self.endpoints
    }

    pub fn country(&self) -> Country {
        self.country
    }

    pub fn session(&self) -> Session {
        self.session.lock().expect("session lock").clone()
    }

    // ----------------------------------------------------------- the catalogue

    /// Products for a query. Needs no credentials of any kind.
    pub async fn listing(&self, query: &Query) -> Result<Listing> {
        let url = query.url(&self.endpoints, &self.client_id);
        let body = self.get(&url).await?;
        catalog::listing(&body)
    }

    pub async fn search(&self, term: &str, page: u32, per_page: u32) -> Result<Listing> {
        self.listing(&Query::term(term).page(page).per_page(per_page))
            .await
    }

    pub async fn browse(&self, group_id: &str, page: u32, per_page: u32) -> Result<Listing> {
        self.listing(&Query::group(group_id).page(page).per_page(per_page))
            .await
    }

    /// One product, by keycode.
    ///
    /// A search rather than a lookup, because the index has no item endpoint
    /// -- and because a keycode is a term it matches exactly, the result is
    /// checked rather than trusted: a keycode that has been retired matches
    /// nothing exactly and the index answers with something *similar*, which
    /// would otherwise be returned as though it were the product asked for.
    pub async fn product(&self, keycode: &str) -> Result<Product> {
        let listing = self.listing(&Query::term(keycode).per_page(5)).await?;
        listing
            .products
            .into_iter()
            .find(|p| p.keycode == keycode || p.variations.iter().any(|v| v.keycode == keycode))
            .ok_or_else(|| Error::NoSuchProduct(keycode.to_string()))
    }

    /// The category tree, to a depth.
    pub async fn categories(&self, depth: u32) -> Result<Vec<Category>> {
        let url = catalog::tree_url(&self.endpoints, &self.client_id, depth);
        let body = self.get(&url).await?;
        catalog::categories(&body)
    }

    // -------------------------------------------------------------- open gateway

    /// What the gateway knows about a postcode.
    ///
    /// Also the cheapest proof that the gateway is reachable and unchallenged,
    /// which is what makes it the right check for `doctor`.
    pub async fn postcodes(&self, query: &str) -> Result<Vec<Postcode>> {
        let data: wire::PostcodeData = self
            .gql(
                "getPostcodeSuggestions",
                gql::POSTCODE_SUGGESTIONS,
                serde_json::json!({"input": {"query": query, "country": self.country.code()}}),
                Auth::None,
            )
            .await?;
        Ok(data
            .suggestions
            .into_iter()
            .filter_map(|s| {
                Some(Postcode {
                    postcode: s.postcode?,
                    suburb: s.suburb,
                    state: s.state,
                    metro: s.is_metro_region_for_free_shipping,
                })
            })
            .collect())
    }

    /// Where a product can be had, near a postcode.
    pub async fn availability(
        &self,
        keycode: &str,
        postcode: &str,
        national_inventory: bool,
    ) -> Result<Availability> {
        let input = serde_json::json!({
            "country": self.country.code(),
            "postcode": postcode,
            "products": [{
                "keycode": keycode,
                "quantity": 1,
                "isNationalInventory": national_inventory,
                "isClickAndCollectOnly": false,
            }],
            "fulfilmentMethods": ["HOME_DELIVERY", "CLICK_AND_COLLECT", "EXPRESS_DELIVERY"],
        });
        let data: wire::AvailabilityData = self
            .gql(
                "getProductAvailability",
                gql::PRODUCT_AVAILABILITY,
                serde_json::json!({ "input": input }),
                Auth::None,
            )
            .await?;
        let result = data
            .result
            .ok_or_else(|| Error::Shape("the gateway returned no availability".into()))?;
        let f = result.availability.unwrap_or_default();

        let hd = f.home_delivery.unwrap_or_default();
        Ok(Availability {
            keycode: keycode.to_string(),
            postcode: result.postcode.unwrap_or_else(|| postcode.to_string()),
            region: result.region,
            home_delivery: hd.first().and_then(|c| c.stock.as_ref()?.available),
            pool: hd.first().and_then(|c| c.pool_name.clone()),
            express: f
                .express
                .unwrap_or_default()
                .first()
                .and_then(|c| c.stock.as_ref()?.available),
            click_and_collect: f
                .click_and_collect
                .as_ref()
                .and_then(|c| c.first())
                .and_then(|c| c.stock.as_ref()?.total_available),
            stores: store_stock(f.click_and_collect.as_deref()),
            in_store: store_stock(f.in_store.as_deref()),
        })
    }

    /// One store, by id.
    pub async fn store(&self, id: &str) -> Result<Store> {
        let data: wire::LocationData = self
            .gql(
                "getLocationDetail",
                gql::LOCATION_DETAIL,
                serde_json::json!({"input": {"locationId": id}}),
                Auth::None,
            )
            .await?;
        let d = data
            .location
            .ok_or_else(|| Error::NoSuchStore(id.to_string()))?;
        Ok(Store {
            id: id.to_string(),
            // Names arrive padded -- `"Sylvia Park Nz "` -- which shows up in
            // every table that prints one.
            name: d.public_name.unwrap_or_default().trim().to_string(),
            phone: d.phone_number,
            address: [d.address1, d.address2, d.address3]
                .into_iter()
                .flatten()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect(),
            city: d.city,
            state: d.state,
            postcode: d.postcode,
            latitude: d.latitude,
            longitude: d.longitude,
            distance_km: None,
            hours: d
                .trading_hours
                .into_iter()
                .filter_map(|h| {
                    Some(TradingDay {
                        day: h.week_day?,
                        hours: h.hours.unwrap_or_default(),
                    })
                })
                .collect(),
        })
    }

    /// The stores that serve a postcode, nearest first.
    ///
    /// There is no store-finder endpoint. What there is, is an availability
    /// query: asking about *any* keycode near a postcode comes back with the
    /// collect network for it, ordered by distance. So this asks about one and
    /// throws the stock away -- which is why `limit` matters, since naming
    /// each store is a request of its own.
    pub async fn stores_near(&self, postcode: &str, limit: usize) -> Result<Vec<Store>> {
        // A sentinel keycode: the site's own probe for exactly this, and it
        // keeps the answer about geography rather than about a real product
        // that might be ranged oddly.
        let availability = self.availability("9999", postcode, true).await?;
        let mut out = Vec::new();
        for stock in availability.stores.iter().take(limit) {
            let mut store = self.store(&stock.store_id).await?;
            store.distance_km = stock.distance_km;
            out.push(store);
        }
        Ok(out)
    }

    // ------------------------------------------------------------ account half

    pub async fn customer(&self) -> Result<Customer> {
        let data: wire::MeData<wire::CustomerData> = self
            .gql(
                "getDetails",
                gql::CUSTOMER,
                serde_json::json!({}),
                Auth::Required,
            )
            .await?;
        let c = data.me.and_then(|m| m.customer).ok_or(Error::NotSignedIn)?;
        Ok(Customer {
            id: c.id,
            email: c.email,
            name: match (c.first_name, c.last_name) {
                (Some(f), Some(l)) => Some(format!("{f} {l}").trim().to_string()),
                (Some(n), None) | (None, Some(n)) => Some(n),
                (None, None) => None,
            },
        })
    }

    pub async fn cart(&self) -> Result<Option<Cart>> {
        let data: wire::MeData<wire::ActiveCart> = self
            .gql(
                "getMyActiveCart",
                gql::ACTIVE_CART,
                serde_json::json!({}),
                Auth::Required,
            )
            .await?;
        // `me` present with a null cart is a signed-in shopper who has not
        // started one. `me` itself null is not signed in, and `gql` has
        // already turned that into an error.
        Ok(data.me.and_then(|m| m.active_cart).map(cart))
    }

    /// The cart, starting one if there is none.
    async fn cart_or_start(&self, postcode: Option<&str>) -> Result<Cart> {
        if let Some(cart) = self.cart().await? {
            return Ok(cart);
        }
        let draft = serde_json::json!({
            "currency": self.country.currency(),
            "country": self.country.code(),
            "shippingAddress": {"country": self.country.code()},
            "postcodeSelector": serde_json::to_string(&serde_json::json!({
                "postalCode": postcode.unwrap_or_default(),
                "country": self.country.code(),
            })).unwrap_or_default(),
        });
        let data: wire::UpdateCartData = self
            .gql(
                "createMyBag",
                gql::CREATE_CART,
                serde_json::json!({ "draft": draft }),
                Auth::Required,
            )
            .await?;
        data.created
            .map(cart)
            .ok_or_else(|| Error::Shape("the gateway created no cart".into()))
    }

    /// Apply actions to the cart, at the version just read.
    async fn update_cart(
        &self,
        id: &str,
        version: i64,
        actions: serde_json::Value,
    ) -> Result<Cart> {
        let data: wire::UpdateCartData = self
            .gql(
                "updateMyBag",
                gql::UPDATE_CART,
                serde_json::json!({"id": id, "version": version, "actions": actions}),
                Auth::Required,
            )
            .await?;
        data.cart
            .map(cart)
            .ok_or_else(|| Error::Shape("the gateway returned no cart".into()))
    }

    pub async fn cart_add(
        &self,
        keycode: &str,
        quantity: i64,
        postcode: Option<&str>,
    ) -> Result<Cart> {
        let current = self.cart_or_start(postcode).await?;
        self.update_cart(
            &current.id,
            current.version,
            serde_json::json!([{
                "addLineItem": {"sku": keycode, "quantity": quantity, "addToCartSource": "PLP"}
            }]),
        )
        .await
    }

    /// Read the cart, find the line holding a keycode, and act on it.
    ///
    /// The read is not avoidable: every action addresses a line by an id the
    /// cart assigns, and the write carries the version that read returned.
    async fn line_action(
        &self,
        keycode: &str,
        action: impl FnOnce(&str) -> serde_json::Value,
    ) -> Result<Cart> {
        let current = self
            .cart()
            .await?
            .ok_or_else(|| Error::NoSuchProduct(keycode.into()))?;
        let line = current
            .line(keycode)
            .ok_or_else(|| Error::NoSuchProduct(keycode.to_string()))?;
        self.update_cart(
            &current.id,
            current.version,
            serde_json::json!([action(&line.id)]),
        )
        .await
    }

    /// Set a line to an exact quantity. Zero removes it.
    pub async fn cart_set(&self, keycode: &str, quantity: i64) -> Result<Cart> {
        // Not by asking for zero of it: the gateway validates the quantity as
        // a positive number and answers `VALIDATION_ERROR`, so "none of this"
        // has to be spelled as a removal.
        if quantity < 1 {
            return self.cart_remove(keycode).await;
        }
        self.line_action(keycode, |id| {
            serde_json::json!({
                "changeLineItemQuantity": {"lineItemId": id, "quantity": quantity}
            })
        })
        .await
    }

    pub async fn cart_remove(&self, keycode: &str) -> Result<Cart> {
        self.line_action(
            keycode,
            |id| serde_json::json!({"removeLineItem": {"lineItemId": id}}),
        )
        .await
    }

    pub async fn wishlist(&self) -> Result<Wishlist> {
        let data: wire::MeData<wire::WishlistData> = self
            .gql(
                "GetWishListItems",
                gql::WISHLIST,
                serde_json::json!({"countryCode": self.country.code()}),
                Auth::Required,
            )
            .await?;
        Ok(wishlist(data.me.and_then(|m| m.list)))
    }

    /// Save a product.
    ///
    /// The counterpart -- taking one off again -- is deliberately absent: the
    /// site's own removal was never captured, introspection is disabled on the
    /// gateway, and an invented operation name would fail at runtime rather
    /// than here. See this crate's README.
    pub async fn wishlist_add(&self, keycode: &str, quantity: i64) -> Result<Wishlist> {
        let existing = self.wishlist().await?;
        let data: wire::WishlistAddData = self
            .gql(
                "createMyShoppingList",
                gql::WISHLIST_ADD,
                serde_json::json!({
                    "sku": keycode,
                    "quantity": quantity,
                    "version": existing.version,
                    "offerId": serde_json::Value::Null,
                }),
                Auth::Required,
            )
            .await?;
        Ok(wishlist(data.list))
    }

    pub async fn orders(&self, limit: u32, after: Option<&str>) -> Result<OrderPage> {
        let data: wire::OrdersData = self
            .gql(
                "getOrdersForCustomer",
                gql::ORDERS,
                serde_json::json!({"limit": limit, "startsAfter": after}),
                Auth::Required,
            )
            .await?;
        let page = data.page.ok_or(Error::NotSignedIn)?;
        Ok(OrderPage {
            total: page.count,
            next: page.starts_after,
            orders: page
                .orders
                .into_iter()
                .filter_map(|o| {
                    Some(Order {
                        id: o.display_order_id?,
                        // Dollars here, cents in the cart. Same gateway.
                        total: o.order_total.map(Price::from_dollars),
                        status: o.order_status,
                        placed: o.order_date,
                        tracking: o
                            .shipped_items
                            .into_iter()
                            .map(|s| Tracking {
                                number: s.tracking_number,
                                carrier: s.carrier,
                                link: s.tracking_link,
                            })
                            .collect(),
                    })
                })
                .collect(),
        })
    }

    // ---------------------------------------------------------------- renewal

    /// Get a usable token, renewing if the one held has lapsed.
    ///
    /// Cheapest source first: the token in hand, then the refresh grant, then
    /// -- only if that is refused -- the whole login. Most runs stop at the
    /// first.
    pub async fn renew(&self) -> Result<Tokens> {
        let held = self.session().tokens().cloned();
        if let Some(tokens) = held.clone().filter(|t| !t.lapsed()) {
            return Ok(tokens);
        }

        if let Some(refresh) = held.as_ref().and_then(|t| t.refresh.clone()) {
            match crate::auth::refresh(&self.endpoints, &refresh, &crate::auth::no_trace).await {
                Ok(mut fresh) => {
                    // Auth0 may or may not rotate the refresh token. Keeping
                    // the old one when it does not is the difference between a
                    // session that renews forever and one that dies in fifteen
                    // minutes.
                    if fresh.refresh.is_none() {
                        fresh.refresh = Some(refresh);
                    }
                    self.keep(fresh.clone(), None)?;
                    return Ok(fresh);
                }
                // A refused refresh token is not fatal while a password is on
                // hand; without one it is the end of the road.
                Err(e) if self.reauth.is_none() => return Err(e),
                Err(_) => {}
            }
        }

        // The same dead end whether there are no credentials or a named
        // password turns out to have nothing behind it.
        let unrenewable = || match held {
            Some(_) => Error::SessionExpired,
            None => Error::NotSignedIn,
        };
        let reauth = self.reauth.as_ref().ok_or_else(unrenewable)?;
        let password = reauth.password.password().await?.ok_or_else(unrenewable)?;
        // Seed whatever admission this session holds: the auth host is under
        // `.kmart.com.au`, so the Australian bucket is the one that covers it.
        let admission = self.session().admission(Country::Au);
        let tokens = crate::auth::login(
            &self.endpoints,
            &reauth.email,
            &password,
            &admission,
            &crate::auth::no_trace,
        )
        .await?;
        self.keep(tokens.clone(), Some(reauth.email.clone()))?;
        Ok(tokens)
    }

    /// Hold new tokens in memory and on disk.
    ///
    /// On disk whenever there is a store, whatever the tokens came from: the
    /// refresh grant rotates, so the copy in memory is worth less than the one
    /// that outlives the process.
    fn keep(&self, tokens: Tokens, email: Option<String>) -> Result<()> {
        let mut session = self.session.lock().expect("session lock");
        *session = session.clone().with_tokens(tokens);
        let Some(store) = &self.store else {
            return Ok(());
        };
        let email = email.or_else(|| store.email.clone());
        StoredSession::of(&session, email)
            .with_auth_country(store.auth_country)
            .save(&store.secrets)
    }

    // --------------------------------------------------------------- plumbing

    /// A plain GET, for the catalogue.
    async fn get(&self, url: &str) -> Result<String> {
        let res = self.http.get(url).send().await;
        let (_, body) = net_kit::http::text("GET", url, res).await?;
        Ok(body)
    }

    /// One GraphQL call.
    async fn gql<T: DeserializeOwned>(
        &self,
        operation: &'static str,
        document: &str,
        variables: serde_json::Value,
        auth: Auth,
    ) -> Result<T> {
        // Say what is missing before spending a request on finding out. The
        // two failures need opposite advice and the gateway distinguishes
        // neither.
        if !self.session().admitted(self.country) {
            return Err(Error::Challenged {
                host: host_of(&self.endpoints.api),
            });
        }
        if auth == Auth::Required {
            self.renew().await?;
        }

        let url = self.endpoints.graphql();
        let payload = serde_json::json!({
            "operationName": operation,
            "variables": variables,
            "query": document,
        });
        let mut req = self
            .http
            .post(&url)
            .header(wreq::header::CONTENT_TYPE, "application/json")
            .header(wreq::header::ORIGIN, self.country.origin())
            .header(wreq::header::REFERER, format!("{}/", self.country.origin()))
            .header("x-country-code", self.country.code());
        let session = self.session();
        if let Some(cookie) = session.cookie_header(self.country) {
            req = req.header(wreq::header::COOKIE, cookie);
        }
        if auth == Auth::Required {
            if let Some(bearer) = session.bearer() {
                req = req.header(wreq::header::AUTHORIZATION, bearer);
            }
        }

        let res = req
            .body(payload.to_string())
            .send()
            .await
            .map_err(|source| net_kit::HttpError::Transport {
                method: "POST",
                url: url.clone(),
                source,
            })?;
        let status = res.status();
        let headers = res.headers().clone();
        let body = res.text().await.unwrap_or_default();

        // Akamai answers a challenge as 429 or 403 with a JSON marker, which
        // is neither rate limiting nor a permission problem and must not be
        // retried as either.
        if is_challenge_body(&body) {
            return Err(Error::Challenged {
                host: host_of(&self.endpoints.api),
            });
        }
        if status.as_u16() == 429 {
            return Err(Error::RateLimited {
                retry_after: headers
                    .get(wreq::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse().ok()),
            });
        }
        if status.as_u16() == 403 && body.trim().is_empty() {
            return Err(Error::Challenged {
                host: host_of(&self.endpoints.api),
            });
        }

        self.session
            .lock()
            .expect("session lock")
            .absorb(self.country, &headers);

        let envelope: wire::GqlEnvelope<T> = serde_json::from_str(&body).map_err(|e| {
            if !status.is_success() {
                return Error::Http(net_kit::HttpError::Status {
                    method: "POST",
                    url: url.clone(),
                    status: status.as_u16(),
                    detail: net_kit::error::truncate(&body, 300),
                    body: body.clone(),
                });
            }
            Error::decode(format!("reading the answer to {operation}"), e)
        })?;

        if let Some(error) = envelope.errors.first() {
            let message = error.message.clone().unwrap_or_default();
            return Err(match error.code() {
                Some("UNAUTHENTICATED") => Error::SessionExpired,
                Some("ConcurrentModification") => Error::CartConflict,
                _ if message.contains("version") && message.contains("mismatch") => {
                    Error::CartConflict
                }
                _ => Error::Graphql { operation, message },
            });
        }

        envelope.data.ok_or_else(|| Error::Graphql {
            operation,
            message: "the gateway answered with neither data nor an error".into(),
        })
    }
}

/// Whether a call needs an account behind it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Auth {
    None,
    Required,
}

fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| url.to_string())
}

/// The per-store rows out of a collect-shaped channel.
fn store_stock(channel: Option<&[wire::CncStock]>) -> Vec<StoreStock> {
    channel
        .and_then(|c| c.first())
        .map(|c| {
            c.locations
                .iter()
                .filter_map(|l| {
                    let f = l.fulfilment.as_ref();
                    // Collect names the store on `fulfilment`; in-store only
                    // on `location`. Either is the same id.
                    let store_id = f
                        .and_then(|f| f.location_id.clone())
                        .or_else(|| l.location.as_ref()?.location_id.clone())?;
                    Some(StoreStock {
                        store_id,
                        name: None,
                        available: f
                            .and_then(|f| f.stock.as_ref())
                            .and_then(|s| s.available)
                            .unwrap_or(0),
                        distance_km: l.distance_in_km,
                        buddy: f.and_then(|f| f.is_buddy_location).unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The variant attributes worth showing.
///
/// The gateway attaches the whole merchandising record to a line item -- tax
/// rates, dead weight, image keys, `ProductDimensionsWithWeight` as a JSON
/// string -- and there is no flag separating the presentational few from the
/// rest. So they are named.
const VARIANT_OPTIONS: [&str; 2] = ["Size", "Colour"];

fn cart(body: wire::CartBody) -> Cart {
    Cart {
        id: body.id.unwrap_or_default(),
        version: body.version.unwrap_or_default(),
        shipping: body
            .shipping_info
            .as_ref()
            .and_then(|s| s.shipping_rate.as_ref())
            .and_then(|r| r.price.as_ref())
            .and_then(|m| m.cent_amount)
            .map(Price::cents),
        shipping_method: body
            .shipping_info
            .as_ref()
            .and_then(|s| s.shipping_method_name.clone()),
        total: body
            .total_price
            .and_then(|m| m.cent_amount)
            .map(Price::cents),
        collect_store_id: body.selected_cnc_store_id,
        lines: body
            .line_items
            .into_iter()
            .filter_map(|l| {
                let variant = l.variant?;
                Some(CartLine {
                    id: l.id?,
                    keycode: variant.sku.unwrap_or_default(),
                    name: l.name.unwrap_or_default(),
                    quantity: l.quantity.unwrap_or(0),
                    unit_price: l
                        .price
                        .and_then(|p| p.value)
                        .and_then(|m| m.cent_amount)
                        .map(Price::cents),
                    total: l.total_price.and_then(|m| m.cent_amount).map(Price::cents),
                    seller: l.custom.and_then(|c| c.fields).and_then(|f| f.seller_name),
                    options: variant
                        .attributes
                        .iter()
                        .filter(|a| {
                            a.name
                                .as_deref()
                                .is_some_and(|n| VARIANT_OPTIONS.contains(&n))
                        })
                        .filter_map(|a| {
                            let name = a.name.clone()?;
                            let value = a.text()?;
                            (!value.trim().is_empty()).then_some((name, value))
                        })
                        .collect(),
                })
            })
            .collect(),
    }
}

fn wishlist(body: Option<wire::WishlistBody>) -> Wishlist {
    let Some(body) = body else {
        // A signed-in shopper who has never saved anything has no list at all,
        // which is an empty wishlist rather than a missing one.
        return Wishlist {
            id: None,
            version: None,
            items: Vec::new(),
        };
    };
    Wishlist {
        id: body.id,
        version: body.version,
        items: body
            .line_items
            .into_iter()
            .filter_map(|l| {
                let variant = l.variant?;
                Some(WishlistItem {
                    id: l.id?,
                    keycode: variant.sku.unwrap_or_default(),
                    name: l.name,
                    quantity: l.quantity.unwrap_or(1),
                    price: variant
                        .shopping_list_price
                        .and_then(|m| m.cent_amount)
                        .map(Price::cents),
                })
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("tests/fixtures/{name}")).unwrap()
    }

    fn availability_of(name: &str) -> wire::AvailabilityResult {
        let env: wire::GqlEnvelope<wire::AvailabilityData> =
            serde_json::from_str(&fixture(name)).unwrap();
        env.data.unwrap().result.unwrap()
    }

    #[test]
    fn collect_rows_carry_the_store_the_distance_and_the_count() {
        let result = availability_of("stock-all-channels.json");
        let f = result.availability.as_ref().unwrap();
        let rows = store_stock(f.click_and_collect.as_deref());
        assert!(!rows.is_empty());
        let first = &rows[0];
        assert!(!first.store_id.is_empty());
        assert!(first.distance_km.is_some(), "the only ordering on offer");
        // Nearest first, as the gateway returned them.
        let distances: Vec<f64> = rows.iter().filter_map(|r| r.distance_km).collect();
        assert!(
            distances.windows(2).all(|w| w[0] <= w[1]),
            "not sorted: {distances:?}"
        );
    }

    #[test]
    fn a_keycode_that_does_not_exist_reads_as_zero_rather_than_an_error() {
        // The gateway answers `NO POOL FOUND` and zeroes rather than failing,
        // so nothing downstream can treat a 200 as proof the product is real.
        let env: wire::GqlEnvelope<wire::AvailabilityData> =
            serde_json::from_str(&fixture("stock-none.json")).unwrap();
        let f = env.data.unwrap().result.unwrap().availability.unwrap();
        let hd = f.home_delivery.unwrap();
        assert_eq!(hd[0].stock.as_ref().unwrap().available, Some(0));
        assert_eq!(hd[0].pool_name.as_deref(), Some("NO POOL FOUND"));
    }

    #[test]
    fn an_empty_channel_and_a_missing_one_both_read_as_nothing() {
        // IN_STORE comes back null and EXPRESS_DELIVERY as [], for the same
        // product, in the same answer.
        let f = availability_of("stock-all-channels.json")
            .availability
            .unwrap();
        assert!(f.in_store.is_none());
        assert_eq!(f.express.as_ref().map(Vec::len), Some(0));
        assert!(store_stock(f.in_store.as_deref()).is_empty());
    }

    #[test]
    fn a_signed_out_cart_is_absent_rather_than_an_error() {
        let env: wire::GqlEnvelope<wire::MeData<wire::ActiveCart>> =
            serde_json::from_str(&fixture("cart-anonymous.json")).unwrap();
        assert!(env.errors.is_empty(), "the gateway called this a success");
        assert!(env.data.unwrap().me.unwrap().active_cart.is_none());
    }

    #[test]
    fn a_signed_out_wishlist_is_absent_rather_than_an_error() {
        let env: wire::GqlEnvelope<wire::MeData<wire::WishlistData>> =
            serde_json::from_str(&fixture("unauthenticated.json")).unwrap();
        assert!(env.errors.is_empty());
        let list = env.data.unwrap().me.unwrap().list;
        assert!(list.is_none());
        // Which is an empty list, not a broken one.
        assert!(wishlist(list).items.is_empty());
    }

    #[test]
    fn a_stores_coordinates_arrive_quoted_and_still_parse() {
        // Latitude is the string "-36.914257999999997" while distanceInKm in
        // the same gateway is a bare number. Typing either as f64 alone fails
        // on the other.
        let env: wire::GqlEnvelope<wire::LocationData> =
            serde_json::from_str(&fixture("store.json")).unwrap();
        let d = env.data.unwrap().location.unwrap();
        assert!(d.latitude.is_some_and(|v| v < 0.0), "{:?}", d.latitude);
        assert!(d.longitude.is_some_and(|v| v > 0.0));
        // And the gateway pads its names: "Sylvia Park Nz ".
        assert!(d.public_name.as_deref().unwrap().ends_with(' '));
        assert!(!d.trading_hours.is_empty());
    }

    #[test]
    fn postcodes_decode_with_the_subdivision_the_island_filter_wants() {
        let env: wire::GqlEnvelope<wire::PostcodeData> =
            serde_json::from_str(&fixture("postcode.json")).unwrap();
        let s = &env.data.unwrap().suggestions[0];
        assert_eq!(s.postcode.as_deref(), Some("1010"));
        assert_eq!(s.state.as_deref(), Some("NI"), "feeds the catalogue filter");
        assert!(s.suburb.is_some());
    }

    #[test]
    fn a_graphql_error_code_is_read_off_the_extensions() {
        let env: wire::GqlEnvelope<serde_json::Value> = serde_json::from_str(
            r#"{"errors":[{"message":"nope","extensions":{"code":"UNAUTHENTICATED"}}]}"#,
        )
        .unwrap();
        assert_eq!(env.errors[0].code(), Some("UNAUTHENTICATED"));
        assert!(env.data.is_none());
    }
}
