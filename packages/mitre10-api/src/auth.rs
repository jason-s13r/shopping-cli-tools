//! Signing in, which is five requests and no captcha.
//!
//! OAuth2 authorization code with PKCE against a **public** client, so there
//! is no client secret to hold:
//!
//! 1. `GET /authorizationserver/oauth/authorize` -- **priming**. Signed out
//!    this yields no code; what it does is establish the OAuth client in the
//!    session, and nothing else works until it has.
//! 2. `GET /authorizationserver/csrf` -- a token, and the `JSESSIONID` that
//!    everything after this depends on.
//! 3. `POST /authorizationserver/login` -- the password, as a form. Answers
//!    `302` whether or not it worked; the `Location` is the only evidence.
//! 4. `GET /authorizationserver/oauth/authorize` -- the same challenge again,
//!    now answering `302` with the authorization code in the `Location`.
//! 5. `POST /authorizationserver/oauth/token` -- the code plus the verifier,
//!    for an access and a refresh token.
//!
//! **Step 1 is not optional and does not look necessary.** Without it `/csrf`
//! answers `403 Invalid CORS request`, because Spring builds the allowed-origin
//! list out of the OAuth client's registration -- so "no client in this
//! session" surfaces as a CORS refusal rather than as a missing-context one.
//! The site's own storefront hits that 403 and recovers the same way.
//!
//! Steps 1 to 4 share one session cookie, so they run on a client with its own
//! jar and redirects turned **off**: following step 4 would discard the code
//! and land on the website.

use std::sync::Arc;

use base64::Engine;
use net_kit::wreq;
use sha2::{Digest, Sha256};

use crate::endpoints::{Endpoints, OAUTH_CLIENT_ID};
use crate::error::{Error, Result};
use crate::session::Session;

/// What a step did, for `--debug`. No credential is ever passed here: the
/// password, the code and the tokens are all dropped.
pub type Trace<'a> = &'a (dyn Fn(&str, &str) + Send + Sync);

/// A trace that discards everything.
pub fn no_trace(_step: &str, _detail: &str) {}

/// A PKCE pair: the secret, and the digest of it that goes out first.
///
/// The authorization code is useless without the verifier, which only holds if
/// the verifier is unpredictable -- so it comes from the system random source
/// and never from anything derived from the clock.
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl Pkce {
    pub fn generate() -> Pkce {
        let mut bytes = [0u8; 32];
        rand::fill(&mut bytes);
        Pkce::from_verifier(b64(&bytes))
    }

    pub fn from_verifier(verifier: String) -> Pkce {
        let challenge = b64(&Sha256::digest(verifier.as_bytes()));
        Pkce {
            verifier,
            challenge,
        }
    }
}

/// Base64url, no padding -- what RFC 7636 specifies.
fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// A random URL-safe value, for `state`.
fn nonce() -> String {
    let mut bytes = [0u8; 16];
    rand::fill(&mut bytes);
    b64(&bytes)
}

/// What the token endpoint answered.
#[derive(Debug, serde::Deserialize)]
pub struct Tokens {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in: Option<u64>,
}

impl Tokens {
    pub fn apply(self, session: &mut Session) {
        session.set_tokens(self.access_token, self.refresh_token, self.expires_in);
    }
}

/// Walk the whole flow and come back with tokens.
///
/// Builds its own client because the flow needs a cookie jar and this crate's
/// ordinary client is stateless.
pub async fn login(
    spec: net_kit::ClientSpec,
    endpoints: &Endpoints,
    username: &str,
    password: &str,
    trace: Trace<'_>,
) -> Result<Tokens> {
    let jar = Arc::new(wreq::cookie::Jar::default());
    let http = net_kit::http::build(spec.with_cookies(jar))
        .map_err(|e| Error::Shape(format!("building the sign-in client: {e}")))?;

    // One challenge, used twice: the priming call and the one that yields the
    // code must present the same `code_challenge` or the exchange is refused.
    let pkce = Pkce::generate();

    // Priming. Signed out there is no code to take, and that is expected --
    // the point is the client context it leaves in the session.
    if authorize(&http, endpoints, &pkce, trace).await?.is_some() {
        trace(
            "authorize",
            "already signed in; taking the code without a password",
        );
    }

    let csrf = csrf(&http, endpoints, trace).await?;
    submit_password(&http, endpoints, username, password, &csrf, trace).await?;

    let code = authorize(&http, endpoints, &pkce, trace)
        .await?
        .ok_or(Error::LoginLapsed)?;
    exchange(&http, endpoints, &code, &pkce.verifier, trace).await
}

