//! The one GraphQL endpoint, and everything reached through it.

use std::sync::Mutex;

use net_kit::wreq;
use serde_json::json;

use crate::domain::{Cart, Category, Change, Filter, OrderDetail, OrderPage, Product, Store};
use crate::endpoints::Endpoints;
use crate::error::{Error, Result};
use crate::gql;
use crate::session::Session;
use crate::wire;

/// The site's own default ordering.
pub const DEFAULT_SORT: &str = "RELEVANCE";

/// The orderings the site's own sort menu offers. Anything else is still passed
/// through -- an unfamiliar value should reach the API rather than be rejected
/// here -- but these are what a `--sort` help string suggests.
pub const SORTS: [&str; 6] = [
    "RELEVANCE",
    "PRICE_LOW_HIGH",
    "PRICE_HIGH_LOW",
    "CUP_PRICE_LOW_HIGH",
    "FAVOURITES",
    "FREQUENCY",
];

/// The page size the site asks for. Nothing documents a ceiling, so this stays
/// at what is known to work and pages for the rest.
const MAX_PAGE_SIZE: u32 = 36;

/// How products are selected. All four are fields of one `CompositeSearchInput`,
/// which is why search, browse, specials and buy-again share a document.
#[derive(Clone, Debug)]
pub enum SearchBy {
    Keyword(String),
    Category(String),
    /// Everything currently on special. Carries no value of its own; the
    /// `SPECIALS` static filter is what selects it.
    Specials,
    /// What this account has bought before -- the site's "buy it again", which
    /// is how previous purchases are read here.
    BuyAgain,
}

impl SearchBy {
    fn field(&self) -> &'static str {
        match self {
            SearchBy::Keyword(_) => "byKeyword",
            SearchBy::Category(_) => "byCategoryKey",
            SearchBy::Specials => "byProductPromotionSpecials",
            SearchBy::BuyAgain => "byBuyAgain",
        }
    }

    fn value(&self) -> Option<&str> {
        match self {
            SearchBy::Keyword(v) | SearchBy::Category(v) => Some(v),
            SearchBy::Specials | SearchBy::BuyAgain => None,
        }
    }

    /// Whether this selection needs an account rather than a guest token.
    pub fn needs_account(&self) -> bool {
        matches!(self, SearchBy::BuyAgain)
    }

    /// A sensible default ordering. Buy-again is ranked by how often something
    /// was bought, which is the only ordering that makes sense for it.
    pub fn default_sort(&self) -> &'static str {
        match self {
            SearchBy::BuyAgain => "FREQUENCY",
            _ => DEFAULT_SORT,
        }
    }
}

pub struct SearchResult {
    pub products: Vec<Product>,
    pub total_available: u32,
}

/// What a client needs to sign itself in again when its session lapses.
///
/// A Woolworths session cannot be refreshed -- the cookie is encrypted and only
/// the site can mint one -- so the only renewal there is walks the whole login
/// flow again, which takes a password.
pub struct Reauth {
    pub email: String,
    /// Named rather than fetched: most clients never renew, and reading a
    /// password is a keychain prompt.
    pub password: net_kit::password::Source,
    pub secrets: net_kit::Secrets,
}

pub struct Client {
    http: wreq::Client,
    endpoints: Endpoints,
    /// Replaced in place by [`Client::renew`], so one command's later calls use
    /// the session its earlier ones bought.
    session: Mutex<Session>,
    reauth: Option<Reauth>,
}

impl Client {
    pub fn new(http: wreq::Client, endpoints: Endpoints, session: Session) -> Client {
        Client {
            http,
            endpoints,
            session: Mutex::new(session),
            reauth: None,
        }
    }

    /// Let this client sign in again by itself when the session it holds is
    /// refused. Without it a lapsed session is reported and the command stops.
    pub fn with_reauth(mut self, reauth: Option<Reauth>) -> Client {
        self.reauth = reauth;
        self
    }

    pub fn endpoints(&self) -> &Endpoints {
        &self.endpoints
    }

    pub fn session(&self) -> Session {
        self.session.lock().expect("session lock").clone()
    }

