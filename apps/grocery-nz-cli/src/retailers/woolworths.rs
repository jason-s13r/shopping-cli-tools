//! Woolworths, behind one GraphQL endpoint.
//!
//! Two things shape this adapter. Authorisation is entirely by cookie, and the
//! account cookie is encrypted -- only the site can mint one -- so the only
//! renewal is walking the whole login flow again, which takes a password.
//! And the selected store is a property of the *cart*, not a local preference,
//! so `store set` here is a mutation rather than a saved string.

pub mod convert;

use async_trait::async_trait;
use gsnz_core::{
    AuthStatus, Caps, Cart, Change, CodePrompt, Department, Error, Fact, Order, OrderFilter,
    OrderLine, OrderSummary, Result, Retailer, RetailerId, Search, SearchBy, SearchResult, Sort,
    Store,
};
use net_kit::{wreq, Paths, Secrets};
use wwnz_api::{Endpoints, Session, StoredSession};

const ID: RetailerId = RetailerId::Woolworths;

pub struct Woolworths {
    http: wreq::Client,
    endpoints: Endpoints,
    paths: Paths,
    secrets: Secrets,
    password: net_kit::password::Source,
    /// A `--store` given for this run. Recorded rather than applied: see
    /// [`Woolworths::no_store_override`].
    store_override: Option<String>,
    debug: bool,
}

pub struct Setup {
    pub endpoints: Endpoints,
    pub paths: Paths,
    pub secrets: Secrets,
    pub password: net_kit::password::Source,
    pub store_override: Option<String>,
    /// Narrate the login flow on stderr. Injected rather than read here: no
    /// library in this tree reads the environment.
    pub debug: bool,
}

impl Woolworths {
    pub fn new(setup: Setup) -> Result<Woolworths> {
        let http = net_kit::http::build(wwnz_api::client_spec()).map_err(|e| Error::Upstream {
            retailer: ID,
            message: "building the HTTP client".into(),
            source: Box::new(e),
        })?;
        Ok(Woolworths {
            http,
            endpoints: setup.endpoints,
            paths: setup.paths,
            secrets: setup.secrets,
            password: setup.password,
            store_override: setup.store_override,
            debug: setup.debug,
        })
    }

    fn err(&self, e: wwnz_api::Error) -> Error {
        convert::error(e)
    }

    /// Refuse a per-command `--store` rather than ignoring it.
    ///
    /// Prices here are quoted against whatever store the *cart* is bound to,
    /// which is server-side state. Accepting `--store` and then answering with
    /// the other store's prices would be a wrong-price bug with nothing on
    /// screen to explain it.
    fn no_store_override(&self) -> Result<()> {
        match &self.store_override {
            None => Ok(()),
            Some(_) => Err(Error::unsupported_hint(
                ID,
                "a per-command --store",
                "the Woolworths cart is bound to a store server-side; \
                 run `gsnz -b ww store set <store>` instead",
            )),
        }
    }

    fn stored(&self) -> Result<Option<StoredSession>> {
        StoredSession::load(&self.secrets).map_err(|e| self.err(e))
    }

    /// A client for the account-scoped half: the cart, orders, buy-again.
    async fn account(&self) -> Result<Held> {
        let stored = self.stored()?.ok_or(Error::NeedsLogin { retailer: ID })?;
        let session = stored.session();
        let before = session.cookies();
        // An account to sign in as is what decides this; the password is named
        // rather than read, so a command that never renews pays no keychain
        // prompt for one.
        let reauth = stored
            .email
            .clone()
            .filter(|email| !email.is_empty())
            .map(|email| wwnz_api::Reauth {
                email,
                password: self.password.clone(),
                secrets: self.secrets.clone(),
            });
        let client = wwnz_api::Client::new(self.http.clone(), self.endpoints.clone(), session)
            .with_reauth(reauth);
        Ok(Held {
            client,
            before,
            email: stored.email,
        })
    }