/// Step 2: the CSRF token, and the session cookie that carries the rest.
async fn csrf(http: &wreq::Client, endpoints: &Endpoints, trace: Trace<'_>) -> Result<String> {
    let url = endpoints.auth("/csrf");
    trace("csrf", &url);
    let body: serde_json::Value = net_kit::http::json(
        "GET",
        &url,
        http.get(&url).headers(site_headers(endpoints)).send().await,
    )
    .await
    .map_err(|e| match e.status() {
        // "Invalid CORS request", which is what this endpoint says when the
        // session carries no OAuth client -- so this is the priming call
        // having failed, not a CORS bug and not a bad password. The usual
        // cause is `navigation_headers` not reaching it.
        Some(403) => Error::LoginUnavailable,
        _ => Error::Http(e),
    })?;
    body.get("token")
        .and_then(|t| t.as_str())
        .map(str::to_string)
        .ok_or_else(|| Error::Shape("the CSRF answer carried no token".into()))
}

/// Step 3: the password.
///
/// Answers `302` either way. A refusal goes back to the login page with an
/// `error` parameter, so the `Location` is the only thing that says whether
/// the credentials were accepted.
async fn submit_password(
    http: &wreq::Client,
    endpoints: &Endpoints,
    username: &str,
    password: &str,
    csrf: &str,
    trace: Trace<'_>,
) -> Result<()> {
    let url = endpoints.auth("/login");
    trace("login", &url);
    let form = [
        ("username", username),
        ("password", password),
        ("remember-me", "on"),
        ("_csrf", csrf),
        ("loginPageUri", &format!("{}/login", endpoints.site)),
    ];
    let response = http
        .post(&url)
        .headers(navigation_headers(endpoints, Some(&endpoints.site)))
        .form(&form)
        .send()
        .await
        .map_err(|source| {
            Error::Http(net_kit::HttpError::Transport {
                method: "POST",
                url: url.clone(),
                source,
            })
        })?;

    let status = response.status().as_u16();
    let location = header(&response, "location").unwrap_or_default();
    trace("login", &format!("{status} -> {}", redacted(&location)));

    if status == 429 {
        return Err(Error::RateLimited {
            retry_after: header(&response, "retry-after").and_then(|v| v.parse().ok()),
        });
    }
    // Landing anywhere carrying an error, or back on the login page at all, is
    // a refusal: the success case redirects away from it.
    if location.contains("error") || location.contains("/login") {
        return Err(Error::BadCredentials);
    }
    if !(300..400).contains(&status) {
        return Err(Error::Shape(format!(
            "the sign-in form answered {status} rather than a redirect"
        )));
    }
    Ok(())
}

/// The authorize call, which is made twice.
///
/// `None` means it redirected somewhere without a code -- the login page,
/// signed out. That is the expected answer to the priming call and a failure
/// only after the password has been accepted.
async fn authorize(
    http: &wreq::Client,
    endpoints: &Endpoints,
    pkce: &Pkce,
    trace: Trace<'_>,
) -> Result<Option<String>> {
    let url = authorize_url(endpoints, pkce);
    trace("authorize", &redacted(&url));
    let response = http
        .get(&url)
        .headers(navigation_headers(endpoints, None))
        .send()
        .await
        .map_err(|source| {
            Error::Http(net_kit::HttpError::Transport {
                method: "GET",
                url: url.clone(),
                source,
            })
        })?;

    let status = response.status().as_u16();
    let location = header(&response, "location").ok_or_else(|| {
        Error::Shape(format!(
            "the authorize endpoint answered {status} with no redirect to take a code from"
        ))
    })?;
    trace("authorize", &format!("{status} -> {}", redacted(&location)));

    Ok(code_from(&location))
}

