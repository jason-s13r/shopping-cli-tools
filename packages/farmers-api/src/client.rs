//! The storefront, and everything reached through it.
//!
//! One type over three backends, because from a caller's point of view there
//! is one shop. What it hides is the warm-up: everything under `/INTERSHOP/`
//! answers [`Error::Denied`] to a cold client, so those calls go through
//! [`Client::warm`] first and, if they are denied anyway, warm again and retry
//! **once**.
//!
//! One retry, and no backoff. The retry covers the cookie that lapsed between
//! commands, which is both the common case and the recoverable one -- a
//! browser-earned jar is good for a few minutes, not a day. Nothing beyond
//! that is worth spending a request on: a second denial means the warm-up
//! itself is not being honoured, and repeating it only spends another browser
//! launch. See [`Warmer`].

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use net_kit::wreq;
use serde::de::DeserializeOwned;

use crate::auth;
use crate::domain::{
    Account, Cart, Category, Hit, Listing, Order, Product, StoreStock, Suggestion, Variant,
    Wishlist,
};
use crate::endpoints::{query_string, Endpoints};
use crate::error::{is_challenge_page, is_deny_page, Error, Result};
use crate::extract;
use crate::search::{self, Query};
use crate::session::{Session, StoredSession};
use crate::stores;
use crate::wire;

/// Whether the account the storefront just named is the one being asked for.
///
/// The storefront's own answer first, and the stored email only as a fallback:
/// the account header does not always carry an address, and a session filed by
/// a previous sign-in remembers which one it was for. Case-insensitive, because
/// an email address is.
///
/// Unknown means no. Signing in again costs a password and a request, and
/// wrongly deciding somebody is already signed in as the account they asked
/// for would leave them looking at the wrong person's orders.
fn is_the_same_person(account: &Account, session: &Session, email: &str) -> bool {
    let wanted = email.trim().to_lowercase();
    account
        .email
        .as_deref()
        .or(session.email.as_deref())
        .is_some_and(|known| known.trim().to_lowercase() == wanted)
}

/// Whether a redirect target is the sign-in page.
///
/// Intershop sends an account page's visitor to `/login` carrying the pipeline
/// it wanted in `TargetPipeline`, so either the path or that parameter settles
/// it.
fn is_sign_in_redirect(target: &str) -> bool {
    let target = target.to_ascii_lowercase();
    target.contains("/login") || target.contains("targetpipeline")
}

/// How deep `/categories?view=tree` is asked for when nothing says otherwise.
///
/// Three is the whole of the site's own menu. The tree is one request whatever
/// the depth, but it grows fast: depth 1 is 3KB and the full tree is 2.5MB,
/// most of it the 1,354 brands.
pub const DEFAULT_DEPTH: usize = 3;

/// Where a session is filed, for a client that should keep what it earns.
///
/// Worth setting even with nothing to sign in with: the Akamai cookies a
/// warm-up buys are what let the *next* run skip its own.
pub struct SessionStore {
    pub secrets: net_kit::Secrets,
}

/// What a client needs to sign itself in again, with nobody at the keyboard.
///
/// There is no grant to spend here and nothing to renew *from*: the session is
/// a cookie, and the only way to get another is to run the sign-in form again.
/// So "renewing" is re-running the login with the typing already answered,
/// which is why this holds a password where the Mitre 10 equivalent holds a
/// refresh token.
pub struct Reauth {
    pub email: String,
    pub password: net_kit::password::Source,
    pub secrets: net_kit::Secrets,
}

/// Cookies earned somewhere this crate cannot reach, and the reason this hook
/// exists.
///
/// **An `_abck` this crate fetches for itself is no longer accepted.** Measured
/// 2026-09-18, one address, one minute, one profile: a jar harvested from a
/// real browser was served the whole category tree *through `wreq`*, and a jar
/// `wreq` earned from the same home page was answered `Access Denied` by the
/// byte-identical request. The bot manager scores where the token came from,
/// not the fingerprint of the request spending it -- so no emulation profile
/// fixes this and the warm-up has to happen in a browser.
///
/// What comes back is a cookie jar. Driving the browser is the caller's
/// business: this crate takes values, and a browser is a hundred megabytes and
/// a subprocess, neither of which belongs in a library.
pub type Warmer = Arc<
    dyn Fn() -> Pin<Box<dyn Future<Output = Result<BTreeMap<String, String>>> + Send>>
        + Send
        + Sync,
>;

pub struct Client {
    http: wreq::Client,
    endpoints: Endpoints,
    /// Replaced in place as calls absorb cookies, so one command's later calls
    /// use the session its earlier ones bought.
    session: Mutex<Session>,
    store: Option<SessionStore>,
    reauth: Option<Reauth>,
    /// Where warmth comes from, when this crate cannot earn it itself. See
    /// [`Warmer`].
    warmer: Option<Warmer>,
    /// Whether this process has fetched the home page yet. Separate from
    /// [`Session::warmed`], which asks whether there are cookies: a session
    /// restored from a previous run has those and still counts as unwarmed
    /// here, so one command never warms twice.
    warmed: AtomicBool,
    /// What Constructor.io is told this browser is called. Opaque, per run.
    client_id: String,
    debug: bool,
}

impl Client {
    pub fn new(http: wreq::Client, endpoints: Endpoints, session: Session) -> Client {
        Client {
            http,
            endpoints,
            // A restored session's cookies are worth trying, but the site may
            // have expired them; the first deny warms and retries.
            warmed: AtomicBool::new(false),
            session: Mutex::new(session),
            store: None,
            reauth: None,
            warmer: None,
            client_id: client_id(),
            debug: false,
        }
    }

    pub fn with_session_store(mut self, store: Option<SessionStore>) -> Client {
        self.store = store;
        self
    }

    /// Give this client the means to sign itself in again.
    pub fn with_reauth(mut self, reauth: Option<Reauth>) -> Client {
        self.reauth = reauth;
        self
    }

    /// Give this client somewhere to get warmth that it cannot earn itself.
    ///
    /// With one set, [`Client::warm`] asks it instead of fetching the home
    /// page. See [`Warmer`] for why that is now the only thing that works.
    pub fn with_warmer(mut self, warmer: Option<Warmer>) -> Client {
        self.warmer = warmer;
        self
    }

    /// Whether warmth can be renewed without a person.
    pub fn can_warm(&self) -> bool {
        self.warmer.is_some()
    }