    /// A client for the products-and-stores half, which a guest token covers.
    /// The account session is used when there is one, so a signed-in run sees
    /// member pricing.
    async fn guest(&self) -> Result<wwnz_api::Client> {
        let session = match self.stored()? {
            Some(stored) => stored.session(),
            None => wwnz_api::session::guest(&self.http, &self.endpoints.origin, &self.paths)
                .await
                .map_err(|e| self.err(e))?,
        };
        Ok(wwnz_api::Client::new(
            self.http.clone(),
            self.endpoints.clone(),
            session,
        ))
    }

    /// Keep a session the client renewed for itself, so the next command does
    /// not have to log in again.
    fn persist(&self, held: &Held) {
        let after = held.client.session();
        if after.cookies() == held.before {
            return;
        }
        let _ = StoredSession {
            email: held.email.clone(),
            cookies: after.cookies(),
            obtained_at: net_kit::jwt::now_secs(),
        }
        .save(&self.secrets);
    }

    fn save_session(&self, email: Option<String>, session: &Session) -> Result<()> {
        StoredSession {
            email,
            cookies: session.cookies(),
            obtained_at: net_kit::jwt::now_secs(),
        }
        .save(&self.secrets)
        .map_err(|e| self.err(e))
    }

    async fn category_key(&self, name: &str) -> Result<String> {
        // A key reaches the API unchanged; a name has to be looked up, because
        // browsing here selects on the key and nothing else.
        let root = self
            .guest()
            .await?
            .categories()
            .await
            .map_err(|e| self.err(e))?;
        root.find(name).map(|c| c.key.clone()).ok_or_else(|| {
            Error::Other(format!(
                "no Woolworths department matching {name:?}: run `gsnz -b ww departments`"
            ))
        })
    }
}

/// A client plus what is needed to notice it signed itself in again.
struct Held {
    client: wwnz_api::Client,
    before: std::collections::BTreeMap<String, String>,
    email: Option<String>,
}

fn sort(sort: &Sort) -> String {
    match sort {
        Sort::Relevance => wwnz_api::DEFAULT_SORT.to_string(),
        // The site's menu has no popularity ordering; favourites is the nearest
        // thing it does offer.
        Sort::Popularity => "FAVOURITES".into(),
        Sort::PriceAsc => "PRICE_LOW_HIGH".into(),
        Sort::PriceDesc => "PRICE_HIGH_LOW".into(),
        // No name ordering exists here. Relevance is what the site falls back
        // to for anything it does not recognise, so ask for it outright.
        Sort::NameAsc => wwnz_api::DEFAULT_SORT.to_string(),
        Sort::Raw(raw) => raw.clone(),
    }
}

#[async_trait]
impl Retailer for Woolworths {
    fn id(&self) -> RetailerId {
        ID
    }

    fn facts(&self) -> Vec<Fact> {
        vec![
            Fact::new("storefront", &self.endpoints.origin),
            // Auth0, not an API host: the GraphQL endpoint is on the
            // storefront. It is here because a login failing is the most
            // likely thing to need diagnosing.
            Fact::new("sign-in", &self.endpoints.auth),
        ]
    }

    fn caps(&self) -> Caps {
        Caps {
            departments: true,
            order_detail: true,
            previous_purchases: true,
            refresh_session: true,
            import_cookies: true,
            weight_lines: true,
            // The store is a property of the cart on this site, so selecting
            // one is a mutation and takes effect for the account, not the run.
            server_side_store: true,
        }
    }

