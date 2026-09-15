//! One client over two surfaces.
//!
//! | | needs a token |
//! |---|---|
//! | Algolia (search, browse, suggestions) | no |
//! | OCC catalogue, stores, stock, cart | no |
//! | account, orders, wishlist | **yes** |
//!
//! Almost everything is anonymous, including the whole cart: a cart can be
//! built, priced and switched between collection and delivery without an
//! account. Only what is filed against a person needs a bearer.
//!
//! Renewal is cheap and silent: a lapsed access token is one unguarded call
//! away from a fresh one with no password. A refused *refresh* token is
//! reported rather than worked around -- see [`crate::auth`].

use std::sync::Mutex;

use net_kit::wreq;

use crate::catalogue;
use crate::domain::{
    Cart, CartChange, Category, CategoryNode, Customer, Fulfilment, Listing, OrderPage,
    ProductDetail, Stock, Store, Wishlist,
};
use crate::endpoints::{Algolia, Endpoints, ALGOLIA_CONFIG_KEY};
use crate::error::{Error, Result};
use crate::search::{Query, Suggest};
use crate::session::Session;
use crate::{auth, wire};

/// How many product codes `priceForProducts` is asked for at once. The site
/// sends 18; the limit above that is unknown, so it is not pushed.
pub const BATCH: usize = 18;

/// The CMS component batch size the site uses.
const COMPONENT_BATCH: usize = 50;

/// Where a renewed session is filed.
///
/// Kept apart from the credentials themselves: a client that can renew and one
/// that can *remember* it renewed are different capabilities, and a renewal
/// held only in memory means the next command starts from a token it already
/// knows is stale.
pub struct SessionStore {
    pub secrets: net_kit::Secrets,
}

pub struct Client {
    http: wreq::Client,
    endpoints: Endpoints,
    /// Replaced in place by [`Client::renew`], so one command's later calls
    /// use the token its earlier ones bought.
    session: Mutex<Session>,
    /// The Algolia application, once asked for. See [`Client::algolia`].
    algolia: Mutex<Option<Algolia>>,
    store: Option<SessionStore>,
    debug: bool,
}

impl Client {
    pub fn new(http: wreq::Client, endpoints: Endpoints, session: Session) -> Client {
        Client {
            http,
            algolia: Mutex::new(endpoints.algolia.clone()),
            endpoints,
            session: Mutex::new(session),
            store: None,
            debug: false,
        }
    }

    pub fn with_session_store(mut self, store: Option<SessionStore>) -> Client {
        self.store = store;
        self
    }

    pub fn with_debug(mut self, debug: bool) -> Client {
        self.debug = debug;
        self
    }

    pub fn endpoints(&self) -> &Endpoints {
        &self.endpoints
    }

    pub fn session(&self) -> Session {
        self.session.lock().expect("the session lock").clone()
    }

    pub fn is_signed_in(&self) -> bool {
        let session = self.session.lock().expect("the session lock");
        session.access_token.is_some() || session.can_refresh()
    }

    // ------------------------------------------------------------- search

    /// The Algolia application the storefront names, fetched once and kept.
    ///
    /// Not a constant, because the id and its search-only key belong to Mitre
    /// 10 rather than to this crate; the storefront reads them from the same
    /// property at boot. One extra round trip before the first query, none
    /// after, and a rotated key needs no release here.
    pub async fn algolia(&self) -> Result<Algolia> {
        if let Some(algolia) = self.algolia.lock().expect("the algolia lock").clone() {
            return Ok(algolia);
        }
        // Not `occ_send`: that renews a lapsed token before spending the
        // request, which would make search -- an anonymous surface the
        // storefront reads this property for before anyone signs in -- fail
        // on a stale stored login.
        let url = self.url("/config/key", &[("key", ALGOLIA_CONFIG_KEY.to_string())]);
        self.trace("GET", &url);
        let body: serde_json::Value = net_kit::http::json(
            "GET",
            &url,
            self.http
                .get(&url)
                .headers(auth::site_headers(&self.endpoints))
                .send()
                .await,
        )
        .await?;
        let algolia = wire::algolia_config(&body)?;
        // Last writer wins: two racing callers fetched the same property.
        *self.algolia.lock().expect("the algolia lock") = Some(algolia.clone());
        Ok(algolia)
    }