/// The `code` parameter of a redirect, wherever in the query it sits.
pub fn code_from(location: &str) -> Option<String> {
    let query = location.split(['?', '#']).nth(1)?;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == "code" && !value.is_empty()).then(|| value.to_string())
    })
}

fn authorize_url(endpoints: &Endpoints, pkce: &Pkce) -> String {
    let params = [
        ("response_type", "code"),
        ("client_id", OAUTH_CLIENT_ID),
        ("state", &nonce()),
        ("redirect_uri", &endpoints.site),
        ("scope", ""),
        ("code_challenge", &pkce.challenge),
        ("code_challenge_method", "S256"),
    ];
    let query: Vec<String> = params
        .iter()
        .map(|(k, v)| format!("{k}={}", urlencode(v)))
        .collect();
    // `continue` with no value is what the storefront sends, and it is what
    // tells Spring's authorize endpoint to issue rather than re-prompt.
    format!(
        "{}?{}&continue",
        endpoints.auth("/oauth/authorize"),
        query.join("&")
    )
}

/// Step 5: the code and the verifier, for tokens.
pub async fn exchange(
    http: &wreq::Client,
    endpoints: &Endpoints,
    code: &str,
    verifier: &str,
    trace: Trace<'_>,
) -> Result<Tokens> {
    let url = endpoints.auth("/oauth/token");
    trace("token", &url);
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", endpoints.site.as_str()),
        ("code_verifier", verifier),
        ("client_id", OAUTH_CLIENT_ID),
    ];
    token_request(http, endpoints, &url, &form, trace).await
}

/// A new access token from the refresh token, with no password involved.
pub async fn refresh(
    http: &wreq::Client,
    endpoints: &Endpoints,
    refresh_token: &str,
    trace: Trace<'_>,
) -> Result<Tokens> {
    let url = endpoints.auth("/oauth/token");
    trace("refresh", &url);
    let form = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", OAUTH_CLIENT_ID),
    ];
    token_request(http, endpoints, &url, &form, trace).await
}

/// Give a token back to the storefront. Best-effort: a sign-out that cannot
/// reach the network still has to clear the local copy.
pub async fn revoke(
    http: &wreq::Client,
    endpoints: &Endpoints,
    token: &str,
    trace: Trace<'_>,
) -> Result<()> {
    let url = endpoints.auth("/oauth/revoke");
    trace("revoke", &url);
    let form = [("client_id", OAUTH_CLIENT_ID), ("token", token)];
    let _ = http
        .post(&url)
        .headers(site_headers(endpoints))
        .form(&form)
        .send()
        .await;
    Ok(())
}

async fn token_request(
    http: &wreq::Client,
    endpoints: &Endpoints,
    url: &str,
    form: &[(&str, &str)],
    trace: Trace<'_>,
) -> Result<Tokens> {
    let response = http
        .post(url)
        .headers(site_headers(endpoints))
        .form(form)
        .send()
        .await
        .map_err(|source| {
            Error::Http(net_kit::HttpError::Transport {
                method: "POST",
                url: url.to_string(),
                source,
            })
        })?;

    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();
    trace("token", &format!("{status}"));

    if status == 429 {
        return Err(Error::RateLimited { retry_after: None });
    }
    if !(200..300).contains(&status) {
        let refusal = classify(&body);
        // The OAuth error names which half was rejected; without it every
        // failure here reads as "sign in again", including the ones a new
        // password would not fix. Safe to trace: error bodies carry no token.
        trace("token", &describe(&body));
        return Err(refusal);
    }
    serde_json::from_str(&body).map_err(|e| Error::decode("reading the token answer", e))
}