    /// Walk the login flow again and keep what it produces, so the next command
    /// does not have to repeat it.
    ///
    /// Public because this is also what `auth refresh` runs: there is nothing
    /// lighter to offer, since the session cookie cannot be renewed on its own.
    pub async fn renew(&self) -> Result<Session> {
        let reauth = self.reauth.as_ref().ok_or(Error::SessionUnrenewable)?;
        // A named source with nothing behind it is the same dead end as no
        // source at all, only found out later.
        let password = reauth
            .password
            .password()
            .await?
            .ok_or(Error::SessionUnrenewable)?;
        let session = crate::auth::login(
            &self.endpoints,
            &reauth.email,
            &password,
            &crate::auth::no_trace,
        )
        .await?;
        crate::session::StoredSession {
            email: Some(reauth.email.clone()),
            cookies: session.cookies(),
            obtained_at: net_kit::jwt::now_secs(),
        }
        .save(&reauth.secrets)?;
        *self.session.lock().expect("session lock") = session.clone();
        Ok(session)
    }

    /// Run one operation and hand back its `data`.
    async fn call(
        &self,
        operation: &'static str,
        variables: serde_json::Value,
    ) -> Result<serde_json::Value> {
        match self.call_with(operation, &variables, self.session()).await {
            // Everything account-scoped fails this way once the session lapses,
            // and there is nothing to refresh -- so the retry is a whole
            // sign-in, and only ever one.
            Err(e) if e.is_lapsed() && self.reauth.is_some() => {
                let session = self.renew().await?;
                self.call_with(operation, &variables, session).await
            }
            other => other,
        }
    }

    async fn call_with(
        &self,
        operation: &'static str,
        variables: &serde_json::Value,
        session: Session,
    ) -> Result<serde_json::Value> {
        let document = gql::document(operation)
            .ok_or_else(|| Error::Shape(format!("no GraphQL document for {operation}")))?;
        let url = format!("{}?op-name={operation}", self.endpoints.graphql());
        let body = json!({
            "operationName": operation,
            "variables": variables,
            "query": document,
        });

        let mut req = self
            .http
            .post(&url)
            .header(
                wreq::header::ACCEPT,
                "application/graphql-response+json,application/json;q=0.9",
            )
            .header(wreq::header::CONTENT_TYPE, "application/json")
            .header(wreq::header::ORIGIN, &self.endpoints.origin)
            .header(wreq::header::REFERER, format!("{}/", self.endpoints.origin))
            // The site sends the operation name as a header as well as in the
            // document. Cheap to match, and one less way to look unlike the
            // client this endpoint expects.
            .header("wnzx-operation-name", operation);

        if let Some(cookies) = session.header() {
            if let Ok(mut value) = wreq::header::HeaderValue::from_str(&cookies) {
                value.set_sensitive(true);
                req = req.header(wreq::header::COOKIE, value);
            }
        }

        let parsed: serde_json::Value =
            match net_kit::http::json("POST", &url, req.json(&body).send().await).await {
                Ok(v) => v,
                Err(e) => {
                    // A stored session that has lapsed is the common case, and
                    // its fix is not the fix for "you never signed in".
                    if e.status() == Some(401) && e.body().contains("session_expired") {
                        return Err(Error::SessionExpired);
                    }
                    return Err(e.into());
                }
            };

        // GraphQL answers 200 with an `errors` array rather than an HTTP
        // status, so the body has to be inspected either way.
        if let Some(errors) = parsed.get("errors").and_then(|e| e.as_array()) {
            if !errors.is_empty() {
                return Err(graphql_error(operation, errors));
            }
        }

        parsed
            .get("data")
            .filter(|d| !d.is_null())
            .cloned()
            .ok_or_else(|| Error::Shape(format!("{operation} returned no data")))
    }

    // ---- products ----