    async fn search(&self, search: &Search) -> Result<SearchResult> {
        self.no_store_override()?;
        let by = match &search.by {
            SearchBy::Query(q) => wwnz_api::SearchBy::Keyword(q.clone()),
            SearchBy::Department(d) => wwnz_api::SearchBy::Category(self.category_key(d).await?),
            // The only command producing `Everything` is `specials`, and the
            // site has no listing of the whole catalogue to offer instead.
            SearchBy::Everything => wwnz_api::SearchBy::Specials,
        };
        // Buy-again is the one selection needing an account; the rest are
        // guest-scoped, and a guest run must not be made to log in.
        let client = if by.needs_account() {
            self.account().await?.client
        } else {
            self.guest().await?
        };
        let found = client
            .search(
                &by,
                super::fetch_limit(search),
                &sort(&search.sort),
                search.specials_only,
            )
            .await
            .map_err(|e| self.err(e))?;
        Ok(SearchResult {
            products: found.products.into_iter().map(convert::product).collect(),
            total: Some(found.total_available),
        }
        .narrow(search))
    }

    async fn stores(&self, query: Option<&str>, max: u32) -> Result<Vec<Store>> {
        let client = self.guest().await?;
        let found = client
            .stores(query, max.min(200))
            .await
            .map_err(|e| self.err(e))?;
        Ok(super::narrow_stores(
            found.into_iter().map(convert::store).collect(),
            query,
            max,
        ))
    }

    async fn select_store(&self, id: &str) -> Result<Store> {
        let store = super::resolve_store(self.stores(None, u32::MAX).await?, id, ID)?;
        // Not a local preference: prices are quoted against whatever store the
        // cart is bound to, so this has to reach the server to mean anything.
        let held = match self.stored()? {
            Some(_) => {
                let held = self.account().await?;
                held.client
                    .set_store(&store.id)
                    .await
                    .map_err(|e| self.err(e))?;
                Some(held)
            }
            None => {
                self.guest()
                    .await?
                    .set_store(&store.id)
                    .await
                    .map_err(|e| self.err(e))?;
                None
            }
        };
        if let Some(held) = &held {
            self.persist(held);
        }
        Ok(store)
    }

    async fn departments(&self) -> Result<Vec<Department>> {
        self.no_store_override()?;
        let root = self
            .guest()
            .await?
            .categories()
            .await
            .map_err(|e| self.err(e))?;
        // The response is one synthetic root wrapping the real departments;
        // returning its children lines up with the Foodstuffs tree, which has
        // no such wrapper.
        Ok(root.children.iter().map(convert::department).collect())
    }

    async fn cart(&self) -> Result<Cart> {
        let held = self.account().await?;
        let cart = held.client.cart().await.map_err(|e| self.err(e))?;
        self.persist(&held);
        Ok(convert::cart(cart))
    }

    async fn cart_apply(&self, changes: &[Change]) -> Result<Cart> {
        let wire: Vec<wwnz_api::Change> = changes.iter().map(convert::change).collect();
        let held = self.account().await?;
        let cart = held.client.cart_set(&wire).await.map_err(|e| self.err(e))?;
        self.persist(&held);
        Ok(convert::cart(cart))
    }

    async fn cart_clear(&self) -> Result<Cart> {
        let held = self.account().await?;
        let cart = held.client.cart_clear().await.map_err(|e| self.err(e))?;
        self.persist(&held);
        Ok(convert::cart(cart))
    }

    async fn orders(&self, filter: OrderFilter, max: u32) -> Result<Vec<OrderSummary>> {
        let filter = match filter {
            OrderFilter::Active => wwnz_api::Filter::Active,
            OrderFilter::Past => wwnz_api::Filter::Past,
            // Every Woolworths order is an online one; there is no second
            // history to separate it from.
            OrderFilter::All | OrderFilter::Online => wwnz_api::Filter::All,
            OrderFilter::InStore => {
                return Err(Error::unsupported_hint(
                    ID,
                    "in-store receipts",
                    "Woolworths keeps no till-receipt history; try `-b nw` or `-b pns`",
                ))
            }
        };
        let held = self.account().await?;
        let page = held
            .client
            .orders(max, filter)
            .await
            .map_err(|e| self.err(e))?;
        self.persist(&held);
        Ok(page.orders.into_iter().map(convert::summary).collect())
    }