    /// One page of search or browse results.
    pub async fn listing(&self, query: &Query) -> Result<Listing> {
        let url = self.endpoints.queries(&self.algolia().await?);
        self.trace("POST", "algolia/queries");
        let body: serde_json::Value = net_kit::http::json(
            "POST",
            &url,
            self.http
                .post(&url)
                .headers(auth::site_headers(&self.endpoints))
                .json(&query.body())
                .send()
                .await,
        )
        .await?;
        wire::listing(&body, query.term().map(str::to_string))
    }

    /// What the site would suggest while someone types.
    pub async fn suggestions(&self, term: &str, limit: u64) -> Result<Vec<String>> {
        let url = self.endpoints.queries(&self.algolia().await?);
        self.trace("POST", "algolia/suggestions");
        let body: serde_json::Value = net_kit::http::json(
            "POST",
            &url,
            self.http
                .post(&url)
                .headers(auth::site_headers(&self.endpoints))
                .json(&Suggest::new(term, limit).body())
                .send()
                .await,
        )
        .await?;
        Ok(wire::suggestions(&body))
    }

    // ---------------------------------------------------------- catalogue

    /// One product, priced for a shop when one is named.
    pub async fn product(&self, code: &str, store: Option<&str>) -> Result<ProductDetail> {
        let mut query = vec![("fields", "FULL".to_string())];
        if let Some(store) = store {
            query.push(("staticStoreCode", store.to_string()));
        }
        let body = self
            .occ_get("product", &format!("/products/{code}"), &query)
            .await
            .map_err(|e| match e {
                Error::Http(ref h) if h.status() == Some(400) || h.status() == Some(404) => {
                    Error::NoSuchProduct(code.to_string())
                }
                other => other,
            })?;
        wire::product_detail(&body)
    }

    /// Several products at once, in batches of [`BATCH`].
    ///
    /// The endpoint zero-pads codes to eighteen digits; callers pass them
    /// plain, as every other call here takes them.
    pub async fn products(
        &self,
        codes: &[String],
        store: Option<&str>,
        postcode: Option<&str>,
    ) -> Result<Vec<ProductDetail>> {
        let mut out = Vec::with_capacity(codes.len());
        for chunk in codes.chunks(BATCH) {
            let padded: Vec<String> = chunk.iter().map(|c| pad(c)).collect();
            let mut query = vec![
                ("fields", "DEFAULT".to_string()),
                ("productCodes", padded.join(",")),
            ];
            if let Some(store) = store {
                query.push(("staticStoreCode", store.to_string()));
            }
            if let Some(postcode) = postcode {
                query.push(("postcode", postcode.to_string()));
            }
            let body = self
                .occ_get("prices", "/products/priceForProducts", &query)
                .await?;
            out.extend(wire::product_batch(&body));
        }
        Ok(out)
    }

    /// Every shop's answer for one product, in one unauthenticated call.
    pub async fn stock(&self, code: &str) -> Result<Vec<Stock>> {
        let body = self
            .occ_get("stock", &format!("/products/{code}/stockIndicator"), &[])
            .await
            .map_err(|e| match e {
                Error::Http(ref h) if h.status() == Some(400) || h.status() == Some(404) => {
                    Error::NoSuchProduct(code.to_string())
                }
                other => other,
            })?;
        Ok(wire::stock(&body))
    }

    /// One category's own record: name, URL and ancestors, but not children.
    pub async fn category(&self, code: &str) -> Result<Category> {
        let body = self
            .occ_get(
                "category",
                &format!("/categories/{code}"),
                &[("fields", "FULL".to_string())],
            )
            .await
            .map_err(|e| match e {
                Error::Http(ref h) if h.status() == Some(400) || h.status() == Some(404) => {
                    Error::NoSuchCategory(code.to_string())
                }
                other => other,
            })?;
        wire::category(&body)
    }