    async fn search_page(
        &self,
        by: &SearchBy,
        page: u32,
        page_size: u32,
        sort: &str,
        specials_only: bool,
    ) -> Result<wire::ProductPage> {
        let mut input = json!({
            "pageIndex": page,
            "pageSize": page_size,
            "facetFilters": [],
            "staticFilters": if specials_only { json!(["SPECIALS"]) } else { json!([]) },
            "sortBy": sort,
        });
        if let Some(value) = by.value() {
            input["value"] = json!(value);
        }

        let data = self
            .call(
                "ProductSearch",
                json!({ "searchInput": { by.field(): input } }),
            )
            .await?;
        let envelope: wire::SearchEnvelope = serde_json::from_value(data)
            .map_err(|e| Error::decode("parsing the product search response", e))?;
        envelope
            .my
            .and_then(|m| m.products)
            .ok_or_else(|| Error::Shape("the search response carried no products".into()))
    }

    /// Page until `max` products are in hand or the results run out.
    pub async fn search(
        &self,
        by: &SearchBy,
        max: u32,
        sort: &str,
        specials_only: bool,
    ) -> Result<SearchResult> {
        let page_size = max.clamp(1, MAX_PAGE_SIZE);
        let mut products: Vec<Product> = Vec::new();
        let mut page = 0u32;
        let mut total_pages = 1u32;
        let mut total_available = 0u32;

        while (products.len() as u32) < max && page < total_pages {
            let res = self
                .search_page(by, page, page_size, sort, specials_only)
                .await?;
            total_pages = res.total_pages.unwrap_or(page + 1);

            let mapped: Vec<Product> = res
                .results
                .into_iter()
                .filter_map(|r| match r {
                    wire::WireResult::ProductSummary(p) => {
                        p.into_product(&self.endpoints.origin, false)
                    }
                    // Ads are real products at real prices, marked so a reader
                    // can tell why they are at the top.
                    wire::WireResult::SponsoredProduct(p) => {
                        p.into_product(&self.endpoints.origin, true)
                    }
                    wire::WireResult::Other => None,
                })
                .collect();

            total_available = res
                .total_count
                .unwrap_or(products.len() as u32 + mapped.len() as u32);

            // A page can be all ad slots and no products, which is not the end
            // of the results -- only an empty page from the server is.
            if mapped.is_empty() && page + 1 >= total_pages {
                break;
            }

            let room = max as usize - products.len();
            products.extend(mapped.into_iter().take(room));
            page += 1;
        }

        Ok(SearchResult {
            products,
            total_available,
        })
    }

    /// The whole department tree.
    pub async fn categories(&self) -> Result<Category> {
        let data = self
            .call("GetAllCategories", json!({ "categoryKey": "" }))
            .await?;
        let envelope: wire::SearchEnvelope = serde_json::from_value(data)
            .map_err(|e| Error::decode("parsing the category tree", e))?;
        envelope
            .my
            .and_then(|m| m.categories)
            .and_then(wire::WireCategory::into_category)
            .ok_or_else(|| Error::Shape("the category response carried no categories".into()))
    }

    // ---- stores ----

    pub async fn stores(&self, query: Option<&str>, max: u32) -> Result<Vec<Store>> {
        let input = json!({
            "search": query.unwrap_or(""),
            // With no search term the API only answers with anything at all
            // when it is told to list them all.
            "allStores": true,
            "filter": { "sortingMethod": "DISTANCE", "sortingOrder": "ASCENDING", "max": max },
        });
        let data = self
            .call("SearchLocations", json!({ "input": input }))
            .await?;
        let envelope: wire::LocationsEnvelope =
            serde_json::from_value(data).map_err(|e| Error::decode("parsing the store list", e))?;
        Ok(envelope
            .locations
            .map(|l| l.locations)
            .unwrap_or_default()
            .into_iter()
            .filter_map(wire::WireLocation::into_store)
            .collect())
    }