    /// Whether an expired session could be replaced without a person.
    pub fn can_reauth(&self) -> bool {
        self.reauth.is_some()
    }

    pub fn with_debug(mut self, debug: bool) -> Client {
        self.debug = debug;
        self
    }

    /// Override the id sent to the search backend, for a test.
    pub fn with_client_id(mut self, id: impl Into<String>) -> Client {
        self.client_id = id.into();
        self
    }

    pub fn endpoints(&self) -> &Endpoints {
        &self.endpoints
    }

    pub fn session(&self) -> Session {
        self.session.lock().expect("session lock").clone()
    }

    /// Whether the stored cookies claim an account. A local guess;
    /// [`Client::whoami`] is the answer that counts.
    pub fn is_signed_in(&self) -> bool {
        self.session().account()
    }

    // ---- transport ----

    fn trace(&self, message: &str) {
        if self.debug {
            eprintln!("farmers-api: {message}");
        }
    }

    /// Write the session back, if this client was given somewhere to put it.
    ///
    /// Best effort, like a cache: a failed write costs the next run a warm-up,
    /// not this one its result.
    pub fn persist(&self) {
        let Some(store) = &self.store else { return };
        if let Err(e) = StoredSession::of(&self.session()).save(&store.secrets) {
            self.trace(&format!("the session could not be filed: {e}"));
        }
    }

    /// Fetch the home page, which is what earns the Akamai cookies.
    ///
    /// A plain navigation, and that is the point: it is the request a browser
    /// makes first, and the bot manager hands out `_abck` and `bm_sz` on it.
    /// The cookie it earns is *unvalidated* -- no JavaScript sensor is posted,
    /// so its flag reads `~-1~` -- and the REST API accepts that. See
    /// [`crate::http`].
    ///
    /// Idempotent within a process: the second call is free.
    ///
    /// With a [`Warmer`] set, a restored session that already carries an
    /// `_abck` is taken at its word rather than re-warmed. Without one, warmth
    /// costs a request and re-buying it is cheap insurance; with one it costs
    /// a browser launch and several seconds, and the cookies outlive a command
    /// by a good margin. The deny path below is what catches the jar that has
    /// gone stale, which is the same retry that was always there.
    pub async fn warm(&self) -> Result<()> {
        if self.warmed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        if self.warmer.is_some() && self.session().warmed() {
            self.trace("keeping the stored Akamai cookies rather than starting a browser");
            return Ok(());
        }
        match self.rewarm().await {
            // A stored `_abck` the site has since turned against is refused on
            // every request carrying it -- including this warm-up, which is
            // what makes it unrecoverable rather than merely stale. Dropping
            // the Akamai set and warming genuinely cold is the one retry that
            // works, and it costs the request the stored session was saving.
            Err(Error::Denied { .. }) => {
                self.trace("the warm-up was refused; dropping the stored Akamai cookies");
                self.session.lock().expect("session lock").forget_warmth();
                self.rewarm().await
            }
            other => other,
        }
    }

    /// Warm again, whether or not this client already did.
    ///
    /// A [`Warmer`] is asked first and its answer is final: when one is set,
    /// fetching the home page here is known not to work, so falling back to it
    /// would spend a request to fail. Without one this is the plain
    /// navigation it always was, which is what the mock-backed tests exercise
    /// and what would start working again if the site relented.
    async fn rewarm(&self) -> Result<()> {
        if let Some(warmer) = self.warmer.clone() {
            return self.warm_by_hand(&warmer).await;
        }
        let url = self.endpoints.home();
        self.trace("warming against the home page");
        let mut request = self
            .http
            .get(&url)
            .header(
                wreq::header::ACCEPT,
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            )
            .header("sec-fetch-dest", "document")
            .header("sec-fetch-mode", "navigate")
            // A cold visit, which is what this is.
            .header("sec-fetch-site", "none");
        if let Some(cookies) = self.session().header() {
            request = sensitive(request, &cookies);
        }

        let (headers, body) = net_kit::http::text("GET", &url, request.send().await)
            .await
            .map_err(|e| {
                self.warmed.store(false, Ordering::SeqCst);
                crate::error::from_http(e)
            })?;
        self.session.lock().expect("session lock").absorb(&headers);

        // The home page is gated like everything else, and refuses the same
        // way: 200, a `WAF_Deny_Page` body, and no `Set-Cookie` at all. Reading
        // the body here is what stops a refused warm-up being reported against
        // whichever call came after it.
        if is_challenge_page(&body) {
            self.warmed.store(false, Ordering::SeqCst);
            return Err(Error::Challenged);
        }
        if is_deny_page(&body) {
            self.warmed.store(false, Ordering::SeqCst);
            return Err(Error::Denied { warmed: false });
        }
        // A page that is served but hands out no `_abck` leaves nothing to make
        // the next call with, so it is a refusal whatever it looks like.
        if !self.session().warmed() {
            self.warmed.store(false, Ordering::SeqCst);
            self.trace("the home page was served but handed out no _abck");
            return Err(Error::Denied { warmed: false });
        }
        self.warmed.store(true, Ordering::SeqCst);
        let session = self.session();
        self.trace(&format!(
            "warmed, holding {} (validation flag {})",
            session.names().join(", "),
            session
                .validation()
                .map(|f| f.to_string())
                .unwrap_or_else(|| "none".into()),
        ));
        Ok(())
    }

    /// Take warmth from the [`Warmer`] and fold it into the session.
    ///
    /// Only the bot manager's cookies are taken, however many the hook hands
    /// back: a browser that has just loaded the home page also holds an
    /// anonymous `sid`, and adopting that would sign out anybody who was
    /// signed in. [`Session::rewarm`] is where that line is drawn.
    async fn warm_by_hand(&self, warmer: &Warmer) -> Result<()> {
        self.trace("asking for warmth from outside");
        let cookies = warmer().await.inspect_err(|_| {
            self.warmed.store(false, Ordering::SeqCst);
        })?;

        self.session.lock().expect("session lock").rewarm(cookies);

        if !self.session().warmed() {
            self.warmed.store(false, Ordering::SeqCst);
            self.trace("what came back carried no _abck");
            return Err(Error::Denied { warmed: false });
        }
        self.warmed.store(true, Ordering::SeqCst);
        // Filed straight away. It was expensive, and a command that fails
        // after this point should not make the next one buy it again.
        self.persist();
        let session = self.session();
        self.trace(&format!(
            "warmed from outside, holding {} (validation flag {})",
            session.names().join(", "),
            session
                .validation()
                .map(|f| f.to_string())
                .unwrap_or_else(|| "none".into()),
        ));
        Ok(())
    }