/// The `error` and `error_description` of a refusal, for the trace.
fn describe(body: &str) -> String {
    let parsed: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    match (
        parsed.get("error").and_then(|e| e.as_str()),
        parsed.get("error_description").and_then(|d| d.as_str()),
    ) {
        (Some(kind), Some(detail)) => format!("{kind}: {detail}"),
        (Some(kind), None) => kind.to_string(),
        _ => body.chars().take(200).collect(),
    }
}

/// An OAuth error body, in the terms this crate reports.
fn classify(body: &str) -> Error {
    let parsed: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    let kind = parsed.get("error").and_then(|e| e.as_str()).unwrap_or("");
    let detail = parsed
        .get("error_description")
        .and_then(|d| d.as_str())
        .unwrap_or(body);
    match kind {
        // Covers an expired or revoked refresh token *and* a mismatched
        // verifier. Both mean this flow has to start over from a password.
        "invalid_grant" => Error::LoginLapsed,
        "invalid_client" | "unauthorized_client" => Error::Shape(format!(
            "the storefront refused the OAuth client {OAUTH_CLIENT_ID}: {detail}"
        )),
        _ => Error::Shape(format!("the token endpoint refused the exchange: {detail}")),
    }
}

/// The headers every call to this storefront needs.
///
/// `Origin` and `Referer` because the API's CORS rules answer `403` without
/// them. `Accept` because **OCC serves XML by default**: with no `Accept` it
/// answers `application/xml` with a `200`, which fails as a decode error and
/// reads as a moved schema. This is the site's own spelling; `text/plain`
/// belongs in it for the endpoints that answer a bare value.
///
/// The `Sec-Fetch-*` triplet is set because every call this crate makes is an
/// XHR, while the emulation profile's defaults describe a **top-level
/// navigation** (`document`/`navigate`/`none`). Left alone they contradict the
/// `Origin` above -- no browser sends one with `Sec-Fetch-Site: none` -- which
/// is exactly the shape a bot manager looks for.
///
/// The profile also adds `Upgrade-Insecure-Requests` and `Sec-Fetch-User`,
/// which belong to a navigation and not to this. They cannot be removed
/// through the request builder -- `wreq` merges them in at send time -- and
/// nothing has been observed to care, so they stay.
pub fn site_headers(endpoints: &Endpoints) -> wreq::header::HeaderMap {
    let mut headers = wreq::header::HeaderMap::new();
    for (name, value) in [
        ("accept", "application/json, text/plain, */*".to_string()),
        ("origin", endpoints.site.clone()),
        ("referer", format!("{}/", endpoints.site)),
        ("sec-fetch-dest", "empty".to_string()),
        ("sec-fetch-mode", "cors".to_string()),
        // The API is a different host under the same registrable domain as
        // the site, which is what `same-site` means. `none` is not a claim an
        // XHR can make at all.
        ("sec-fetch-site", "same-site".to_string()),
    ] {
        if let (Ok(name), Ok(value)) = (
            wreq::header::HeaderName::from_bytes(name.as_bytes()),
            wreq::header::HeaderValue::from_str(&value),
        ) {
            headers.insert(name, value);
        }
    }
    headers
}