    /// Bind the cart to a store, which is what prices are then quoted against.
    ///
    /// A cart mutation rather than a saved preference: on this site the store
    /// *is* a property of the cart. It works for a guest too.
    pub async fn set_store(&self, store_id: &str) -> Result<Option<String>> {
        let data = self
            .call(
                "SetCartShoppingMode",
                json!({ "setCartShoppingModeInput": {
                    "pickupLocationId": store_id,
                    "shoppingMode": "Pickup",
                }}),
            )
            .await?;
        Ok(data
            .get("setCartShoppingMode")
            .and_then(|c| c.get("shoppingMode"))
            .and_then(|s| s.get("pickupLocation"))
            .and_then(|p| p.get("name"))
            .and_then(|n| n.as_str())
            .map(str::to_string))
    }

    // ---- cart ----

    pub async fn cart(&self) -> Result<Cart> {
        let data = self.call("CustomerCart", json!({})).await?;
        let mut cart = read_cart(data, "customerCart")?;
        self.name_unnamed_lines(&mut cart).await;
        Ok(cart)
    }

    pub async fn cart_set(&self, changes: &[Change]) -> Result<Cart> {
        if changes.is_empty() {
            return self.cart().await;
        }
        let data = self
            .call(
                "SetCartLineItemQuantity",
                json!({ "input": { "cartLineItemQuantityUpdates": changes } }),
            )
            .await?;
        let mut cart = read_cart(data, "setCartLineItemQuantity")?;
        self.name_unnamed_lines(&mut cart).await;
        Ok(cart)
    }

    pub async fn cart_clear(&self) -> Result<Cart> {
        let data = self.call("ClearCart", json!({})).await?;
        read_cart(data, "clearCart")
    }

    /// Name the lines the cart would not name.
    ///
    /// A line takes its name from its variant, and the cart document can only
    /// spread the variant types it knows -- `GroceryVariant` and
    /// `RegulatedVariant`. Anything else, which is most of general
    /// merchandise, comes back with an unnamed variant and prints as a blank
    /// row. Search knows the product by its SKU, so ask it for the gaps.
    ///
    /// Only a blank line costs a request, so a grocery cart pays nothing, and
    /// a lookup that fails leaves the row as it was: a cart that cannot be
    /// fully named is still worth printing.
    async fn name_unnamed_lines(&self, cart: &mut Cart) {
        for i in 0..cart.lines.len() {
            if !cart.lines[i].name.trim().is_empty() || cart.lines[i].sku.is_empty() {
                continue;
            }
            let sku = cart.lines[i].sku.clone();
            let Ok(found) = self
                .search(&SearchBy::Keyword(sku.clone()), 1, DEFAULT_SORT, false)
                .await
            else {
                continue;
            };
            if let Some(found) = found.products.into_iter().find(|p| p.sku == sku) {
                cart.lines[i].name = found.name;
                cart.lines[i].brand = cart.lines[i].brand.take().or(found.brand);
            }
        }
    }

    // ---- orders ----

    async fn orders_page(&self, page: u32, size: u32, filter: Filter) -> Result<OrderPage> {
        let data = self
            .call(
                "Orders",
                json!({ "input": {
                    "pageIndex": page,
                    "pageSize": size,
                    "inclusiveFilter": filter.wire(),
                }}),
            )
            .await?;
        let envelope: wire::OrdersEnvelope =
            serde_json::from_value(data).map_err(|e| Error::decode("parsing the order list", e))?;
        Ok(envelope
            .orders
            .ok_or_else(|| Error::Shape("the order response carried no orders".into()))?
            .into_page())
    }

    /// Page until `max` orders are in hand or the history runs out.
    pub async fn orders(&self, max: u32, filter: Filter) -> Result<OrderPage> {
        let per_page = max.clamp(1, MAX_PAGE_SIZE);
        let mut orders = Vec::new();
        let mut page = 0u32;
        let mut total_pages = 1u32;
        let mut total = 0u32;

        while (orders.len() as u32) < max && page < total_pages {
            let res = self.orders_page(page, per_page, filter).await?;
            total = res.total;
            total_pages = res.total_pages.max(1);
            if res.orders.is_empty() {
                break;
            }
            let room = max as usize - orders.len();
            orders.extend(res.orders.into_iter().take(room));
            page += 1;
        }

        Ok(OrderPage {
            orders,
            total,
            total_pages,
        })
    }

