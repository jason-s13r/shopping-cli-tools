//! New World and PAK'nSAVE, as one [`Retailer`] implementation instantiated
//! twice.
//!
//! Two names, one Foodstuffs platform: the same endpoints, the same Club Plus
//! login, the same product codes. What differs is the hostname and the code the
//! token is scoped to, which is what [`Banner`] carries. A New World token
//! presented to PAK'nSAVE is not refused -- it answers with an empty cart --
//! so the two are kept apart by the [`fsnz_api::token`] cache, not by trust.

pub mod convert;

use std::sync::Arc;

use async_trait::async_trait;
use fsnz_api::{Banner, ClubPlusEndpoints, Endpoints};
use gsnz_core::{
    AuthStatus, Caps, Cart, Change, CodePrompt, Department, Error, Fact, Order, OrderFilter,
    OrderLine, OrderSummary, Result, Retailer, RetailerId, Search, SearchBy, SearchResult, Sort,
    Store,
};
use net_kit::{wreq, Jar, Paths, Secrets};

/// Where this banner's cookies are filed. Both banners share one jar because
/// the Cloudflare clearance and the Club Plus refresh cookie are set on
/// `.co.nz` hosts either can be reached through.
const COOKIE_ACCOUNT: &str = "cookies";

pub struct Foodstuffs {
    id: RetailerId,
    banner: Banner,
    http: wreq::Client,
    jar: Arc<Jar>,
    endpoints: Endpoints,
    clubplus: ClubPlusEndpoints,
    paths: Paths,
    secrets: Secrets,
    token_command: Option<String>,
    explicit_token: Option<String>,
    password: net_kit::password::Source,
    store_id: Option<String>,
    /// Echoed back to the SSO exchange as `fingerprintGuest`. Taken from the
    /// client that will send the request rather than named twice.
    user_agent: String,
}

/// Everything needed to build one, so the caller assembles it in one place
/// instead of passing nine arguments in the right order.
pub struct Setup {
    pub id: RetailerId,
    pub endpoints: Endpoints,
    pub clubplus: ClubPlusEndpoints,
    pub paths: Paths,
    pub secrets: Secrets,
    pub token_command: Option<String>,
    pub explicit_token: Option<String>,
    pub password: net_kit::password::Source,
    pub store_id: Option<String>,
}

impl Foodstuffs {
    pub fn new(setup: Setup) -> Result<Foodstuffs> {
        let banner = convert::banner(setup.id)
            .ok_or_else(|| Error::Other(format!("{} is not a Foodstuffs banner", setup.id)))?;
        let jar = Arc::new(Jar::load(
            &setup.secrets,
            COOKIE_ACCOUNT,
            fsnz_api::cookie_keep,
        ));
        let http = net_kit::http::build(fsnz_api::client_spec(jar.clone())).map_err(|e| {
            Error::Upstream {
                retailer: setup.id,
                message: "building the HTTP client".into(),
                source: Box::new(e),
            }
        })?;
        Ok(Foodstuffs {
            id: setup.id,
            banner,
            user_agent: fsnz_api::http::user_agent(),
            http,
            jar,
            endpoints: setup.endpoints,
            clubplus: setup.clubplus,
            paths: setup.paths,
            secrets: setup.secrets,
            token_command: setup.token_command,
            explicit_token: setup.explicit_token,
            password: setup.password,
            store_id: setup.store_id,
        })
    }

    fn err(&self, e: fsnz_api::Error) -> Error {
        convert::error(self.id, e)
    }

    fn store(&self) -> Result<&str> {
        self.store_id
            .as_deref()
            .ok_or(Error::NoStore { retailer: self.id })
    }

    /// A client carrying a usable token.
    ///
    /// `guest` asks for an anonymous one even when signed in, which some
    /// endpoints require: `/v1/edge/store` answers a guest token with the store
    /// list and an account token with a flat 400.
    async fn client(&self, guest: bool) -> Result<fsnz_api::Client> {
        let token = fsnz_api::token::acquire(fsnz_api::token::Request {
            http: &self.http,
            banner: self.banner,
            endpoints: &self.endpoints,
            clubplus: &self.clubplus,
            paths: &self.paths,
            secrets: &self.secrets,
            explicit: self.explicit_token.as_deref(),
            token_command: self.token_command.as_deref(),
            password: Some(&self.password),
            user_agent: &self.user_agent,
            guest,
            force_refresh: false,
        })
        .await
        .map_err(|e| self.err(e))?;
        // Whatever the handshake picked up is worth keeping: the next run then
        // starts past the bot check rather than at it.
        self.jar.save(&self.secrets);
        Ok(fsnz_api::Client::new(
            self.http.clone(),
            self.banner,
            self.endpoints.clone(),
            token.token,
        ))
    }