    /// The whole category tree, from the CMS navigation component.
    ///
    /// Two round trips' worth: one page, then the link components in batches.
    /// The page is around a megabyte, so a caller listing categories often is
    /// better off caching this than calling it twice.
    pub async fn category_tree(&self) -> Result<CategoryNode> {
        let page = self
            .occ_get(
                "pages",
                &format!("/users/{}/cms/pages/homepage", self.user()),
                &[("fields", "FULL".to_string())],
            )
            .await?;
        let mut tree = catalogue::tree(&page).ok_or_else(|| {
            Error::Shape("the homepage carried no category navigation component".into())
        })?;

        let ids = catalogue::link_ids(&tree);
        for chunk in ids.chunks(COMPONENT_BATCH) {
            let components = self
                .occ_get(
                    "components",
                    &format!("/users/{}/cms/components", self.user()),
                    &[
                        ("fields", "DEFAULT".to_string()),
                        ("productCode", String::new()),
                        ("currentPage", "0".to_string()),
                        ("pageSize", COMPONENT_BATCH.to_string()),
                        ("componentIds", chunk.join(",")),
                    ],
                )
                .await?;
            catalogue::apply(&mut tree, &components);
        }
        Ok(tree)
    }

    // ------------------------------------------------------------- stores

    pub async fn stores_near(&self, latitude: f64, longitude: f64) -> Result<Vec<Store>> {
        let body = self
            .occ_get(
                "stores",
                "/geolocation/stores-for-location",
                &[
                    ("fields", "FULL".to_string()),
                    ("latitude", latitude.to_string()),
                    ("longitude", longitude.to_string()),
                ],
            )
            .await?;
        Ok(wire::stores(&body))
    }

    pub async fn stores_for_postcode(&self, postcode: &str) -> Result<Vec<Store>> {
        let body = self
            .occ_get(
                "stores",
                "/geolocation/stores-for-postcode",
                &[
                    ("fields", "FULL".to_string()),
                    ("postcode", postcode.to_string()),
                    ("currentPage", "0".to_string()),
                ],
            )
            .await?;
        Ok(wire::stores(&body))
    }

    /// One shop by its numeric code.
    pub async fn store(&self, code: &str) -> Result<Store> {
        let body = self
            .occ_get(
                "store",
                &format!("/stores/mystore/{code}"),
                &[("fields", "FULL".to_string())],
            )
            .await
            .map_err(|e| match e {
                Error::Http(ref h) if h.status() == Some(400) || h.status() == Some(404) => {
                    Error::NoSuchStore(code.to_string())
                }
                other => other,
            })?;
        Ok(wire::store(&body))
    }

    /// The delivery-pricing group a postcode falls in.
    ///
    /// Answers a bare number, not JSON. It is what the search index's
    /// `homeDelivery` filter matches on, so a delivery-filtered listing needs
    /// this first -- see [`crate::search::Availability`].
    pub async fn postcode_group(&self, postcode: &str) -> Result<String> {
        let url = self.url(
            "/c/postcodegroupid-postcode",
            &[("postCode", postcode.to_string())],
        );
        self.trace("GET", &url);
        let (_, body) = net_kit::http::text(
            "GET",
            &url,
            self.http
                .get(&url)
                .headers(auth::site_headers(&self.endpoints))
                .send()
                .await,
        )
        .await?;
        let group = body.trim().trim_matches('"').to_string();
        if group.is_empty() {
            return Err(Error::Shape(format!(
                "no delivery group for postcode {postcode}"
            )));
        }
        Ok(group)
    }

    // --------------------------------------------------------------- cart