    /// A REST call, warmed and retried once if the bot manager refuses it.
    ///
    /// The retry is the whole reason this is not two lines. A deny arrives as
    /// **HTTP 200 with an HTML body**, so it cannot be told from an answer by
    /// status and has to be read off the body -- and then the only move
    /// available is a fresh warm-up. See the module docs for why it is not
    /// tried twice.
    async fn rest<T: DeserializeOwned>(&self, what: &'static str, path: &str) -> Result<T> {
        self.warm().await?;
        let body = match self.rest_once(what, path).await {
            Ok(body) => body,
            Err(Error::Denied { .. }) => {
                self.trace(&format!("{what} was denied; warming again"));
                self.rewarm().await?;
                self.rest_once(what, path).await.map_err(|e| match e {
                    // Said plainly: the second refusal is the interesting
                    // one, because it means the fingerprint is the problem
                    // rather than the cookies.
                    Error::Denied { .. } => Error::Denied { warmed: true },
                    other => other,
                })?
            }
            Err(other) => return Err(other),
        };
        serde_json::from_str(&body).map_err(|e| Error::decode(format!("reading {what}"), e))
    }

    async fn rest_once(&self, what: &'static str, path: &str) -> Result<String> {
        let url = self.endpoints.rest(path);
        let mut request = self
            .http
            .get(&url)
            // What the storefront's own scripts ask for. `*/*` alone is also
            // served, but there is no reason to look different.
            .header(
                wreq::header::ACCEPT,
                "application/json, text/javascript, */*; q=0.01",
            )
            .header("x-requested-with", "XMLHttpRequest")
            .header(wreq::header::REFERER, self.endpoints.home())
            .header("sec-fetch-dest", "empty")
            .header("sec-fetch-mode", "cors")
            .header("sec-fetch-site", "same-origin");
        if let Some(cookies) = self.session().header() {
            request = sensitive(request, &cookies);
        }

        let (headers, body) = net_kit::http::text("GET", &url, request.send().await)
            .await
            .map_err(crate::error::from_http)?;
        self.session.lock().expect("session lock").absorb(&headers);
        if is_challenge_page(&body) {
            return Err(Error::Challenged);
        }
        if is_deny_page(&body) {
            return Err(Error::Denied { warmed: false });
        }
        self.trace(&format!("read {what}"));
        Ok(body)
    }

    /// A `ViewX-` pipeline, which answers HTML.
    ///
    /// Warmed and retried exactly like a REST call. These are gated too --
    /// measured, not assumed: a cold `ViewUserAccount-AjaxHeader` is answered
    /// `403 Access Denied`, and the same call after one home-page fetch is
    /// served. The sign-in *page* is the exception that misleads, because it is
    /// served cold; the pipelines behind it are not. A client on a refused
    /// profile is denied the home page too, which the warm-up now catches
    /// rather than blaming on the call that followed it.
    async fn pipeline(
        &self,
        what: &'static str,
        action: &str,
        params: &[(String, String)],
    ) -> Result<String> {
        self.warm().await?;
        match self.pipeline_once(what, action, params).await {
            Err(Error::Denied { .. }) => {
                self.trace(&format!("{what} was denied; warming again"));
                self.rewarm().await?;
                self.pipeline_once(what, action, params)
                    .await
                    .map_err(|e| match e {
                        Error::Denied { .. } => Error::Denied { warmed: true },
                        other => other,
                    })
            }
            other => other,
        }
    }

    async fn pipeline_once(
        &self,
        what: &'static str,
        action: &str,
        params: &[(String, String)],
    ) -> Result<String> {
        let base = self.endpoints.pipeline(action);
        // A bare trailing `?` is not merely untidy: it is part of the URL the
        // bot manager scores, and no page on this site emits one.
        let url = if params.is_empty() {
            base
        } else {
            format!("{base}?{}", query_string(params))
        };
        let mut request = self
            .http
            .get(&url)
            .header(wreq::header::ACCEPT, "text/html, */*; q=0.01")
            .header("x-requested-with", "XMLHttpRequest")
            .header(wreq::header::REFERER, self.endpoints.home())
            .header("sec-fetch-dest", "empty")
            .header("sec-fetch-mode", "cors")
            .header("sec-fetch-site", "same-origin");
        if let Some(cookies) = self.session().header() {
            request = sensitive(request, &cookies);
        }

        let (headers, body) = net_kit::http::text("GET", &url, request.send().await)
            .await
            .map_err(crate::error::from_http)?;
        self.session.lock().expect("session lock").absorb(&headers);
        if is_challenge_page(&body) {
            return Err(Error::Challenged);
        }
        if is_deny_page(&body) {
            return Err(Error::Denied { warmed: false });
        }
        self.trace(&format!("read {what}"));
        Ok(body)
    }

    /// A POST to a `ViewX-` pipeline, warmed and retried like everything else
    /// under `/INTERSHOP/`.
    ///
    /// **The retry matters more here than anywhere else.** A POST is scored
    /// harder than a read: measured 2026-09-18, one jar served `cart list` and
    /// the catalogue while the same jar in the same minute was refused for
    /// `cart add`. So the basket commands are the ones that meet a refusal on
    /// warmth everything else is still happy with, and without a retry they
    /// were the only commands that could never recover -- this said it was
    /// retried, and was not.
    ///
    /// Retrying a POST is safe here because a refused one did not happen: the
    /// bot manager answers instead of the pipeline, so there is nothing to
    /// have half-applied. That is only true of a *denial*, which is why
    /// nothing else is retried.
    async fn post_pipeline(
        &self,
        what: &'static str,
        action: &str,
        params: &[(String, String)],
        form: &[(String, String)],
    ) -> Result<String> {
        self.warm().await?;
        match self.post_pipeline_once(what, action, params, form).await {
            Err(Error::Denied { .. }) => {
                self.trace(&format!("{what} was denied; warming again"));
                self.rewarm().await?;
                self.post_pipeline_once(what, action, params, form)
                    .await
                    .map_err(|e| match e {
                        Error::Denied { .. } => Error::Denied { warmed: true },
                        other => other,
                    })
            }
            other => other,
        }
    }