    fn clubplus_config<'a>(&'a self, device_id: &'a str) -> fsnz_api::auth::Config<'a> {
        fsnz_api::auth::Config {
            http: &self.http,
            clubplus: &self.clubplus,
            device_id,
        }
    }
}

/// Foodstuffs orders by a `sortOrder` string. Only `NI_POPULARITY_ASC` is
/// attested in a recording; the rest follow its shape and are unverified, which
/// is why [`Sort::Raw`] passes through untouched as the escape hatch.
fn sort(sort: &Sort) -> String {
    match sort {
        Sort::Relevance | Sort::Popularity => fsnz_api::DEFAULT_SORT.to_string(),
        Sort::PriceAsc => "NI_PRICE_ASC".into(),
        Sort::PriceDesc => "NI_PRICE_DESC".into(),
        Sort::NameAsc => "NI_TITLE_ASC".into(),
        Sort::Raw(raw) => raw.clone(),
    }
}

#[async_trait]
impl Retailer for Foodstuffs {
    fn id(&self) -> RetailerId {
        self.id
    }

    fn facts(&self) -> Vec<Fact> {
        let mut facts = vec![
            Fact::new("storefront", &self.endpoints.origin),
            Fact::new("api", &self.endpoints.api),
        ];
        // The cached account token, if there is one. Reported because it is
        // the thing between a Club Plus login and a working request, and the
        // one that quietly lapses after half an hour.
        facts.push(Fact::new(
            "token",
            match fsnz_api::token::peek(&self.paths, self.banner, false) {
                Some(token) if token.fresh() => match token.expires_in() {
                    Some(left) => format!(
                        "cached, {}, expires in {}",
                        token.source.describe(),
                        cli_kit::human_duration(left)
                    ),
                    None => format!("cached, {}", token.source.describe()),
                },
                Some(_) => "cached but lapsed; the next call mints a fresh one".into(),
                None => "none cached; the next call mints one".into(),
            },
        ));
        facts
    }

    fn caps(&self) -> Caps {
        Caps {
            departments: true,
            order_detail: true,
            previous_purchases: true,
            refresh_session: true,
            import_cookies: true,
            weight_lines: true,
            // `store set` writes the local preference *and* binds the
            // account's cart, which is a separate store the API keeps.
            server_side_store: true,
        }
    }

    async fn search(&self, search: &Search) -> Result<SearchResult> {
        let store = self.store()?;
        let (query, department) = match &search.by {
            SearchBy::Query(q) => (q.as_str(), None),
            SearchBy::Department(d) => ("", Some(d.as_str())),
            SearchBy::Everything => ("", None),
        };
        let filters = fsnz_api::filters(store, search.specials_only, department);
        let client = self.client(false).await?;
        let found = client
            // Over-fetch when a client-side size filter will throw some away,
            // so `--size 2l --limit 5` still has five to show.
            .collect(
                store,
                query,
                &filters,
                super::fetch_limit(search),
                &sort(&search.sort),
            )
            .await
            .map_err(|e| self.err(e))?;
        Ok(SearchResult {
            products: found
                .products
                .into_iter()
                .map(|p| convert::product(self.id, p))
                .collect(),
            total: Some(found.total_available),
        }
        .narrow(search))
    }

    async fn stores(&self, query: Option<&str>, max: u32) -> Result<Vec<Store>> {
        let client = self.client(true).await?;
        let all = client.stores().await.map_err(|e| self.err(e))?;
        Ok(super::narrow_stores(
            all.into_iter()
                .map(|s| convert::store(self.id, s))
                .collect(),
            query,
            max,
        ))
    }