    /// Start a cart. Anonymous unless this client holds a session.
    pub async fn create_cart(&self) -> Result<Cart> {
        let body = self
            .occ_send(
                "createCart",
                wreq::Method::POST,
                &format!("/users/{}/carts", self.user()),
                &[("fields", "FULL".to_string())],
                Some(serde_json::json!({})),
            )
            .await?;
        Ok(wire::cart(&body))
    }

    /// The identifier this client's cart paths take -- the guid while
    /// anonymous, the code once signed in. See [`Cart::id_for`].
    pub fn cart_id(&self, cart: &Cart) -> Option<String> {
        cart.id_for(self.user() == "current").map(str::to_string)
    }

    pub async fn cart(&self, id: &str) -> Result<Cart> {
        let body = self
            .occ_get(
                "cart",
                &format!("/users/{}/carts/{id}", self.user()),
                &[("fields", "FULL".to_string())],
            )
            .await?;
        Ok(wire::cart(&body))
    }

    /// Add a product. `store` is the numeric code it is collected from, which
    /// the storefront requires even for a delivery cart.
    pub async fn cart_add(
        &self,
        id: &str,
        code: &str,
        quantity: i64,
        store: &str,
        collect: bool,
    ) -> Result<CartChange> {
        let body = self
            .occ_send(
                "addToCart",
                wreq::Method::POST,
                &format!("/users/{}/carts/{id}/entries", self.user()),
                &[("fields", "FULL".to_string())],
                Some(serde_json::json!({
                    "quantity": quantity,
                    "product": { "code": code },
                    "storeId": store,
                    "giftCardAmount": 0,
                    "clickToCollect": collect,
                })),
            )
            .await?;
        self.change(id, &body).await
    }

    /// Change a line's quantity, by the entry number Hybris gave it.
    pub async fn cart_update(&self, id: &str, entry: i64, quantity: i64) -> Result<CartChange> {
        let body = self
            .occ_send(
                "updateCart",
                wreq::Method::PUT,
                &format!("/users/{}/carts/{id}/entries/{entry}", self.user()),
                &[("fields", "FULL".to_string())],
                Some(serde_json::json!({ "quantity": quantity })),
            )
            .await?;
        self.change(id, &body).await
    }

    /// The cart after a change, with whatever the storefront said about it.
    async fn change(&self, id: &str, body: &serde_json::Value) -> Result<CartChange> {
        let notice = wire::cart_notice(body);
        Ok(CartChange {
            cart: self.cart(id).await?,
            notice,
        })
    }

    pub async fn cart_remove(&self, id: &str, entry: i64) -> Result<CartChange> {
        let body = self
            .occ_send(
                "removeFromCart",
                wreq::Method::DELETE,
                &format!("/users/{}/carts/{id}/entries/{entry}", self.user()),
                &[],
                None,
            )
            .await?;
        self.change(id, &body).await
    }

    pub async fn cart_fulfilment(&self, id: &str, how: Fulfilment) -> Result<CartChange> {
        let body = self
            .occ_send(
                "changeDeliveryMode",
                wreq::Method::POST,
                &format!("/users/{}/carts/{id}/changeDeliveryMode", self.user()),
                &[("fields", "FULL".to_string())],
                Some(serde_json::json!({ "fulfillmentMethod": how.code() })),
            )
            .await?;
        self.change(id, &body).await
    }

    pub async fn cart_voucher(&self, id: &str, code: &str) -> Result<CartChange> {
        let body = self
            .occ_send(
                "applyVoucher",
                wreq::Method::POST,
                &format!("/users/{}/carts/{id}/vouchers", self.user()),
                &[("voucherId", code.to_string())],
                Some(serde_json::json!({ "voucherId": code })),
            )
            .await?;
        self.change(id, &body).await
    }

    // ------------------------------------------------------------ account

    pub async fn customer(&self) -> Result<Customer> {
        let body = self
            .occ_authed(
                "customer",
                "/users/current",
                &[("fields", "FULL".to_string())],
            )
            .await?;
        Ok(wire::customer(&body))
    }