    /// Not routed through `net_kit::http::text`, which treats any non-2xx as a
    /// failure: these pipelines answer **302** on success and the redirect is
    /// not followed, so a 302 has to reach the caller as an answer.
    async fn post_pipeline_once(
        &self,
        what: &'static str,
        action: &str,
        params: &[(String, String)],
        form: &[(String, String)],
    ) -> Result<String> {
        let base = self.endpoints.pipeline(action);
        let url = if params.is_empty() {
            base
        } else {
            format!("{base}?{}", query_string(params))
        };

        let mut request = self
            .http
            .post(&url)
            .header(wreq::header::ACCEPT, "text/html, */*; q=0.01")
            .header("x-requested-with", "XMLHttpRequest")
            .header(wreq::header::ORIGIN, &self.endpoints.origin)
            .header(wreq::header::REFERER, self.endpoints.home())
            .header("sec-fetch-dest", "empty")
            .header("sec-fetch-mode", "cors")
            .header("sec-fetch-site", "same-origin");
        if let Some(cookies) = self.session().header() {
            request = sensitive(request, &cookies);
        }

        let response = request.form(form).send().await.map_err(|source| {
            Error::Http(net_kit::HttpError::Transport {
                method: "POST",
                url: url.clone(),
                source,
            })
        })?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let body = response.text().await.unwrap_or_default();
        self.session.lock().expect("session lock").absorb(&headers);

        if is_challenge_page(&body) {
            return Err(Error::Challenged);
        }
        if is_deny_page(&body) || (status == 403 && body.contains("Access Denied")) {
            return Err(Error::Denied { warmed: false });
        }
        if status == 429 && body.contains("cpr_chlge") {
            return Err(Error::Challenged);
        }
        // 302 is success here. Anything else outside 2xx is not.
        if !(200..400).contains(&status) {
            return Err(Error::Http(net_kit::HttpError::Status {
                method: "POST",
                url,
                status,
                detail: net_kit::error::truncate(&body, 300),
                body,
            }));
        }
        self.trace(&format!("posted {what}"));
        Ok(body)
    }

    /// A plain storefront page, as a browser would ask for it.
    ///
    /// `/cart`, `/orders` and `/wishlists` are ordinary navigations rather
    /// than XHR fragments, and the storefront serves a different thing to a
    /// request that claims otherwise.
    ///
    /// Warmed and retried exactly like [`Client::pipeline`]. Not because a
    /// page is more likely to be denied, but because a caller should not have
    /// to know which of the two it is talking to in order to predict what a
    /// refusal costs.
    async fn page(&self, what: &'static str, url: &str) -> Result<String> {
        self.warm().await?;
        match self.page_once(what, url).await {
            Err(Error::Denied { .. }) => {
                self.trace(&format!("{what} was denied; warming again"));
                self.rewarm().await?;
                self.page_once(what, url).await.map_err(|e| match e {
                    Error::Denied { .. } => Error::Denied { warmed: true },
                    other => other,
                })
            }
            other => other,
        }
    }