/// Headers for the two steps that are page navigations rather than XHRs: the
/// authorize redirects and the login form post.
///
/// `Accept: application/json` here is fatal, and silently so. The authorize
/// call answers the same `302` to the login page either way, but the session
/// it leaves behind carries no OAuth client -- and `/csrf` then refuses that
/// session as `Invalid CORS request`, three calls later with nothing to
/// connect it to. A navigation asks for HTML.
///
/// `origin` is the form post's: a cross-site POST carries one, a GET
/// navigation carries none. The profile's `Upgrade-Insecure-Requests` and
/// `Sec-Fetch-User`, which [`site_headers`] has to tolerate, are correct here.
fn navigation_headers(endpoints: &Endpoints, origin: Option<&str>) -> wreq::header::HeaderMap {
    let mut headers = wreq::header::HeaderMap::new();
    let pairs = [
        (
            "accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8".to_string(),
        ),
        ("referer", format!("{}/", endpoints.site)),
        ("sec-fetch-dest", "document".to_string()),
        ("sec-fetch-mode", "navigate".to_string()),
        ("sec-fetch-site", "same-site".to_string()),
        ("upgrade-insecure-requests", "1".to_string()),
    ];
    for (name, value) in pairs
        .into_iter()
        .chain(origin.map(|o| ("origin", o.to_string())))
    {
        if let (Ok(name), Ok(value)) = (
            wreq::header::HeaderName::from_bytes(name.as_bytes()),
            wreq::header::HeaderValue::from_str(&value),
        ) {
            headers.insert(name, value);
        }
    }
    headers
}

fn header(response: &wreq::Response, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)?
        .to_str()
        .ok()
        .map(str::to_string)
}

/// A URL with its secrets taken out, for a trace that is safe to print.
fn redacted(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    for (i, part) in url.split('&').enumerate() {
        if i > 0 {
            out.push('&');
        }
        match part.split_once('=') {
            Some((key, _)) if matches!(key.rsplit('?').next(), Some("code" | "code_challenge")) => {
                out.push_str(key);
                out.push_str("=<redacted>");
            }
            _ => out.push_str(part),
        }
    }
    out
}