    pub async fn orders(&self, page: u64, page_size: u64) -> Result<OrderPage> {
        let body = self
            .occ_authed(
                "orders",
                "/users/current/orders",
                &[
                    ("fields", "FULL".to_string()),
                    ("currentPage", page.to_string()),
                    ("pageSize", page_size.to_string()),
                ],
            )
            .await?;
        Ok(wire::orders(&body))
    }

    pub async fn wishlist(&self) -> Result<Wishlist> {
        let body = self
            .occ_authed("wishlist", "/wishlist", &[("fields", "FULL".to_string())])
            .await?;
        Ok(wire::wishlist(&body))
    }

    pub async fn wishlist_add(&self, code: &str) -> Result<()> {
        self.require_session()?;
        self.occ_send(
            "wishlistAdd",
            wreq::Method::POST,
            &format!("/wishlist/add/{code}"),
            &[],
            Some(serde_json::json!({})),
        )
        .await?;
        Ok(())
    }

    pub async fn wishlist_clear(&self) -> Result<()> {
        self.require_session()?;
        self.occ_send(
            "wishlistClear",
            wreq::Method::DELETE,
            "/wishlist/clear",
            &[],
            None,
        )
        .await?;
        Ok(())
    }

    // ---------------------------------------------------------------- auth

    /// Mint a fresh access token from the refresh token.
    pub async fn renew(&self) -> Result<()> {
        let refresh_token = {
            let session = self.session.lock().expect("the session lock");
            session.refresh_token.clone().ok_or(Error::NotSignedIn)?
        };
        let tokens = auth::refresh(
            &self.http,
            &self.endpoints,
            &refresh_token,
            &|step, detail| self.trace(step, detail),
        )
        .await?;
        let session = {
            let mut session = self.session.lock().expect("the session lock");
            tokens.apply(&mut session);
            session.clone()
        };
        self.file(session);
        Ok(())
    }

    /// Adopt tokens a sign-in just earned.
    pub fn adopt(&self, tokens: auth::Tokens) {
        let session = {
            let mut session = self.session.lock().expect("the session lock");
            tokens.apply(&mut session);
            session.clone()
        };
        self.file(session);
    }

    /// Give the tokens back and forget them. Best-effort at the network end:
    /// a sign-out that cannot reach the storefront still clears the local copy.
    pub async fn sign_out(&self) -> Result<()> {
        let token = {
            let session = self.session.lock().expect("the session lock");
            session
                .access_token
                .clone()
                .or_else(|| session.refresh_token.clone())
        };
        if let Some(token) = token {
            let _ = auth::revoke(&self.http, &self.endpoints, &token, &|step, detail| {
                self.trace(step, detail)
            })
            .await;
        }
        *self.session.lock().expect("the session lock") = Session::default();
        if let Some(store) = &self.store {
            crate::session::StoredSession::delete(&store.secrets)?;
        }
        Ok(())
    }

    // ------------------------------------------------------------ plumbing