    async fn select_store(&self, id: &str) -> Result<Store> {
        let store = super::resolve_store(self.stores(None, u32::MAX).await?, id, self.id)?;
        // Two different stores: the one searches are scoped to, which is the
        // local preference the caller saves, and the one the account's cart
        // carries. Only binding the second makes `cart add` work, and until
        // this existed a signed-in account was refused with "Store is not
        // defined" and nothing to do about it from here.
        //
        // Signed out there is no cart to bind and browsing still works, so
        // that case is passed over rather than raised.
        match self.client(false).await {
            Ok(client) => {
                if let Err(e) = client.set_store(&store.id).await {
                    let e = self.err(e);
                    if !matches!(e, Error::NeedsLogin { .. }) {
                        return Err(e);
                    }
                }
            }
            Err(Error::NeedsLogin { .. }) => {}
            Err(e) => return Err(e),
        }
        Ok(store)
    }

    async fn departments(&self) -> Result<Vec<Department>> {
        let store = self.store()?;
        let client = self.client(false).await?;
        let raw = client.categories(store).await.map_err(|e| self.err(e))?;
        Ok(raw.into_iter().map(|c| convert::department(c, 0)).collect())
    }

    async fn cart(&self) -> Result<Cart> {
        let client = self.client(false).await?;
        let cart = client.cart().await.map_err(|e| self.err(e))?;
        Ok(convert::cart(self.id, cart))
    }

    async fn cart_apply(&self, changes: &[Change]) -> Result<Cart> {
        let wire: Vec<fsnz_api::Change> = changes
            .iter()
            .map(|c| convert::change(self.id, c))
            .collect();
        let client = self.client(false).await?;
        let cart = client.cart_apply(&wire).await.map_err(|e| self.err(e))?;
        Ok(convert::cart(self.id, cart))
    }

    async fn cart_clear(&self) -> Result<Cart> {
        let client = self.client(false).await?;
        client.cart_clear().await.map_err(|e| self.err(e))?;
        let cart = client.cart().await.map_err(|e| self.err(e))?;
        Ok(convert::cart(self.id, cart))
    }

    async fn orders(&self, filter: OrderFilter, max: u32) -> Result<Vec<OrderSummary>> {
        let source = match filter {
            OrderFilter::Online => Some(fsnz_api::order::Source::Online),
            OrderFilter::InStore => Some(fsnz_api::order::Source::InStore),
            _ => None,
        };
        let client = self.client(false).await?;
        let page = client.orders(max, source).await.map_err(|e| self.err(e))?;
        Ok(page
            .orders
            .into_iter()
            .map(|o| convert::summary(self.id, o))
            .collect())
    }

    async fn order(&self, id: &str) -> Result<Order> {
        let client = self.client(false).await?;
        // An id says which of the two histories it belongs to; asking the wrong
        // endpoint answers "no such order" rather than redirecting.
        let source = fsnz_api::order::Source::infer(id);
        let order = client.order(id, source).await.map_err(|e| self.err(e))?;
        Ok(convert::order(self.id, order))
    }

    async fn previous_purchases(&self, max: u32, exclude_cart: bool) -> Result<Vec<OrderLine>> {
        let client = self.client(false).await?;
        let lines = client
            .previous_purchases(max, exclude_cart)
            .await
            .map_err(|e| self.err(e))?;
        Ok(lines.into_iter().map(convert::order_line).collect())
    }

    async fn auth_status(&self) -> Result<AuthStatus> {
        let stored = fsnz_api::auth::session::load(&self.secrets).map_err(|e| self.err(e))?;
        let Some(stored) = stored else {
            return Ok(AuthStatus {
                retailer: self.id,
                signed_in: false,
                account: None,
                expires_in: None,
                detail: None,
            });
        };
        let expires_in = stored
            .expires_at_ms()
            .map(|at| at.saturating_sub(net_kit::jwt::now_ms()) / 1000);
        let renewable = stored.can_renew();
        Ok(AuthStatus {
            retailer: self.id,
            signed_in: true,
            account: Some(stored.email),
            expires_in,
            detail: Some(
                if renewable {
                    "renewable without a password"
                } else if self.password.exists().unwrap_or(false) {
                    "renewable from the stored password"
                } else {
                    "cannot be renewed; sign in again when it lapses"
                }
                .into(),
            ),
        })
    }