    async fn page_once(&self, what: &'static str, url: &str) -> Result<String> {
        let mut request = self
            .http
            .get(url)
            .header(
                wreq::header::ACCEPT,
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            )
            .header(wreq::header::REFERER, self.endpoints.home())
            .header("sec-fetch-dest", "document")
            .header("sec-fetch-mode", "navigate")
            .header("sec-fetch-site", "same-origin");
        if let Some(cookies) = self.session().header() {
            request = sensitive(request, &cookies);
        }

        // Not routed through `net_kit::http::text`, which treats any non-2xx
        // as a failure. An account page asked for by a lapsed session answers
        // **302 to `/login`**, and that redirect is the evidence -- reported as
        // a raw HTTP 302 it reads like a transport fault rather than the one
        // thing it means.
        let response = request.send().await.map_err(|source| {
            Error::Http(net_kit::HttpError::Transport {
                method: "GET",
                url: url.to_string(),
                source,
            })
        })?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let body = response.text().await.unwrap_or_default();
        self.session.lock().expect("session lock").absorb(&headers);

        if is_challenge_page(&body) {
            return Err(Error::Challenged);
        }
        if is_deny_page(&body) {
            return Err(Error::Denied { warmed: false });
        }
        if (300..400).contains(&status) {
            let to = headers
                .get(wreq::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();
            // The body carries the same target as a meta refresh when the
            // `Location` header is absent, so both are worth reading.
            if is_sign_in_redirect(&to) || is_sign_in_redirect(&body) {
                self.trace(&format!("{what} was redirected to the sign-in page"));
                return Err(self.lapsed());
            }
            return Err(Error::Shape(format!(
                "{what} was redirected somewhere unexpected: {to}"
            )));
        }
        if !(200..300).contains(&status) {
            return Err(Error::Http(net_kit::HttpError::Status {
                method: "GET",
                url: url.to_string(),
                status,
                detail: net_kit::error::truncate(&body, 300),
                body,
            }));
        }
        self.trace(&format!("read {what}"));
        Ok(body)
    }

    /// Which "you are not signed in" this is.
    ///
    /// The distinction is what the caller tells someone to do: a session that
    /// lapsed can be renewed from stored credentials without a person, while
    /// one that never existed needs a sign-in.
    fn lapsed(&self) -> Error {
        match self.is_signed_in() {
            true => Error::SessionExpired,
            false => Error::NotSignedIn,
        }
    }

    /// A Constructor.io call. No warm-up, no cookies, no bot manager: a
    /// different company's host, and none of this crate's session belongs on
    /// it.
    async fn constructor<T: DeserializeOwned>(&self, what: &'static str, url: &str) -> Result<T> {
        let request = self
            .http
            .get(url)
            .header(wreq::header::ACCEPT, "application/json")
            .header(wreq::header::ORIGIN, &self.endpoints.origin)
            .header(wreq::header::REFERER, self.endpoints.home())
            .header("sec-fetch-dest", "empty")
            .header("sec-fetch-mode", "cors")
            .header("sec-fetch-site", "cross-site");

        let body = net_kit::http::json(what_method(what), url, request.send().await)
            .await
            .map_err(crate::error::from_http)?;
        self.trace(&format!("read {what}"));
        Ok(body)
    }

    /// The session counter Constructor.io expects. Always a first visit here:
    /// a CLI has no tab to come back to.
    fn search_session(&self) -> u64 {
        1
    }

    // ---- catalogue ----

    /// One product, by SKU.
    ///
    /// Takes a master (`6867065`) or a variant (`6867065002`) and answers for
    /// whichever it was given. A master's own `sale_price` is its cheapest
    /// variant's, and it cannot be bought -- see [`Client::variants`].
    pub async fn product(&self, sku: &str) -> Result<Product> {
        let sku = sku.trim();
        if sku.is_empty() {
            return Err(Error::NoSuchProduct(sku.to_string()));
        }
        let wire: wire::WireProduct = self
            .rest("the product", &format!("/products/{sku}"))
            .await
            .map_err(|e| not_found(e, || Error::NoSuchProduct(sku.to_string())))?;
        let product = wire.into_product();
        if product.sku.is_empty() {
            return Err(Error::Shape(format!(
                "the answer for {sku} carried no SKU, so it is not a product record"
            )));
        }
        Ok(product)
    }

    /// Several products at once, each reported on its own.
    ///
    /// One request each: there is no batch endpoint. A failure on one is
    /// carried rather than thrown, so pricing a list does not stop at the first
    /// code that has been delisted.
    pub async fn products(&self, skus: &[String]) -> Vec<(String, Result<Product>)> {
        let mut out = Vec::with_capacity(skus.len());
        for sku in skus {
            out.push((sku.clone(), self.product(sku).await));
        }
        out
    }

    /// What can actually be bought under a master.
    pub async fn variants(&self, sku: &str) -> Result<Vec<Variant>> {
        let sku = sku.trim();
        let wire: wire::Elements<wire::WireVariation> = self
            .rest(
                "the variations",
                // The default page is 50, and a colour-by-size grid runs past
                // that. 200 is above the largest seen and the endpoint caps
                // rather than refusing.
                &format!("/products/{sku}/variations?amount=200"),
            )
            .await
            .map_err(|e| not_found(e, || Error::NoSuchProduct(sku.to_string())))?;
        Ok(wire
            .elements
            .into_iter()
            .filter_map(wire::WireVariation::into_variant)
            .collect())
    }

    /// The category tree, from the top.
    ///
    /// `depth` is how many levels below the root to fetch in the one request.
    /// Beware the whole tree: the `Brands` branch alone has 1,354 children and
    /// the full answer is some megabytes.
    pub async fn categories(&self, depth: usize) -> Result<Vec<Category>> {
        let wire: wire::Elements<wire::WireCategory> = self
            .rest(
                "the category tree",
                &format!("/categories?view=tree&depth={depth}"),
            )
            .await?;
        Ok(wire
            .elements
            .into_iter()
            .map(wire::WireCategory::into_category)
            .collect())
    }

    /// One category and its children.
    pub async fn category(&self, id: &str, depth: usize) -> Result<Category> {
        let id = id.trim();
        let wire: wire::WireCategory = self
            .rest(
                "the category",
                &format!("/categories/{id}?view=tree&depth={depth}"),
            )
            .await
            .map_err(|e| not_found(e, || Error::NoSuchCategory(id.to_string())))?;
        let category = wire.into_category();
        if category.id.is_empty() {
            return Err(Error::NoSuchCategory(id.to_string()));
        }
        Ok(category)
    }

    // ---- search ----

    /// Search the catalogue.
    ///
    /// Constructor.io, not Intershop -- see [`crate::search`] for why. A term
    /// that a merchandising rule claims comes back with
    /// [`Listing::redirect`] set and no hits; [`Client::find`] follows those.
    pub async fn search(&self, term: &str, query: &Query) -> Result<Listing> {
        let url = query.search(
            &self.endpoints,
            term,
            &self.client_id,
            self.search_session(),
        );
        let envelope: wire::SearchEnvelope = self.constructor("the search results", &url).await?;
        Ok(envelope.response.into_listing(query.page))
    }

    /// A category or brand listing.
    pub async fn browse(&self, group: &str, query: &Query) -> Result<Listing> {
        let url = query.browse(
            &self.endpoints,
            group,
            &self.client_id,
            self.search_session(),
        );
        let envelope: wire::SearchEnvelope = self.constructor("the browse results", &url).await?;
        Ok(envelope.response.into_listing(query.page))
    }

    /// Search, following a merchandising redirect into the category it names.
    ///
    /// `lego` is the live example: it is not searched at all, it is redirected
    /// to `/toys/lego-construction`. A caller that stopped at the redirect
    /// would report no results for one of the busiest terms on the site.
    pub async fn find(&self, term: &str, query: &Query) -> Result<Listing> {
        let listing = self.search(term, query).await?;
        let Some(redirect) = listing.redirect.clone() else {
            return Ok(listing);
        };
        // The redirect is a website path, not a category id. The tree is what
        // maps one to the other, and a miss is not a failure -- the redirect is
        // still worth handing back so a caller can say where it points.
        self.trace(&format!("{term:?} redirects to {redirect}"));
        let Some(id) = self.category_id_for_path(&redirect).await else {
            return Ok(listing);
        };
        let mut followed = self.browse(&id, query).await?;
        followed.redirect = Some(redirect);
        Ok(followed)
    }

    /// The category id the website serves a path under.
    ///
    /// Best effort by design: this walks the tree the site publishes, and a
    /// path that is a landing page rather than a category has no id to find.
    async fn category_id_for_path(&self, path: &str) -> Option<String> {
        let wanted = path.trim_matches('/').to_lowercase();
        let tree = self.categories(DEFAULT_DEPTH).await.ok()?;
        tree.iter()
            .flat_map(|c| c.flatten())
            .find(|(_, c)| c.path.as_deref().map(str::to_lowercase) == Some(wanted.clone()))
            .map(|(_, c)| c.id.clone())
    }

    /// What the site would suggest for a partial term.
    pub async fn suggest(&self, term: &str) -> Result<Vec<Suggestion>> {
        let url = search::autocomplete(
            &self.endpoints,
            term,
            &self.client_id,
            self.search_session(),
        );
        let envelope: wire::AutocompleteEnvelope =
            self.constructor("the suggestions", &url).await?;
        Ok(envelope.into_suggestions())
    }

    /// Price a page of search results.
    ///
    /// The index carries no price, so this is the join: one REST call per hit,
    /// against the *default variant* rather than the master where there is one,
    /// because a master's price is a range and its stock is a sum.
    pub async fn price_hits(&self, hits: &[Hit]) -> Vec<(Hit, Option<Product>)> {
        let mut out = Vec::with_capacity(hits.len());
        for hit in hits {
            let sku = hit.variants.first().unwrap_or(&hit.sku);
            out.push((hit.clone(), self.product(sku).await.ok()));
        }
        out
    }

    // ---- stores ----

    /// Which stores in one region have a product.
    ///
    /// One region per call: the endpoint takes a `State` and answers only for
    /// it. [`Client::stock`] is the nationwide version and costs thirteen
    /// requests.
    pub async fn stock_in(&self, sku: &str, region: &str) -> Result<Vec<StoreStock>> {
        let code = stores::region(region).ok_or_else(|| Error::NoSuchRegion(region.to_string()))?;
        let params = vec![
            ("CountryCode".to_string(), "NZ".to_string()),
            // Without this the endpoint answers only the stores that can also
            // ship, which is a different question and a shorter list.
            ("NoShippingFilter".to_string(), "true".to_string()),
            ("SKU".to_string(), sku.trim().to_string()),
            ("State".to_string(), code.to_string()),
        ];
        let html = self
            .pipeline(
                "the store stock",
                "ViewCheckoutShipping-GetAvailableStoresAjax",
                &params,
            )
            .await?;
        Ok(extract::store_stock(&html))
    }

    /// Which stores anywhere have a product.
    ///
    /// Thirteen requests, one per region, in the order [`stores::REGIONS`]
    /// lists them. A region that fails is skipped rather than failing the lot:
    /// a partial answer is worth more than none, and the caller can see which
    /// regions came back.
    pub async fn stock(&self, sku: &str) -> Result<Vec<(&'static str, Vec<StoreStock>)>> {
        let mut out = Vec::new();
        for (code, _) in stores::REGIONS {
            match self.stock_in(sku, code).await {
                Ok(stock) if stock.is_empty() => {}
                Ok(stock) => out.push((code, stock)),
                // A denial is not regional and will not fix itself by trying
                // the next twelve.
                Err(e @ Error::Denied { .. }) => return Err(e),
                Err(e) => self.trace(&format!("{code} could not be read: {e}")),
            }
        }
        Ok(out)
    }

    // ---- basket ----

    /// What is in the basket.
    ///
    /// One small request to the mini-cart fragment, not the 160KB cart page.
    /// Anonymous: a basket hangs off the session cookie, so this works signed
    /// out and the basket survives signing in.
    pub async fn cart(&self) -> Result<Cart> {
        let html = self
            .pipeline("the basket", "ViewMiniCart-Status", &[])
            .await?;
        Ok(extract::cart(&html))
    }

    /// Put something in the basket.
    ///
    /// Takes a **variant** SKU. A master has no stock and no single price, and
    /// the storefront answers an add for one by doing nothing -- so a caller
    /// holding a search hit wants [`Hit::variants`] first.
    ///
    /// `options` are the variation axes as `/products/{sku}` names them:
    /// `[("Colour-DisplayName", "Grey"), ("Size-DisplayName", "L-XL")]`. The
    /// site's own form sends them and this sends them too, though the SKU
    /// alone has also been observed to work.
    ///
    /// **Sent as a GET, alone among the things that change something.**
    /// `ViewExpressShop-AddProduct` is the one pipeline on this storefront
    /// whose POST the bot manager refuses: measured 2026-09-18, one jar in one
    /// minute was answered `403 Access Denied` for this POST and served for
    /// POSTs to `ViewCart-Dispatch` and `ViewWishlist-AddItems`, and served
    /// for the *same* add asked for as a GET. Add-to-cart is the endpoint
    /// retailers guard hardest, and it is guarded by method here. The pipeline
    /// itself does not care -- Intershop dispatches on the parameters, not the
    /// verb, and the GET returns the same mini-cart fragment and moves the
    /// same line count.
    ///
    /// A browser's POST is served, so the alternative was to drive one for
    /// every add. That is ten seconds and a subprocess to send a request the
    /// site already answers.
    pub async fn cart_add(
        &self,
        sku: &str,
        quantity: i64,
        options: &[(String, String)],
    ) -> Result<Cart> {
        let sku = sku.trim();
        let quantity = quantity.max(1);
        let mut params: Vec<(String, String)> = vec![("SKU".into(), sku.to_string())];
        for (axis, value) in options {
            params.push((format!("VariationAttribute_{axis}"), value.clone()));
        }
        // Keyed by SKU, which is how the product page's own form names it.
        params.push((format!("Quantity_{sku}"), quantity.to_string()));
        // Without this the pipeline adds the item and then redirects to the
        // cart page instead of answering with the mini-cart fragment.
        params.push(("addToCartBehavior".into(), "expresscart".into()));

        // The retry inside `pipeline` cannot double-add. A denial is the bot
        // manager answering in the pipeline's place, so the add did not
        // happen -- which is worth saying out loud here, because this is the
        // one retried call that is not idempotent.
        let html = self
            .pipeline("the basket add", "ViewExpressShop-AddProduct", &params)
            .await?;
        // The answer *is* the new mini-cart, so there is no second request to
        // find out what happened.
        Ok(extract::cart(&html))
    }

    /// Take a line out, by the line item id [`Cart`] carries.
    ///
    /// Not the SKU: the same product can sit on two lines, and one of them is
    /// the one being removed.
    pub async fn cart_remove(&self, line: &str) -> Result<Cart> {
        let params = vec![("RemovePLI".to_string(), line.trim().to_string())];
        let html = self
            .pipeline(
                "the basket remove",
                "ViewMiniCart-RemoveItemFromMinicart",
                &params,
            )
            .await?;
        // This one answers the fragment directly, and needs no form token --
        // it is what the mini-cart's own remove button calls.
        Ok(extract::cart(&html))
    }

    /// Set a line to an exact quantity, or apply a promotion code, or both.
    ///
    /// The one basket call that is a **two-step**: the cart page mints a
    /// `SynchronizerToken` and this pipeline refuses the post without it. The
    /// other three need no token, which is why only this one costs two
    /// requests.
    ///
    /// Quantities are sent for every line, not just the changed one, because
    /// the form posts the whole cart and a line it does not mention is left
    /// alone rather than zeroed -- but sending them all is what the page does
    /// and is one fewer thing to be wrong about.
    pub async fn cart_update(
        &self,
        quantities: &[(String, i64)],
        promotion: Option<&str>,
    ) -> Result<Cart> {
        let (token, _) = self.cart_page_token().await?;
        let mut form: Vec<(String, String)> = vec![
            ("SynchronizerToken".into(), token),
            ("submitval".into(), String::new()),
        ];
        for (line, quantity) in quantities {
            form.push((format!("Quantity_{line}"), quantity.max(&0).to_string()));
        }
        form.push((
            "promotionCode".into(),
            promotion.unwrap_or_default().to_string(),
        ));
        // The submit button's name, which is what the pipeline dispatches on.
        form.push(("update".into(), String::new()));

        self.post_pipeline("the basket update", "ViewCart-Dispatch", &[], &form)
            .await?;
        // The post answers 302 to the cart page, so the basket is read back
        // rather than parsed out of the redirect.
        self.cart().await
    }

    /// The cart page's form token, and the page it came from.
    async fn cart_page_token(&self) -> Result<(String, String)> {
        let url = self.endpoints.cart_page();
        let html = self.page("the cart page", &url).await?;
        let token = extract::synchronizer_token(&html).ok_or_else(|| Error::NotInPage {
            what: "the cart form token".into(),
            detail: ", so the basket cannot be updated".into(),
        })?;
        Ok((token, html))
    }

    // ---- saved lists ----

    /// The saved lists. Needs an account.
    ///
    /// Farmers keeps *several* lists per account rather than one, and a plain
    /// add goes to whichever is marked preferred.
    pub async fn wishlists(&self) -> Result<Vec<Wishlist>> {
        self.require_account().await?;
        let html = self
            .page("the wish lists", &self.endpoints.wishlists_page())
            .await?;
        if extract::says_empty(&html, "wish lists") {
            return Ok(Vec::new());
        }
        Ok(extract::wishlists(&html))
    }

    /// Save a product to the preferred list. Needs an account.
    pub async fn wishlist_add(&self, sku: &str) -> Result<()> {
        self.require_account().await?;
        let sku = sku.trim();
        let form: Vec<(String, String)> = vec![
            ("SKU".into(), sku.to_string()),
            (format!("Quantity_{sku}"), "1".into()),
            ("addToCartBehavior".into(), "expresscart".into()),
            ("addToWishlistProduct".into(), String::new()),
        ];
        let params = vec![("AjaxRequestMarker".to_string(), "true".to_string())];
        let html = self
            .post_pipeline("the wish list add", "ViewWishlist-AddItems", &params, &form)
            .await?;

        // Signed out, this pipeline answers 200 with a link to the sign-in
        // page rather than an error -- so a session that lapsed between the
        // check above and here reads as success unless this is looked for.
        if html.contains("Please login") || html.contains("/login?") {
            return Err(Error::SessionExpired);
        }
        Ok(())
    }

    // ---- orders ----

    /// Past orders. Needs an account.
    pub async fn orders(&self) -> Result<Vec<Order>> {
        self.require_account().await?;
        let html = self
            .page("the order history", &self.endpoints.orders_page())
            .await?;
        if extract::says_empty(&html, "orders") {
            return Ok(Vec::new());
        }
        Ok(extract::orders(&html))
    }

    /// Refuse an account-only call before it is made, signing in again first
    /// where that is possible.
    ///
    /// Worth doing locally: these pipelines answer a signed-out request with
    /// the sign-in page and a 200, so an unchecked call reads as an empty
    /// order history rather than as "you are not signed in".
    async fn require_account(&self) -> Result<()> {
        if self.is_signed_in() {
            return Ok(());
        }
        if self.reauth.is_some() {
            self.renew().await?;
            return Ok(());
        }
        Err(Error::NotSignedIn)
    }

    // ---- account ----

    /// Who the storefront thinks is asking.
    ///
    /// The authoritative answer, against [`Client::is_signed_in`]'s guess: it
    /// asks the site rather than reading the cookie jar, so it catches a
    /// session the site has expired underneath us.
    pub async fn whoami(&self) -> Result<Account> {
        let html = self
            .pipeline("the account header", "ViewUserAccount-AjaxHeader", &[])
            .await?;
        Ok(extract::account(&html))
    }

    /// The account this session already speaks for, if it is the one being
    /// asked for and the storefront still agrees.
    ///
    /// Costs nothing when there is no grant to check, and one request when
    /// there is. A caller about to ask somebody for a password should ask this
    /// first -- being signed in already is not worth typing for.
    pub async fn signed_in_as(&self, email: &str) -> Result<Option<Account>> {
        if !self.is_signed_in() {
            return Ok(None);
        }
        self.warm().await?;
        // A denial is not an answer to "is this session good", so it is
        // propagated rather than swallowed -- a sign-in would be refused in
        // exactly the same way, and spending a password to find that out tells
        // the wrong story about why.
        let account = self.whoami().await?;
        Ok(
            match account.signed_in && is_the_same_person(&account, &self.session(), email) {
                true => Some(account),
                false => None,
            },
        )
    }

    /// Sign in, and keep the session.
    ///
    /// **A session that is already good is left alone.** Signing in again is
    /// what someone does when they are not sure they are signed in, and
    /// spending a password to replace a working session is the wrong answer to
    /// that. See [`Client::signed_in_as`].
    pub async fn login(&self, email: &str, password: &str) -> Result<Account> {
        if let Some(account) = self.signed_in_as(email).await? {
            self.trace("already signed in as this account; leaving the session alone");
            return Ok(account);
        }
        // Warmed first: the sign-in page is not gated, but arriving with the
        // cookies of a visit is what the site's own form does, and it saves the
        // POST looking like the first request of the day.
        self.warm().await?;

        let debug = self.debug;
        let session = auth::login(
            &self.http,
            &self.endpoints,
            // Never the current session: Intershop answers `/login` with a
            // redirect to `/account` for one that still carries a grant, and
            // an expired account cookie sent with the form leaves the failure
            // looking like a bad password.
            self.anonymous(),
            email,
            password,
            &move |step: &str| {
                if debug {
                    eprintln!("farmers-api: {step}");
                }
            },
        )
        .await?;

        *self.session.lock().expect("session lock") = session;
        self.persist();
        // Asked rather than assumed: the cookie says a sign-in happened, and
        // this says whose.
        let account = self.whoami().await.unwrap_or_else(|_| Account {
            signed_in: true,
            email: Some(email.trim().to_string()),
            ..Default::default()
        });
        Ok(account)
    }

    /// Sign in again from the stored credentials, and keep what it produces.
    ///
    /// The scheduled counterpart to [`Client::login`], and the reason a
    /// password is kept at all. It renews nothing -- there is no grant here --
    /// it runs the sign-in form again.
    pub async fn renew(&self) -> Result<Account> {
        let reauth = self.reauth.as_ref().ok_or(Error::NotSignedIn)?;
        let password = reauth.password.password().await?;
        self.trace(&format!(
            "signing in again from {}",
            reauth.password.describe()
        ));
        // Deliberately not the current session: sending an expired account
        // cookie along with the form leaves the failure looking like a bad
        // password.
        let session = auth::login(
            &self.http,
            &self.endpoints,
            self.anonymous(),
            &reauth.email,
            &password,
            &|step: &str| self.trace(step),
        )
        .await?;

        StoredSession::of(&session).save(&reauth.secrets)?;
        *self.session.lock().expect("session lock") = session;
        self.whoami().await
    }

    /// Whether the storefront still recognises the stored session, at the cost
    /// of one request.
    ///
    /// **A refusal by the bot manager is propagated, not reported as a lapsed
    /// session.** That distinction is the whole point of this method here.
    /// Signing in again would be refused in exactly the same way, so reading a
    /// denial as "your session expired" would spend a password to learn
    /// nothing and tell someone the wrong story about why.
    pub async fn verify(&self) -> Result<bool> {
        if !self.is_signed_in() {
            return Ok(false);
        }
        match self.whoami().await {
            Ok(account) => Ok(account.signed_in),
            Err(e) if e.is_lapsed() => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// The warmed cookies without the account ones, for a sign-in that should
    /// not carry a stale session into the form.
    fn anonymous(&self) -> Session {
        let mut session = self.session();
        session.sign_out();
        session
    }

    /// Give the session back and forget it.
    ///
    /// The Akamai cookies are kept: they were bought by a request, they say
    /// nothing about the account, and throwing them away makes the next
    /// command pay for them again.
    pub async fn sign_out(&self) -> Result<()> {
        if !self.is_signed_in() {
            return Err(Error::NotSignedIn);
        }
        // Best effort. The cookies are dropped locally either way, so a
        // storefront that does not answer cannot leave this tool believing it
        // is still signed in.
        if let Err(e) = self
            .pipeline("the sign-out", "ViewUserAccount-Logout", &[])
            .await
        {
            self.trace(&format!("the sign-out page did not answer: {e}"));
        }
        self.session.lock().expect("session lock").sign_out();
        self.persist();
        Ok(())
    }

    /// Adopt a session obtained elsewhere, and file it.
    pub fn adopt(&self, session: Session) {
        *self.session.lock().expect("session lock") = session;
        self.persist();
    }
}

/// Attach the session, marked so the value never reaches a log.
fn sensitive(request: wreq::RequestBuilder, cookies: &str) -> wreq::RequestBuilder {
    match wreq::header::HeaderValue::from_str(cookies) {
        Ok(mut value) => {
            value.set_sensitive(true);
            request.header(wreq::header::COOKIE, value)
        }
        Err(_) => request,
    }
}

/// Every call this crate makes is a GET; `net_kit` wants the verb as a
/// `&'static str` for its error messages.
fn what_method(_what: &'static str) -> &'static str {
    "GET"
}

/// Turn an upstream 404 into the crate's own "no such thing".
///
/// Only a 404: this API answers an unknown *path* with an honest 404 and an
/// unknown *parameter* by ignoring it, so anything else is a real failure and
/// must not be reported as a missing product.
fn not_found(error: Error, missing: impl Fn() -> Error) -> Error {
    match &error {
        Error::Http(e) if e.status() == Some(404) => missing(),
        _ => error,
    }
}

/// An opaque id for the search backend to count visits with.
///
/// Constructor.io requires one and uses it for analytics. This is not a
/// credential and nothing is authorised by it, so it is generated from the
/// clock rather than pulling in a random-number crate -- and generated fresh
/// per run rather than persisted, because a stable one would be a tracking id
/// this tool has no reason to keep.
fn client_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // Shaped like the UUID the site's own script generates. The service does
    // not check the shape, but there is no reason to look different.
    format!(
        "{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}",
        (nanos >> 96) as u32 ^ nanos as u32,
        (nanos >> 80) as u16,
        (nanos >> 64) as u16 & 0x0fff,
        (nanos >> 48) as u16 & 0x0fff,
        nanos as u64 & 0xffff_ffff_ffff,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_product_is_named_only_when_the_site_said_404() {
        // An unknown path 404s honestly here, but an unknown *parameter* is
        // ignored -- so anything other than a 404 is a real failure and
        // reporting it as "no such product" would hide it.
        let four_oh_four = Error::Http(net_kit::HttpError::Status {
            method: "GET",
            url: "https://www.farmers.co.nz/x".into(),
            status: 404,
            detail: String::new(),
            body: String::new(),
        });
        assert!(matches!(
            not_found(four_oh_four, || Error::NoSuchProduct("x".into())),
            Error::NoSuchProduct(_)
        ));

        let server_error = Error::Http(net_kit::HttpError::Status {
            method: "GET",
            url: "https://www.farmers.co.nz/x".into(),
            status: 500,
            detail: String::new(),
            body: String::new(),
        });
        assert!(matches!(
            not_found(server_error, || Error::NoSuchProduct("x".into())),
            Error::Http(_)
        ));
    }

    #[test]
    fn a_denial_is_not_mistaken_for_a_missing_product() {
        // It arrives as a 200, so it never reaches `not_found` -- but if the
        // classification ever moved, calling a denial "no such product" would
        // send someone hunting for a code that is fine.
        assert!(matches!(
            not_found(Error::Denied { warmed: false }, || Error::NoSuchProduct(
                "x".into()
            )),
            Error::Denied { .. }
        ));
    }

    #[test]
    fn a_client_id_looks_like_the_one_the_site_generates() {
        let id = client_id();
        assert_eq!(id.len(), 36, "{id}");
        assert_eq!(id.matches('-').count(), 4, "{id}");
        assert!(
            id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
            "{id}"
        );
    }

    #[test]
    fn a_fresh_client_has_not_warmed_even_with_a_restored_session() {
        // The cookies are worth trying, but the site may have expired them --
        // so the flag has to track this process, not the jar.
        let http = net_kit::http::build(crate::http::client_spec()).expect("a client");
        let session = Session::from_cookies(
            [("_abck".to_string(), "ABC~-1~x".to_string())]
                .into_iter()
                .collect(),
        );
        let client = Client::new(http, Endpoints::defaults(), session);
        assert!(client.session().warmed(), "the cookies are there");
        assert!(
            !client.warmed.load(Ordering::SeqCst),
            "this process has not"
        );
    }
}