fn urlencode(value: &str) -> String {
    const UNRESERVED: &[u8] = b"-_.~";
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
    fn a_verifier_produces_the_challenge_rfc_7636_specifies() {
        // The worked example from RFC 7636 appendix B. Getting this wrong
        // fails only at the last step, with a message about the verifier.
        let pkce = Pkce::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into());
        assert_eq!(
            pkce.challenge,
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn two_generated_verifiers_differ() {
        let a = Pkce::generate();
        let b = Pkce::generate();
        assert_ne!(a.verifier, b.verifier);
        assert_ne!(a.challenge, b.challenge);
        assert!(
            a.verifier.len() >= 43,
            "RFC 7636 wants 43 characters or more"
        );
    }

    #[test]
    fn the_code_is_taken_out_of_the_redirect_wherever_it_sits() {
        assert_eq!(
            code_from("https://www.mitre10.co.nz?code=abc123&state=xyz").as_deref(),
            Some("abc123")
        );
        assert_eq!(
            code_from("https://www.mitre10.co.nz?state=xyz&code=abc123").as_deref(),
            Some("abc123")
        );
        assert_eq!(
            code_from("https://www.mitre10.co.nz?error=access_denied"),
            None
        );
        assert_eq!(code_from("https://www.mitre10.co.nz?code="), None);
        assert_eq!(code_from("https://www.mitre10.co.nz"), None);
    }

    #[test]
    fn the_authorize_url_carries_pkce_and_the_continue_the_storefront_sends() {
        let url = authorize_url(&Endpoints::defaults(), &Pkce::from_verifier("v".into()));
        assert!(url.contains("response_type=code"), "{url}");
        assert!(url.contains("client_id=mobile_android_public"), "{url}");
        assert!(url.contains("code_challenge_method=S256"), "{url}");
        assert!(url.ends_with("&continue"), "{url}");
        assert!(
            url.contains("redirect_uri=https%3A%2F%2Fwww.mitre10.co.nz"),
            "the redirect must be encoded, not raw: {url}"
        );
    }

    #[test]
    fn an_invalid_grant_is_a_lapsed_login_rather_than_a_mystery() {
        // Both an expired refresh token and a mismatched verifier arrive this
        // way, and both are fixed by signing in again.
        assert!(matches!(
            classify(r#"{"error":"invalid_grant","error_description":"code_verifier"}"#),
            Error::LoginLapsed
        ));
        assert!(matches!(
            classify(r#"{"error":"invalid_client"}"#),
            Error::Shape(_)
        ));
    }

    #[test]
    fn a_trace_never_carries_the_code_or_the_challenge() {
        // The trace goes to stderr under --debug; a code in a scrollback is a
        // credential someone else can use.
        let line = redacted(
            "https://ccapi.mitre10.co.nz/authorizationserver/oauth/authorize?\
             response_type=code&code_challenge=SECRET&state=abc",
        );
        assert!(!line.contains("SECRET"), "{line}");
        assert!(line.contains("code_challenge=<redacted>"), "{line}");
        assert!(line.contains("state=abc"), "non-secrets survive: {line}");

        let landed = redacted("https://www.mitre10.co.nz?code=SECRET&state=abc");
        assert!(!landed.contains("SECRET"), "{landed}");
    }

    #[test]
    fn the_priming_call_asks_for_html_because_asking_for_json_breaks_the_session() {
        // Both answer `302` to the login page, so this is invisible at the
        // call itself: the damage only shows up as a 403 from `/csrf` later.
        let nav = navigation_headers(&Endpoints::defaults(), None);
        let accept = nav.get("accept").expect("set").to_str().expect("ascii");
        assert!(accept.starts_with("text/html"), "{accept}");
        assert!(!accept.contains("application/json"), "{accept}");
    }

    #[test]
    fn a_navigation_is_not_dressed_up_as_an_xhr() {
        let nav = navigation_headers(&Endpoints::defaults(), None);
        for (name, want) in [
            ("sec-fetch-dest", "document"),
            ("sec-fetch-mode", "navigate"),
            ("sec-fetch-site", "same-site"),
        ] {
            assert_eq!(nav.get(name).expect(name).to_str().expect("ascii"), want);
        }
    }

    #[test]
    fn only_the_form_post_carries_an_origin() {
        // A browser sends one on a cross-site POST and none on a GET
        // navigation; sending one on both is a shape no browser produces.
        let site = Endpoints::defaults().site.clone();
        assert!(navigation_headers(&Endpoints::defaults(), None)
            .get("origin")
            .is_none());
        assert_eq!(
            navigation_headers(&Endpoints::defaults(), Some(&site))
                .get("origin")
                .expect("set")
                .to_str()
                .expect("ascii"),
            site
        );
    }

    #[test]
    fn every_request_asks_for_json_because_occ_serves_xml_by_default() {
        // Without this the storefront answers 200 with application/xml, which
        // surfaces as a decode failure and reads as a moved schema.
        let headers = site_headers(&Endpoints::defaults());
        let accept = headers.get("accept").expect("set").to_str().expect("ascii");
        assert!(accept.contains("application/json"), "{accept}");
        assert!(
            accept.contains("text/plain"),
            "postcodegroupid answers a bare number: {accept}"
        );
    }

    #[test]
    fn the_fetch_metadata_describes_an_xhr_not_a_navigation() {
        // The emulation profile defaults to document/navigate/none, which
        // contradicts the Origin alongside it: no browser sends an Origin with
        // Sec-Fetch-Site: none.
        let headers = site_headers(&Endpoints::defaults());
        assert_eq!(headers.get("sec-fetch-dest").expect("set"), "empty");
        assert_eq!(headers.get("sec-fetch-mode").expect("set"), "cors");
        assert_eq!(
            headers.get("sec-fetch-site").expect("set"),
            "same-site",
            "the API is a different host under the same registrable domain"
        );
    }

    #[test]
    fn the_cors_headers_name_the_site_rather_than_the_api() {
        // The API answers 403 "Invalid CORS request" when Origin is the API
        // host rather than the website.
        let headers = site_headers(&Endpoints::defaults());
        assert_eq!(
            headers.get("origin").expect("set"),
            "https://www.mitre10.co.nz"
        );
        assert_eq!(
            headers.get("referer").expect("set"),
            "https://www.mitre10.co.nz/"
        );
    }
}