    async fn order(&self, id: &str) -> Result<Order> {
        let held = self.account().await?;
        let order = held.client.order(id).await.map_err(|e| self.err(e))?;
        self.persist(&held);
        Ok(convert::order(order))
    }

    async fn previous_purchases(&self, max: u32, _exclude_cart: bool) -> Result<Vec<OrderLine>> {
        let held = self.account().await?;
        let found = held
            .client
            .search(&wwnz_api::SearchBy::BuyAgain, max, "FREQUENCY", false)
            .await
            .map_err(|e| self.err(e))?;
        self.persist(&held);
        // "Buy it again" answers with products at today's price rather than
        // with historical lines, so the money here is what one would cost now,
        // not what was paid. That is the only thing this API offers.
        Ok(found
            .products
            .into_iter()
            .map(convert::product)
            .map(|p| OrderLine {
                key: p.key,
                sku: p.sku,
                name: p.name,
                brand: p.brand,
                quantity: gsnz_core::Quantity::units(1),
                total_cents: p.price_cents,
            })
            .collect())
    }

    async fn auth_status(&self) -> Result<AuthStatus> {
        let Some(stored) = self.stored()? else {
            return Ok(AuthStatus {
                retailer: ID,
                signed_in: false,
                account: None,
                expires_in: None,
                detail: None,
            });
        };
        let age = net_kit::jwt::now_secs().saturating_sub(stored.obtained_at);
        Ok(AuthStatus {
            retailer: ID,
            signed_in: true,
            account: stored.email,
            // The session cookie is encrypted, so there is no expiry to read --
            // only how long ago it was obtained.
            expires_in: None,
            detail: Some(format!(
                "signed in {} ago; {}",
                cli_kit::human_duration(std::time::Duration::from_secs(age)),
                if self.password.exists().unwrap_or(false) {
                    "renewable from the stored password"
                } else {
                    "renewing needs a password, since the cookie cannot be refreshed"
                }
            )),
        })
    }

    async fn login(
        &self,
        email: &str,
        password: &str,
        _code: CodePrompt<'_>,
    ) -> Result<AuthStatus> {
        // Auth0 challenges nothing this flow can answer, so the prompt is
        // unused rather than optional: a challenge here fails the login.
        let debug = self.debug;
        let trace = move |step: &str, detail: &str| {
            if debug {
                eprintln!("gsnz: auth {step}: {detail}");
            }
        };
        let session = wwnz_api::auth::login(&self.endpoints, email, password, &trace)
            .await
            .map_err(convert::login_error)?;
        self.save_session(Some(email.to_string()), &session)?;
        self.auth_status().await
    }

    async fn refresh_session(&self) -> Result<AuthStatus> {
        let held = self.account().await?;
        let session = held.client.renew().await.map_err(|e| self.err(e))?;
        self.save_session(held.email.clone(), &session)?;
        self.auth_status().await
    }

    async fn import_cookies(&self, text: &str) -> Result<AuthStatus> {
        let session = wwnz_api::auth::from_netscape(text);
        if !session.account {
            return Err(Error::Other(
                "no Woolworths session cookies in that file: it should be a Netscape \
                 cookies.txt exported while signed in, carrying __session__0 and __session__1"
                    .into(),
            ));
        }
        // The email is not in the cookies, so an imported session is nameless
        // until a login replaces it -- and cannot be renewed, for the same
        // reason: `renew` needs an address to sign in with.
        self.save_session(None, &session)?;
        self.auth_status().await
    }

    async fn logout(&self) -> Result<bool> {
        let dropped = StoredSession::clear(&self.secrets).map_err(|e| self.err(e))?;
        wwnz_api::session::clear_guest(&self.paths);
        let _ = net_kit::password::clear(&self.secrets);
        Ok(dropped)
    }
}