    /// `current` once there is a token to prove it, `anonymous` otherwise.
    fn user(&self) -> &'static str {
        if self.session.lock().expect("the session lock").can_refresh() {
            "current"
        } else {
            "anonymous"
        }
    }

    fn require_session(&self) -> Result<()> {
        if self.is_signed_in() {
            Ok(())
        } else {
            Err(Error::NotSignedIn)
        }
    }

    fn file(&self, session: Session) {
        if let Some(store) = &self.store {
            let _ = crate::session::StoredSession::save(&store.secrets, &session);
        }
    }

    fn url(&self, path: &str, query: &[(&str, String)]) -> String {
        // `lang` and `curr` are on every call the site makes; without them the
        // storefront answers in the default locale rather than failing.
        let mut pairs: Vec<String> = query
            .iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, v)| format!("{k}={}", encode(v)))
            .collect();
        pairs.push("lang=en".into());
        pairs.push("curr=NZD".into());
        format!("{}?{}", self.endpoints.occ(path), pairs.join("&"))
    }

    async fn occ_get(
        &self,
        operation: &'static str,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<serde_json::Value> {
        self.occ_send(operation, wreq::Method::GET, path, query, None)
            .await
    }

    /// The same, but refusing rather than answering anonymously when there is
    /// no session, and renewing once if the token has lapsed.
    async fn occ_authed(
        &self,
        operation: &'static str,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<serde_json::Value> {
        self.require_session()?;
        match self.occ_get(operation, path, query).await {
            Err(e) if e.is_lapsed() => {
                self.renew().await?;
                self.occ_get(operation, path, query).await
            }
            other => other,
        }
    }

    async fn occ_send(
        &self,
        operation: &'static str,
        method: wreq::Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        // Renew before spending the request rather than after failing it: the
        // storefront's answer to an expired token is a 401 that a caller would
        // otherwise have to distinguish from being signed out.
        let expired = {
            let session = self.session.lock().expect("the session lock");
            session.can_refresh() && !session.token_fresh()
        };
        if expired {
            self.renew().await?;
        }

        let url = self.url(path, query);
        self.trace(method.as_str(), &url);

        let mut request = self
            .http
            .request(method.clone(), &url)
            .headers(auth::site_headers(&self.endpoints))
            // Hybris reads consent state off this header and answers 500 on
            // some paths without it.
            .header("x-anonymous-consents", "%5B%5D");
        if let Some(bearer) = self.session.lock().expect("the session lock").bearer() {
            request = request.header("authorization", bearer);
        }
        if let Some(body) = &body {
            request = request.json(body);
        }

        let method_name: &'static str = match method {
            wreq::Method::POST => "POST",
            wreq::Method::PUT => "PUT",
            wreq::Method::DELETE => "DELETE",
            _ => "GET",
        };
        let response = request.send().await.map_err(|source| {
            Error::Http(net_kit::HttpError::Transport {
                method: method_name,
                url: url.clone(),
                source,
            })
        })?;

        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();

        if !(200..300).contains(&status) {
            // The body first: Hybris says *why* in `errors[]` and the status
            // alone is usually just 400.
            if let Some(e) = wire::occ_error(operation, &text) {
                return Err(e);
            }
            if let Some(e) = Error::from_status(status, &text) {
                return Err(e);
            }
            return Err(Error::Http(net_kit::HttpError::Status {
                method: method_name,
                url,
                status,
                detail: net_kit::error::truncate(&text, 200),
                body: text,
            }));
        }

        // A 204 and an empty 200 are both ordinary here: `wishlist/clear` and
        // the delete paths answer with nothing.
        if text.trim().is_empty() {
            return Ok(serde_json::Value::Null);
        }
        serde_json::from_str(&text).map_err(|e| Error::decode(format!("reading {operation}"), e))
    }

    fn trace(&self, step: &str, detail: &str) {
        if self.debug {
            eprintln!("mitre10-api: {step} {detail}");
        }
    }
}

/// `priceForProducts` keys on the zero-padded eighteen-digit code.
fn pad(code: &str) -> String {
    let code = code.trim_start_matches('0');
    format!("{code:0>18}")
}

fn encode(value: &str) -> String {
    const UNRESERVED: &[u8] = b"-_.~,";
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || UNRESERVED.contains(byte) {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_product_code_is_padded_for_the_batch_endpoint_only() {
        // /products/{code} takes 174969; priceForProducts takes it padded, and
        // sending the wrong spelling answers with an empty products array
        // rather than an error.
        assert_eq!(pad("174969"), "000000000000174969");
        assert_eq!(pad("000000000000174969"), "000000000000174969");
        assert_eq!(pad("2035819"), "000000000002035819");
    }

    #[test]
    fn a_comma_survives_encoding_because_the_batch_endpoint_separates_on_it() {
        assert_eq!(encode("1,2,3"), "1,2,3");
        assert_eq!(encode("Ruatangata West"), "Ruatangata%20West");
    }
}