    async fn login(&self, email: &str, password: &str, code: CodePrompt<'_>) -> Result<AuthStatus> {
        let device_id = fsnz_api::auth::device_id(&self.paths).map_err(|e| self.err(e))?;
        let cfg = self.clubplus_config(&device_id);
        let session = match fsnz_api::auth::login(&cfg, email, password)
            .await
            .map_err(|e| convert::login_error(self.id, e))?
        {
            fsnz_api::auth::Login::Complete(session) => session,
            fsnz_api::auth::Login::ChallengeRequired(challenge) => {
                // The challenge tokens authorise nothing else and are never
                // stored, so the code has to be answered inside this call.
                let typed = code(&challenge.method)
                    .map_err(|e| Error::Other(format!("reading the verification code: {e}")))?;
                fsnz_api::auth::clubplus::complete_challenge(&cfg, &challenge, &typed)
                    .await
                    .map_err(|e| convert::login_error(self.id, e))?
            }
        };
        fsnz_api::auth::session::save(
            &self.secrets,
            &fsnz_api::auth::StoredLogin {
                email: email.to_string(),
                access_token: session.access_token,
                refresh_token: session.refresh_token,
                refreshed_at_ms: Some(net_kit::jwt::now_ms()),
            },
        )
        .map_err(|e| self.err(e))?;
        // A token cached against the previous account would otherwise be
        // reused, and it answers with that account's cart.
        for guest in [true, false] {
            let _ =
                std::fs::remove_file(fsnz_api::token::cache_file(&self.paths, self.banner, guest));
        }
        self.auth_status().await
    }

    async fn refresh_session(&self) -> Result<AuthStatus> {
        let device_id = fsnz_api::auth::device_id(&self.paths).map_err(|e| self.err(e))?;
        let cfg = self.clubplus_config(&device_id);
        fsnz_api::auth::session::active_session(&cfg, &self.secrets, Some(&self.password), true)
            .await
            .map_err(|e| self.err(e))?;
        self.auth_status().await
    }

    async fn import_cookies(&self, text: &str) -> Result<AuthStatus> {
        let host = self
            .endpoints
            .origin
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .to_string();
        let cookies = net_kit::cookies::from_netscape(text, &host);
        if cookies.is_empty() {
            return Err(Error::Other(format!(
                "no cookies for {host} in that file: it should be a Netscape cookies.txt \
                 exported while signed in to {}",
                self.banner
            )));
        }
        // A year out: the file's own expiry is not carried through, and the
        // server rejects a stale cookie long before a jar would drop it.
        let expires_at = net_kit::jwt::now_ms() / 1000 + 365 * 24 * 60 * 60;
        let mut kept = 0usize;
        for (name, value) in &cookies {
            self.jar.insert(&host, name, value, expires_at);
            if fsnz_api::cookie_keep(name) {
                kept += 1;
            }
        }
        self.jar.save(&self.secrets);
        if kept == 0 {
            return Err(Error::Other(format!(
                "that file has {} cookies for {host}, but none this tool can use. \
                 The ones that matter are fs-user-token and refresh_token.",
                cookies.len()
            )));
        }
        let mut status = self.auth_status().await?;
        status.signed_in = true;
        if self.jar.get("refresh_token").is_none() {
            status.detail = Some(
                "imported without a refresh_token, so this session lapses within the hour \
                 and cannot renew itself"
                    .into(),
            );
        }
        Ok(status)
    }

    async fn logout(&self) -> Result<bool> {
        let dropped = fsnz_api::auth::session::clear(&self.secrets).map_err(|e| self.err(e))?;
        let _ = Jar::clear(&self.secrets, COOKIE_ACCOUNT);
        let _ = net_kit::password::clear(&self.secrets);
        // Both caches, guest and account: a guest token left behind is
        // harmless, but leaving one of a pair is confusing.
        for guest in [true, false] {
            let file = fsnz_api::token::cache_file(&self.paths, self.banner, guest);
            let _ = std::fs::remove_file(file);
        }
        Ok(dropped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unnamed_sort_reaches_the_api_unchanged() {
        assert_eq!(
            sort(&Sort::Raw("NI_SOMETHING_NEW".into())),
            "NI_SOMETHING_NEW"
        );
        assert_eq!(sort(&Sort::Relevance), fsnz_api::DEFAULT_SORT);
    }
}