    /// One order and what was in it.
    pub async fn order(&self, order_number: &str) -> Result<OrderDetail> {
        let data = self
            .call("OrderDetails", json!({ "orderNumber": order_number }))
            .await?;
        let envelope: wire::OrderEnvelope =
            serde_json::from_value(data).map_err(|e| Error::decode("parsing the order", e))?;
        envelope
            .order
            .and_then(wire::WireOrderDetail::into_detail)
            .ok_or_else(|| Error::Shape(format!("no order {order_number} on this account")))
    }
}

fn read_cart(data: serde_json::Value, field: &str) -> Result<Cart> {
    let raw = data
        .get(field)
        .filter(|v| !v.is_null())
        .ok_or_else(|| Error::Shape(format!("the response carried no {field}")))?;
    Ok(serde_json::from_value::<wire::WireCart>(raw.clone())
        .map_err(|e| Error::decode("parsing the cart", e))?
        .into_cart())
}

/// Turn a GraphQL `errors` array into one typed error.
///
/// The API signals "not logged in" as an `AUTH_NOT_AUTHENTICATED` extension on
/// a 200, so this is where a lapsed session is actually noticed. Reading a
/// structured code is what replaces matching English in an error chain.
fn graphql_error(operation: &'static str, errors: &[serde_json::Value]) -> Error {
    let unauthenticated = errors.iter().any(|e| {
        e.get("extensions")
            .and_then(|x| x.get("code"))
            .and_then(|c| c.as_str())
            .is_some_and(|c| c.contains("AUTH_NOT_AUTHENTICATED") || c.contains("UNAUTHENTICATED"))
    });
    if unauthenticated {
        return Error::NotSignedIn;
    }

    let messages: Vec<String> = errors
        .iter()
        .filter_map(|e| e.get("message").and_then(|m| m.as_str()))
        .map(str::to_string)
        .collect();
    let message = if messages.is_empty() {
        net_kit::error::truncate(&serde_json::to_string(errors).unwrap_or_default(), 300)
    } else {
        messages.join("; ")
    };
    Error::Graphql { operation, message }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_way_of_selecting_products_names_its_own_input_field() {
        assert_eq!(SearchBy::Keyword("milk".into()).field(), "byKeyword");
        assert_eq!(SearchBy::Category("9-ABC".into()).field(), "byCategoryKey");
        assert_eq!(SearchBy::Specials.field(), "byProductPromotionSpecials");
        assert_eq!(SearchBy::BuyAgain.field(), "byBuyAgain");
    }

    #[test]
    fn only_buy_again_needs_an_account() {
        assert!(SearchBy::BuyAgain.needs_account());
        assert!(!SearchBy::Keyword("milk".into()).needs_account());
        assert!(!SearchBy::Specials.needs_account());
    }

    #[test]
    fn buy_again_is_ordered_by_how_often_something_was_bought() {
        assert_eq!(SearchBy::BuyAgain.default_sort(), "FREQUENCY");
        assert_eq!(SearchBy::Specials.default_sort(), DEFAULT_SORT);
    }

    #[test]
    fn an_unauthenticated_extension_is_read_as_a_code_not_as_english() {
        let errors = vec![serde_json::json!({
            "message": "Something went wrong",
            "extensions": { "code": "AUTH_NOT_AUTHENTICATED" }
        })];
        assert!(matches!(
            graphql_error("CustomerCart", &errors),
            Error::NotSignedIn
        ));
    }

    #[test]
    fn other_graphql_errors_keep_their_messages() {
        let errors = vec![serde_json::json!({ "message": "Field 'x' is unknown" })];
        let err = graphql_error("CustomerCart", &errors);
        assert!(err.to_string().contains("Field 'x' is unknown"), "{err}");
        assert!(!err.is_lapsed());
    }

    #[test]
    fn an_errors_array_with_no_messages_still_says_something() {
        let errors = vec![serde_json::json!({ "path": ["cart"] })];
        let err = graphql_error("CustomerCart", &errors);
        assert!(err.to_string().contains("cart"), "{err}");
    }
}
