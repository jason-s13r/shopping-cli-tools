//! The client against a mock storefront.
//!
//! What these cover is the behaviour that cannot be unit tested because it is
//! about *sequences* of requests: that a cold call warms first, that a denial
//! is recognised through three different status codes and retried exactly
//! once, and that signing in is two requests where the second needs the
//! first's token.
//!
//! The live site cannot be used for any of this. Its bot protection only
//! admits a client whose cookies a browser earned, and those lapse within
//! minutes -- so a suite that talked to it would need a browser per run and
//! would still fail intermittently.

use std::collections::BTreeMap;

use farmers_api::{Client, Endpoints, Error, Query, Session, Warmer};
use wiremock::matchers::{body_string_contains, method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// Akamai's refusal, as the REST API serves it: a success with an HTML body.
const DENY_BODY: &str = "<HTML><HEAD><TITLE>Access Denied</TITLE></HEAD><BODY>\
<H1>Access Denied</H1>WAF_Deny_Page</BODY></HTML>";

const PRODUCT: &str = r#"{
    "sku": "6867065002",
    "productName": "Chisel Fleece Robe, Charcoal",
    "manufacturer": "Chisel",
    "inStock": true,
    "availableStock": 31,
    "productMasterSKU": "6867065",
    "listPrice": { "value": 89.99, "currency": "NZD" },
    "salePrice": { "value": 69.99, "currency": "NZD" }
}"#;

/// Akamai's interstitial challenge: a 200 whose body is neither the deny page
/// nor an answer. Shaped after a live capture of `/orders` on 2026-09-18, with
/// the sensor's own path replaced -- it identified that session, and nothing
/// here reads it. The two marker classes are what detection turns on.
const CHALLENGE_BODY: &str = r#"<!DOCTYPE html><html><body>
<script type="text/javascript" src="/sensor/path/redacted"></script>
<div id="sec-if-cpt-container" role="main" style="display: none">
  <div class="behavioral-content"><div id="sec-bc-tile-container"></div>
  <div class="scf-akamai-logo-sec-abc">Powered and protected by Akamai</div></div>
</div></body></html>"#;

/// The REST base, with the locale as the matrix parameter Intershop wants.
fn rest_path(rest: &str) -> String {
    format!("/INTERSHOP/rest/WFS/Farmers-Shop-Site/-;loc=en_NZ{rest}")
}

fn pipeline_path(action: &str) -> String {
    format!("/INTERSHOP/web/WFS/Farmers-Shop-Site/en_NZ/-/NZD/{action}")
}

/// A mock that hands out the cookies a warm-up earns.
async fn mount_home(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html>the storefront</html>")
                .append_header("set-cookie", "_abck=ABC~-1~xyz; Path=/")
                .append_header("set-cookie", "bm_sz=zzz; Path=/"),
        )
        .mount(server)
        .await;
}

fn client(server: &MockServer) -> Client {
    let http = net_kit::http::build(farmers_api::client_spec()).expect("a client");
    let endpoints = Endpoints::defaults()
        .with_origin(server.uri())
        .with_search(server.uri());
    Client::new(http, endpoints, Session::default())
}

#[tokio::test]
async fn a_cold_client_warms_before_its_first_rest_call() {
    // The whole reason `warm` exists: a REST call made without one is denied,
    // and the denial is a 200 that no status check would catch.
    let server = MockServer::start().await;
    mount_home(&server).await;
    Mock::given(method("GET"))
        .and(path(rest_path("/products/6867065002")))
        .respond_with(ResponseTemplate::new(200).set_body_string(PRODUCT))
        .mount(&server)
        .await;

    let client = client(&server);
    let product = client.product("6867065002").await.expect("a product");
    assert_eq!(product.sku, "6867065002");
    assert_eq!(product.saving().unwrap().display(), "$20.00");

    let requests = server.received_requests().await.expect("recorded");
    assert_eq!(requests.len(), 2, "one warm, one call");
    assert_eq!(requests[0].url.path(), "/", "the warm comes first");
    // The cookies the warm earned have to be on the call that follows.
    let sent = requests[1]
        .headers
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(sent.contains("_abck=ABC~-1~xyz"), "{sent}");
}

#[tokio::test]
async fn one_command_warms_once_however_many_calls_it_makes() {
    // Warming per call would triple the request count against a host that
    // escalates on volume, which is the thing to avoid here above all.
    let server = MockServer::start().await;
    mount_home(&server).await;
    Mock::given(method("GET"))
        .and(path(rest_path("/products/6867065002")))
        .respond_with(ResponseTemplate::new(200).set_body_string(PRODUCT))
        .mount(&server)
        .await;

    let client = client(&server);
    for _ in 0..3 {
        client.product("6867065002").await.expect("a product");
    }
    let warms = server
        .received_requests()
        .await
        .expect("recorded")
        .iter()
        .filter(|r| r.url.path() == "/")
        .count();
    assert_eq!(warms, 1);
}

#[tokio::test]
async fn a_denial_is_warmed_through_once_and_then_given_up_on() {
    // Exactly once. Retrying harder is what caused the denial, so a loop here
    // would dig the hole it is trying to climb out of.
    let server = MockServer::start().await;
    mount_home(&server).await;
    Mock::given(method("GET"))
        .and(path(rest_path("/products/6867065002")))
        .respond_with(ResponseTemplate::new(200).set_body_string(DENY_BODY))
        .mount(&server)
        .await;

    let client = client(&server);
    let error = client.product("6867065002").await.expect_err("denied");
    assert!(
        matches!(error, Error::Denied { warmed: true }),
        "the second refusal is the one worth reporting: {error}"
    );

    let paths: Vec<String> = server
        .received_requests()
        .await
        .expect("recorded")
        .iter()
        .map(|r| r.url.path().to_string())
        .collect();
    assert_eq!(paths.len(), 4, "warm, call, warm, call: {paths:?}");
    assert_eq!(paths[2], "/", "it warmed again before retrying");
}

#[tokio::test]
async fn a_denial_that_clears_on_the_retry_is_not_reported_at_all() {
    // The case the retry exists for: a cookie that expired between commands.
    let server = MockServer::start().await;
    mount_home(&server).await;
    Mock::given(method("GET"))
        .and(path(rest_path("/products/6867065002")))
        .respond_with(ResponseTemplate::new(200).set_body_string(DENY_BODY))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(rest_path("/products/6867065002")))
        .respond_with(ResponseTemplate::new(200).set_body_string(PRODUCT))
        .mount(&server)
        .await;

    let client = client(&server);
    let product = client
        .product("6867065002")
        .await
        .expect("the retry worked");
    assert_eq!(product.sku, "6867065002");
}

#[tokio::test]
async fn all_three_shapes_of_refusal_are_recognised_as_refusals() {
    // A 200 with an HTML body, a 403, and a 429 that is not a rate limit.
    // Read by status alone, the first is a decode failure and the last is a
    // reason to sleep; both readings are wrong.
    for (status, body) in [
        (200, DENY_BODY),
        (403, DENY_BODY),
        (429, r#"{"cpr_chlge":"true","t":"1"}"#),
    ] {
        let server = MockServer::start().await;
        mount_home(&server).await;
        Mock::given(method("GET"))
            .and(path(rest_path("/products/6867065002")))
            .respond_with(ResponseTemplate::new(status).set_body_string(body))
            .mount(&server)
            .await;

        let error = client(&server)
            .product("6867065002")
            .await
            .expect_err("refused");
        assert!(error.is_denied(), "HTTP {status} read as: {error}");
        assert_eq!(
            net_kit::Fault::auth(&error),
            None,
            "HTTP {status} must not send anyone to re-enter a password"
        );
    }
}

#[tokio::test]
async fn an_unknown_product_is_named_rather_than_reported_as_a_status() {
    let server = MockServer::start().await;
    mount_home(&server).await;
    Mock::given(method("GET"))
        .and(path(rest_path("/products/nope")))
        .respond_with(ResponseTemplate::new(404).set_body_string("{}"))
        .mount(&server)
        .await;

    let error = client(&server).product("nope").await.expect_err("404");
    assert!(matches!(error, Error::NoSuchProduct(sku) if sku == "nope"),);
}

#[tokio::test]
async fn per_store_stock_is_asked_for_one_region_at_a_time_and_scraped() {
    let server = MockServer::start().await;
    mount_home(&server).await;
    Mock::given(method("GET"))
        .and(path(pipeline_path("ViewCheckoutShipping-GetAvailableStoresAjax")))
        .and(query_param("State", "AUK"))
        .and(query_param("SKU", "6867065002"))
        // Without this the endpoint answers only stores that can also ship.
        .and(query_param("NoShippingFilter", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r##"<div class="panel-heading"><div class="store__name"><span>Newmarket</span></div>
                <div class="store-stock">In Stock</div></div>
                <div class="panel-collapse"><div class="store-details">309 Broadway</div></div>
                <div class="panel-heading"><div class="store__name"><span>Albany</span></div>
                <div class="store-stock">Not In Stock</div></div>
                <div class="panel-collapse"><div class="store-details">Don McKinnon Drive</div></div>"##,
        ))
        .mount(&server)
        .await;

    // Asked for by name, not by code -- that is what someone has at a keyboard.
    let stock = client(&server)
        .stock_in("6867065002", "Auckland")
        .await
        .expect("stock");
    assert_eq!(stock.len(), 2);
    assert!(stock[0].available());
    assert!(!stock[1].available());
    assert_eq!(
        stock[1].store.address.as_deref(),
        Some("Don McKinnon Drive")
    );
}

#[tokio::test]
async fn a_region_that_does_not_exist_fails_before_any_request_is_made() {
    // The site has no store there, so asking would spend a request on a
    // guaranteed empty answer -- against a host that counts requests.
    let server = MockServer::start().await;
    let error = client(&server)
        .stock_in("6867065002", "Tasman")
        .await
        .expect_err("no such region");
    assert!(matches!(error, Error::NoSuchRegion(r) if r == "Tasman"));
    assert!(server
        .received_requests()
        .await
        .expect("recorded")
        .is_empty());
}

#[tokio::test]
async fn signing_in_carries_the_form_token_from_the_page_that_minted_it() {
    let server = MockServer::start().await;
    mount_home(&server).await;
    Mock::given(method("GET"))
        .and(path("/login"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"<form name="LoginUserForm" method="post">
               <input type="hidden" name="SynchronizerToken" value="a-minted-token"/>
               </form>"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(pipeline_path("ViewUserAccount-ProcessLogin")))
        .and(body_string_contains("SynchronizerToken=a-minted-token"))
        .and(body_string_contains(
            "ShopLoginForm_Login=shopper%40example.invalid",
        ))
        // Intershop dispatches on the submit button's name; without it the
        // pipeline renders the page back rather than signing anyone in.
        .and(body_string_contains("login=Login"))
        .respond_with(
            // The 302 *is* the success, and the cookie rides on it.
            ResponseTemplate::new(302)
                .append_header("location", "/account?NotifyCart=")
                .append_header(
                    "set-cookie",
                    "__Host-SecureSessionID-paccount=signed-in; Path=/",
                ),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(pipeline_path("ViewUserAccount-AjaxHeader")))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"<span class="my-account-logged-in"></span><script>
               DDTrackingEvents.identityUserEvent('shopper@example.invalid','Ada','Lovelace')
               </script>"#,
        ))
        .mount(&server)
        .await;

    let client = client(&server);
    let account = client
        .login("shopper@example.invalid", "not-a-real-password")
        .await
        .expect("signed in");
    assert!(account.signed_in);
    assert_eq!(account.name().as_deref(), Some("Ada Lovelace"));
    assert!(client.is_signed_in(), "the cookie says so too");
}

#[tokio::test]
async fn a_refused_password_is_reported_in_the_sites_own_words() {
    // Intershop answers 200 and re-renders the page rather than using a status
    // code, so the page is the only place the reason exists.
    let server = MockServer::start().await;
    mount_home(&server).await;
    Mock::given(method("GET"))
        .and(path("/login"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"<input name="SynchronizerToken" value="a-minted-token"/>"#),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(pipeline_path("ViewUserAccount-ProcessLogin")))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"<div class="error-message">The email address or password is incorrect.</div>"#,
        ))
        .mount(&server)
        .await;

    let error = client(&server)
        .login("shopper@example.invalid", "not-a-real-password")
        .await
        .expect_err("refused");
    assert!(error.needs_login(), "{error}");
    assert!(
        error.to_string().contains("password is incorrect"),
        "{error}"
    );
}

#[tokio::test]
async fn credentials_are_never_posted_into_a_page_that_is_not_the_sign_in_form() {
    // A bot check served in place of the sign-in page has no token in it.
    // Posting a password into that would be worse than failing.
    let server = MockServer::start().await;
    mount_home(&server).await;
    Mock::given(method("GET"))
        .and(path("/login"))
        .respond_with(ResponseTemplate::new(200).set_body_string(DENY_BODY))
        .mount(&server)
        .await;

    let error = client(&server)
        .login("shopper@example.invalid", "not-a-real-password")
        .await
        .expect_err("refused");
    assert!(error.is_denied(), "{error}");

    let posts = server
        .received_requests()
        .await
        .expect("recorded")
        .iter()
        .filter(|r: &&Request| r.method == wiremock::http::Method::POST)
        .count();
    assert_eq!(posts, 0, "nothing was posted");
}

#[tokio::test]
async fn search_goes_to_the_index_and_never_to_the_rest_api() {
    // `/products?searchTerm=` is accepted and silently ignored -- it answers
    // the whole catalogue with a plausible total. A regression that routed
    // search there would look like it worked.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/fleece%20robe"))
        .and(query_param("key", farmers_api::CONSTRUCTOR_KEY))
        .and(query_param("page", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{ "response": { "total_num_results": 20, "results": [
                { "value": "Chisel Fleece Robe", "data": {
                    "id": "6867065", "sku": ["6867065", "6867065002"],
                    "manufacturername": "Chisel" } }
            ] } }"#,
        ))
        .mount(&server)
        .await;

    // Page 0 here, page 1 on the wire: the service counts from one.
    let listing = client(&server)
        .search("fleece robe", &Query::new().page(0))
        .await
        .expect("results");
    assert_eq!(listing.total, 20);
    assert_eq!(listing.hits[0].variants, vec!["6867065002".to_string()]);

    let paths: Vec<String> = server
        .received_requests()
        .await
        .expect("recorded")
        .iter()
        .map(|r| r.url.path().to_string())
        .collect();
    assert!(
        !paths.iter().any(|p| p.contains("/INTERSHOP/")),
        "search must not touch the storefront at all: {paths:?}"
    );
    assert!(!paths.contains(&"/".to_string()), "and needs no warm-up");
}

#[tokio::test]
async fn a_merchandising_redirect_is_followed_into_the_category_it_names() {
    // `lego` is not searched, it is redirected. Stopping at the redirect
    // reports "no results" for one of the busiest terms on the site.
    let server = MockServer::start().await;
    mount_home(&server).await;
    Mock::given(method("GET"))
        .and(path("/search/lego"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{ "response": { "total_num_results": 0, "results": [],
                 "redirect": { "data": { "url": "/toys/lego-construction" } } } }"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(rest_path("/categories")))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{ "elements": [ { "id": "51-07", "name": "Toys",
                 "attributes": [ { "name": "URLRewrite", "value": "toys" } ],
                 "subCategories": [ { "id": "51-0701", "name": "Lego",
                   "attributes": [ { "name": "URLRewrite", "value": "toys/lego-construction" } ] } ] } ] }"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/browse/group_id/51-0701"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{ "response": { "total_num_results": 3, "results": [
                 { "value": "Lego City", "data": { "id": "1234567" } } ] } }"#,
        ))
        .mount(&server)
        .await;

    let listing = client(&server)
        .find("lego", &Query::new())
        .await
        .expect("results");
    assert_eq!(listing.total, 3, "the category's products, not zero");
    assert_eq!(listing.hits[0].name, "Lego City");
    assert_eq!(
        listing.redirect.as_deref(),
        Some("/toys/lego-construction"),
        "and it still says where it went"
    );
}

#[tokio::test]
async fn signing_out_keeps_the_warm_up_it_did_not_pay_for() {
    // The Akamai cookies were bought by a request and say nothing about the
    // account. Dropping them makes the next command pay for them again,
    // against a host that counts requests.
    let server = MockServer::start().await;
    mount_home(&server).await;
    Mock::given(method("GET"))
        .and(path(pipeline_path("ViewUserAccount-Logout")))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>bye</html>"))
        .mount(&server)
        .await;

    let http = net_kit::http::build(farmers_api::client_spec()).expect("a client");
    let session = Session::from_cookies(
        [
            (
                "__Host-SecureSessionID-pabc".to_string(),
                "signed-in".to_string(),
            ),
            ("_abck".to_string(), "ABC~-1~xyz".to_string()),
        ]
        .into_iter()
        .collect(),
    );
    let client = Client::new(
        http,
        Endpoints::defaults()
            .with_origin(server.uri())
            .with_search(server.uri()),
        session,
    );

    assert!(client.is_signed_in());
    client.sign_out().await.expect("signed out");
    assert!(!client.is_signed_in());
    assert!(client.session().warmed(), "the warm-up survived");
}

#[tokio::test]
async fn signing_out_when_no_one_is_signed_in_spends_no_request() {
    let server = MockServer::start().await;
    let error = client(&server).sign_out().await.expect_err("nothing to do");
    assert!(matches!(error, Error::NotSignedIn));
    assert!(server
        .received_requests()
        .await
        .expect("recorded")
        .is_empty());
}

/// A client that can sign itself in again, filing into `dir`.
fn client_with_reauth(server: &MockServer, dir: &std::path::Path) -> Client {
    let secrets = net_kit::Secrets::new("farmers-api-test", net_kit::Backend::File, dir);
    client(server).with_reauth(Some(farmers_api::Reauth {
        email: "shopper@example.invalid".into(),
        password: net_kit::password::Source::Stored("not-a-real-password".into()),
        secrets,
    }))
}

/// The sign-in page and a form that answers with a signed-in cookie.
async fn mount_login(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/login"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"<input name="SynchronizerToken" value="a-minted-token"/>"#),
        )
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path(pipeline_path("ViewUserAccount-ProcessLogin")))
        .respond_with(
            ResponseTemplate::new(302)
                .append_header("location", "/account")
                .append_header(
                    "set-cookie",
                    "__Host-SecureSessionID-paccount=signed-in; Path=/",
                ),
        )
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(pipeline_path("ViewUserAccount-AjaxHeader")))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"<span class="my-account-logged-in"></span><script>
               DDTrackingEvents.identityUserEvent('shopper@example.invalid','Ada','Lovelace')
               </script>"#,
        ))
        .mount(server)
        .await;
}

#[tokio::test]
async fn a_client_with_credentials_signs_itself_in_again_unattended() {
    let server = MockServer::start().await;
    mount_home(&server).await;
    mount_login(&server).await;
    let dir = tempfile::tempdir().expect("a temp dir");

    let client = client_with_reauth(&server, dir.path());
    assert!(!client.is_signed_in(), "nothing to start with");

    let account = client.renew().await.expect("signed in again");
    assert!(account.signed_in);
    assert_eq!(account.name().as_deref(), Some("Ada Lovelace"));
    assert!(client.is_signed_in());

    // Filed, so the next process starts signed in rather than doing this
    // again.
    let secrets = net_kit::Secrets::new("farmers-api-test", net_kit::Backend::File, dir.path());
    let stored = farmers_api::StoredSession::load(&secrets)
        .expect("loads")
        .expect("was saved");
    assert!(stored.session().account());
    assert_eq!(stored.email.as_deref(), Some("shopper@example.invalid"));
}

#[tokio::test]
async fn renewing_does_not_carry_a_stale_account_cookie_into_the_form() {
    // Posting an expired session alongside the credentials leaves a refusal
    // looking like a bad password, which sends someone to change a password
    // that was never wrong.
    let server = MockServer::start().await;
    mount_home(&server).await;
    mount_login(&server).await;
    let dir = tempfile::tempdir().expect("a temp dir");

    let secrets = net_kit::Secrets::new("farmers-api-test", net_kit::Backend::File, dir.path());
    let stale = Session::from_cookies(
        [
            (
                "__Host-SecureSessionID-pstale".to_string(),
                "expired".to_string(),
            ),
            ("_abck".to_string(), "ABC~-1~xyz".to_string()),
        ]
        .into_iter()
        .collect(),
    );
    let http = net_kit::http::build(farmers_api::client_spec()).expect("a client");
    let client = Client::new(
        http,
        Endpoints::defaults()
            .with_origin(server.uri())
            .with_search(server.uri()),
        stale,
    )
    .with_reauth(Some(farmers_api::Reauth {
        email: "shopper@example.invalid".into(),
        password: net_kit::password::Source::Stored("not-a-real-password".into()),
        secrets,
    }));

    client.renew().await.expect("signed in again");

    let posted = server
        .received_requests()
        .await
        .expect("recorded")
        .into_iter()
        .find(|r: &Request| r.method == wiremock::http::Method::POST)
        .expect("the form was posted");
    let cookies = posted
        .headers
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(
        !cookies.contains("pstale"),
        "the stale session went too: {cookies}"
    );
    // The warm-up did travel: it is what gets the POST served at all.
    assert!(cookies.contains("_abck"), "{cookies}");
}

#[tokio::test]
async fn renewing_without_credentials_says_so_rather_than_prompting() {
    let server = MockServer::start().await;
    let error = client(&server).renew().await.expect_err("nothing on file");
    assert!(matches!(error, Error::NotSignedIn));
    assert!(server
        .received_requests()
        .await
        .expect("recorded")
        .is_empty());
}

#[tokio::test]
async fn a_bot_manager_refusal_during_verify_is_not_reported_as_a_lapsed_session() {
    // The distinction this whole method exists for. Signing in again would be
    // refused in exactly the same way, so reading a denial as "your session
    // expired" spends a password to learn nothing -- and tells someone their
    // login is the problem when it is not.
    let server = MockServer::start().await;
    mount_home(&server).await;
    Mock::given(method("GET"))
        .and(path(pipeline_path("ViewUserAccount-AjaxHeader")))
        .respond_with(ResponseTemplate::new(429).set_body_string(r#"{"cpr_chlge":"true","t":"1"}"#))
        .mount(&server)
        .await;

    let http = net_kit::http::build(farmers_api::client_spec()).expect("a client");
    let session = Session::from_cookies(
        [(
            "__Host-SecureSessionID-pabc".to_string(),
            "signed-in".to_string(),
        )]
        .into_iter()
        .collect(),
    );
    let client = Client::new(
        http,
        Endpoints::defaults()
            .with_origin(server.uri())
            .with_search(server.uri()),
        session,
    );

    let error = client.verify().await.expect_err("refused, not answered");
    assert!(error.is_denied(), "{error}");
    assert!(!error.is_lapsed(), "{error}");
}

#[tokio::test]
async fn verify_answers_false_for_a_session_the_storefront_has_dropped() {
    let server = MockServer::start().await;
    mount_home(&server).await;
    Mock::given(method("GET"))
        .and(path(pipeline_path("ViewUserAccount-AjaxHeader")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"<span class="my-account-logged-out"></span>"#),
        )
        .mount(&server)
        .await;

    let http = net_kit::http::build(farmers_api::client_spec()).expect("a client");
    let session = Session::from_cookies(
        [(
            "__Host-SecureSessionID-pabc".to_string(),
            "signed-in".to_string(),
        )]
        .into_iter()
        .collect(),
    );
    let client = Client::new(
        http,
        Endpoints::defaults()
            .with_origin(server.uri())
            .with_search(server.uri()),
        session,
    );
    assert!(client.is_signed_in(), "the cookie still claims it");
    assert!(
        !client.verify().await.expect("asked"),
        "the storefront disagrees"
    );
}

#[tokio::test]
async fn verify_spends_no_request_when_there_is_no_session_to_check() {
    let server = MockServer::start().await;
    assert!(!client(&server).verify().await.expect("asked"));
    assert!(server
        .received_requests()
        .await
        .expect("recorded")
        .is_empty());
}

/// A mini-cart holding one line, with obviously fake ids.
const MINICART: &str = r##"<div id="mini-cart-container">
<span id="mini-cart-count">2 items</span>
<div class="mini-cart" id="miniCart" data-currency="NZD" data-cart-id="notarealcartid01"
 cart-total="$179.98" data-cart-grand-total="$179.98" data-cart-subtotal="$179.98">
<div class="product-row quick-cart-row">
<div class="mini-product-title"><div class="row"><div class="col-xs-9">
<a href="https://www.farmers.co.nz/men/robes/a-robe-6867065002">Chisel Fleece Robe, Charcoal</a></div>
<div class="col-xs-3"><a class="ico-remove-item"
onclick="QuickCart.removeItem.call(['/INTERSHOP/web/WFS/Farmers-Shop-Site/en_NZ/-/NZD/ViewMiniCart-RemoveItemFromMinicart?RemovePLI=notarealpli01','x','y'])"
data-remove-pli-minicart="6867065002"></a></div></div></div>
<div class="cart-pli-data"><span>Quantity:</span><span class="product-quantity">2</span></div>
<div class="cart-pli-data"><span>Size:</span>L-XL</div>
<div class="product-price" data-sales-price="$89.99" data-list-price="$89.99">
<div><div class="total-price">$179.98</div></div></div>
</div></div></div>"##;

/// A client whose stored cookies claim an account.
fn signed_in_client(server: &MockServer) -> Client {
    let http = net_kit::http::build(farmers_api::client_spec()).expect("a client");
    let session = Session::from_cookies(
        [
            (
                "__Host-SecureSessionID-pabc".to_string(),
                "signed-in".to_string(),
            ),
            ("_abck".to_string(), "ABC~-1~xyz".to_string()),
        ]
        .into_iter()
        .collect(),
    );
    Client::new(
        http,
        Endpoints::defaults()
            .with_origin(server.uri())
            .with_search(server.uri()),
        session,
    )
}

#[tokio::test]
async fn the_basket_is_read_from_the_fragment_rather_than_the_cart_page() {
    // The page is 160KB of markup carrying the same facts. Against a host that
    // counts requests and bytes, the fragment is the whole point.
    let server = MockServer::start().await;
    mount_home(&server).await;
    Mock::given(method("GET"))
        .and(path(pipeline_path("ViewMiniCart-Status")))
        .respond_with(ResponseTemplate::new(200).set_body_string(MINICART))
        .mount(&server)
        .await;

    let cart = client(&server).cart().await.expect("a basket");
    assert_eq!(cart.lines.len(), 1);
    assert_eq!(cart.lines[0].id, "notarealpli01");
    assert_eq!(cart.units(), 2);
    assert_eq!(cart.grand_total.unwrap().value, 179.98);

    let paths: Vec<String> = server
        .received_requests()
        .await
        .expect("recorded")
        .iter()
        .map(|r| r.url.path().to_string())
        .collect();
    assert!(!paths.iter().any(|p| p.ends_with("/cart")), "{paths:?}");
}

#[tokio::test]
async fn adding_to_the_basket_sends_what_the_product_pages_form_sends() {
    // Sent as a GET, and that is the whole point of this mock rather than an
    // incidental detail: the bot manager refuses this pipeline's POST and
    // serves the same add as a GET. A test asserting the method keeps a later
    // tidy-up from "fixing" it back into a POST that the live site denies.
    let server = MockServer::start().await;
    mount_home(&server).await;
    Mock::given(method("GET"))
        .and(path(pipeline_path("ViewExpressShop-AddProduct")))
        .and(query_param("SKU", "6867065002"))
        .and(query_param("Quantity_6867065002", "2"))
        // Without this the pipeline redirects to the cart page instead of
        // answering with the fragment.
        .and(query_param("addToCartBehavior", "expresscart"))
        .and(query_param("VariationAttribute_Size-DisplayName", "L-XL"))
        .respond_with(ResponseTemplate::new(200).set_body_string(MINICART))
        .mount(&server)
        .await;

    // The answer is the new basket, so adding costs one request and not two.
    let cart = client(&server)
        .cart_add(
            "6867065002",
            2,
            &[("Size-DisplayName".to_string(), "L-XL".to_string())],
        )
        .await
        .expect("added");
    assert_eq!(cart.units(), 2);
    assert_eq!(server.received_requests().await.expect("recorded").len(), 2);
}

#[tokio::test]
async fn removing_a_line_is_addressed_by_the_line_id_and_not_the_sku() {
    // The same product can sit on two lines; removing "6867065002" would be
    // ambiguous where removing a line is not.
    let server = MockServer::start().await;
    mount_home(&server).await;
    Mock::given(method("GET"))
        .and(path(pipeline_path("ViewMiniCart-RemoveItemFromMinicart")))
        .and(query_param("RemovePLI", "notarealpli01"))
        .respond_with(ResponseTemplate::new(200).set_body_string(MINICART))
        .mount(&server)
        .await;

    client(&server)
        .cart_remove("notarealpli01")
        .await
        .expect("removed");
}

#[tokio::test]
async fn changing_a_quantity_takes_the_token_from_the_cart_page_first() {
    // The one basket call that is a two-step. The other three need no token,
    // and paying for one on every call would double the request count.
    let server = MockServer::start().await;
    mount_home(&server).await;
    Mock::given(method("GET"))
        .and(path("/cart"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"<form><input name="SynchronizerToken" value="a-cart-token"/></form>"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(pipeline_path("ViewCart-Dispatch")))
        .and(body_string_contains("SynchronizerToken=a-cart-token"))
        .and(body_string_contains("Quantity_notarealpli01=5"))
        .and(body_string_contains("promotionCode=SAVE10"))
        // The submit button's name, which the pipeline dispatches on.
        .and(body_string_contains("update="))
        .respond_with(ResponseTemplate::new(302).append_header("location", "/cart"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(pipeline_path("ViewMiniCart-Status")))
        .respond_with(ResponseTemplate::new(200).set_body_string(MINICART))
        .mount(&server)
        .await;

    let cart = client(&server)
        .cart_update(&[("notarealpli01".to_string(), 5)], Some("SAVE10"))
        .await
        .expect("updated");
    assert_eq!(cart.lines.len(), 1);
}

#[tokio::test]
async fn a_cart_page_with_no_token_is_reported_rather_than_posted_to_blind() {
    let server = MockServer::start().await;
    mount_home(&server).await;
    Mock::given(method("GET"))
        .and(path("/cart"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>no form here</html>"))
        .mount(&server)
        .await;

    let error = client(&server)
        .cart_update(&[("notarealpli01".to_string(), 5)], None)
        .await
        .expect_err("no token");
    assert!(matches!(error, Error::NotInPage { .. }), "{error}");
    let posts = server
        .received_requests()
        .await
        .expect("recorded")
        .iter()
        .filter(|r: &&Request| r.method == wiremock::http::Method::POST)
        .count();
    assert_eq!(posts, 0);
}

#[tokio::test]
async fn an_account_only_call_is_refused_locally_rather_than_read_as_empty() {
    // These pipelines answer a signed-out request with the sign-in page and a
    // 200, so an unchecked call reports an empty order history instead of
    // saying to sign in.
    let server = MockServer::start().await;
    for result in [
        client(&server).orders().await.err(),
        client(&server).wishlists().await.err(),
        client(&server).wishlist_add("6867065002").await.err(),
    ] {
        assert!(matches!(result, Some(Error::NotSignedIn)), "{result:?}");
    }
    assert!(server
        .received_requests()
        .await
        .expect("recorded")
        .is_empty());
}

#[tokio::test]
async fn an_account_with_no_orders_is_told_apart_from_markup_that_moved() {
    let server = MockServer::start().await;
    mount_home(&server).await;
    Mock::given(method("GET"))
        .and(path("/orders"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<h1>Order History</h1><p>You do not have any Orders</p>"),
        )
        .mount(&server)
        .await;

    let orders = signed_in_client(&server).orders().await.expect("asked");
    assert!(orders.is_empty());
}

#[tokio::test]
async fn a_wishlist_add_that_was_answered_with_the_sign_in_page_is_not_a_success() {
    // The pipeline answers 200 with a link to /login rather than an error, so
    // a session that lapsed reads as "saved" unless this is looked for.
    let server = MockServer::start().await;
    mount_home(&server).await;
    Mock::given(method("POST"))
        .and(path(pipeline_path("ViewWishlist-AddItems")))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"<a id="redirect" href="https://www.farmers.co.nz/login?LoginToUse=wishlists">Please login.</a>"#,
        ))
        .mount(&server)
        .await;

    let error = signed_in_client(&server)
        .wishlist_add("6867065002")
        .await
        .expect_err("not actually saved");
    assert!(matches!(error, Error::SessionExpired), "{error}");
}

#[tokio::test]
async fn a_basket_works_signed_out_because_it_hangs_off_the_session() {
    // Deliberately not behind `require_account`: a guest basket is ordinary,
    // and it survives signing in afterwards.
    let server = MockServer::start().await;
    mount_home(&server).await;
    Mock::given(method("GET"))
        .and(path(pipeline_path("ViewMiniCart-Status")))
        .respond_with(ResponseTemplate::new(200).set_body_string(MINICART))
        .mount(&server)
        .await;

    let client = client(&server);
    assert!(!client.is_signed_in());
    assert_eq!(client.cart().await.expect("a basket").units(), 2);
}

#[tokio::test]
async fn a_page_is_warmed_and_retried_exactly_like_a_pipeline() {
    // Not because a page is likelier to be refused, but so a caller never has
    // to know which of the two it is talking to in order to predict what a
    // refusal costs.
    let server = MockServer::start().await;
    mount_home(&server).await;
    Mock::given(method("GET"))
        .and(path("/orders"))
        .respond_with(ResponseTemplate::new(200).set_body_string(DENY_BODY))
        .mount(&server)
        .await;

    let error = signed_in_client(&server)
        .orders()
        .await
        .expect_err("refused");
    assert!(
        matches!(error, Error::Denied { warmed: true }),
        "the second refusal is the one reported: {error}"
    );

    let paths: Vec<String> = server
        .received_requests()
        .await
        .expect("recorded")
        .iter()
        .map(|r| r.url.path().to_string())
        .collect();
    assert_eq!(paths.len(), 4, "warm, page, warm, page: {paths:?}");
}

#[tokio::test]
async fn a_refused_warm_up_is_a_denial_and_nothing_else_is_tried() {
    // Measured against the live site on 2026-09-18: a client on a profile the
    // site refuses is denied the *home page* too, with the same
    // `WAF_Deny_Page` body and no `Set-Cookie` at all. A warm-up that does not
    // read its own body cannot tell that apart from success, so it reports the
    // refusal against whichever call came next and claims a warm-up that never
    // happened.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(DENY_BODY))
        .mount(&server)
        .await;

    let error = client(&server)
        .product("6867065002")
        .await
        .expect_err("a refused warm-up cannot produce a product");
    assert!(
        matches!(error, Error::Denied { warmed: false }),
        "{error:?}"
    );

    let requests = server.received_requests().await.expect("recorded");
    // Two warm-ups and no product call: the second warm is the cold retry made
    // after dropping any stored Akamai cookies, and the call itself is never
    // attempted because there is nothing to make it with.
    assert_eq!(requests.len(), 2, "two warm-ups, no call");
    assert!(requests.iter().all(|r| r.url.path() == "/"), "both warms");
}

#[tokio::test]
async fn a_warm_up_that_earns_no_cookie_is_a_denial() {
    // A 200 of ordinary markup that hands out no `_abck` leaves nothing to
    // make the next call with. Treating it as warm would spend a request to
    // learn what is already known here.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>no cookies</html>"))
        .mount(&server)
        .await;

    let error = client(&server)
        .product("6867065002")
        .await
        .expect_err("an uncookied warm-up cannot produce a product");
    assert!(
        matches!(error, Error::Denied { warmed: false }),
        "{error:?}"
    );
    // The cold retry runs here too; what matters is that no call follows it.
    let requests = server.received_requests().await.expect("recorded");
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|r| r.url.path() == "/"));
}

#[tokio::test]
async fn a_failed_warm_up_does_not_leave_the_client_believing_it_is_warm() {
    // `warm` marks the client warmed before it knows the outcome, so a failure
    // has to put that back. Otherwise the retry path, and every later call in
    // the same process, skips the warm-up it still needs.
    let server = MockServer::start().await;
    let deny = Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(DENY_BODY))
        .expect(4);
    server.register(deny).await;

    let client = client(&server);
    assert!(client.product("6867065002").await.is_err());
    // A second call on the same client warms again rather than assuming it.
    assert!(client.product("6867065002").await.is_err());

    let requests = server.received_requests().await.expect("recorded");
    // Two per call: the warm and the cold retry after it. The point is that
    // the second call warms at all rather than trusting a flag set by a
    // warm-up that failed.
    assert_eq!(requests.len(), 4, "each call warms for itself");
}

#[tokio::test]
async fn the_interstitial_challenge_is_not_read_as_an_empty_answer() {
    // The dangerous shape. It is a 200, it carries none of the deny markers,
    // and the page it stands in for is an account page whose "empty" state is
    // legitimate -- so a client that misses it reports an empty order history
    // to someone who has orders. Measured live before it was caught.
    let server = MockServer::start().await;
    mount_home(&server).await;
    Mock::given(method("GET"))
        .and(path("/orders"))
        .respond_with(ResponseTemplate::new(200).set_body_string(CHALLENGE_BODY))
        .mount(&server)
        .await;

    let session = Session::from_cookies(
        [(
            "__Host-SecureSessionID-pabc".to_string(),
            "signed-in".to_string(),
        )]
        .into_iter()
        .collect(),
    );
    let client = Client::new(
        net_kit::http::build(farmers_api::client_spec()).expect("a client"),
        Endpoints::defaults().with_origin(server.uri()),
        session,
    );

    let error = client
        .orders()
        .await
        .expect_err("a challenge is not an empty order history");
    assert!(matches!(error, Error::Challenged), "{error:?}");
}

#[tokio::test]
async fn a_challenge_is_recognised_wherever_it_arrives() {
    // It can land on any of the three transports, so the check belongs on all
    // of them rather than on the one it was first seen through.
    assert!(farmers_api::is_challenge_page(CHALLENGE_BODY));
    // And it must not be confused with the deny page, which is a different
    // shape with a different remedy.
    assert!(!farmers_api::is_challenge_page(DENY_BODY));
    assert!(!farmers_api::is_deny_page(CHALLENGE_BODY));
}

/// A stand-in for the browser: hands back a jar and counts how often it was
/// asked. No browser is started anywhere in this suite -- what is under test
/// is when the hook is called and what is done with its answer, and a real
/// camoufox would make that slow and flaky without testing any more of it.
fn warmer(cookies: &[(&str, &str)]) -> (Warmer, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    let jar: BTreeMap<String, String> = cookies
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = calls.clone();
    let hook: Warmer = std::sync::Arc::new(move || {
        counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let jar = jar.clone();
        Box::pin(async move { Ok(jar) })
    });
    (hook, calls)
}

#[tokio::test]
async fn a_client_with_a_warmer_never_fetches_the_home_page_itself() {
    // The finding this whole path exists for: an `_abck` this crate fetched
    // for itself is refused, so spending a request on one would be spending it
    // to fail.
    let server = MockServer::start().await;
    mount_home(&server).await;
    Mock::given(method("GET"))
        .and(path(rest_path("/products/6867065002")))
        .respond_with(ResponseTemplate::new(200).set_body_string(PRODUCT))
        .mount(&server)
        .await;

    let (hook, calls) = warmer(&[("_abck", "FROM~-1~BROWSER"), ("bm_sz", "zzz")]);
    let client = client(&server).with_warmer(Some(hook));
    client.product("6867065002").await.expect("a product");

    let requests = server.received_requests().await.expect("recorded");
    assert_eq!(requests.len(), 1, "no warm-up request, just the call");
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    let sent = requests[0]
        .headers
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(sent.contains("_abck=FROM~-1~BROWSER"), "{sent}");
}

#[tokio::test]
async fn a_stored_jar_is_spent_before_a_browser_is_started() {
    // A warm-up costs a browser launch and ten seconds where it used to cost
    // one request, so warmth that is already in hand is worth trying even
    // though it may have lapsed. The deny path below is what catches that.
    let server = MockServer::start().await;
    mount_home(&server).await;
    Mock::given(method("GET"))
        .and(path(rest_path("/products/6867065002")))
        .respond_with(ResponseTemplate::new(200).set_body_string(PRODUCT))
        .mount(&server)
        .await;

    let (hook, calls) = warmer(&[("_abck", "FROM~-1~BROWSER")]);
    let stored = Session::from_cookies(
        [("_abck".to_string(), "STORED~-1~x".to_string())]
            .into_iter()
            .collect(),
    );
    let http = net_kit::http::build(farmers_api::client_spec()).expect("a client");
    let endpoints = Endpoints::defaults()
        .with_origin(server.uri())
        .with_search(server.uri());
    let client = Client::new(http, endpoints, stored).with_warmer(Some(hook));

    client.product("6867065002").await.expect("a product");
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "no browser for a jar that worked"
    );
}

#[tokio::test]
async fn a_lapsed_jar_is_replaced_by_the_browser_and_the_call_retried() {
    // The common failure by far: a browser-earned jar is good for minutes, not
    // for a day, and it lapses back into `Access Denied` with no warning.
    let server = MockServer::start().await;
    mount_home(&server).await;
    // Denied while the stale cookie is being sent, served once it is not.
    Mock::given(method("GET"))
        .and(path(rest_path("/products/6867065002")))
        .and(wiremock::matchers::header_regex("cookie", "LAPSED"))
        .respond_with(ResponseTemplate::new(200).set_body_string(DENY_BODY))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(rest_path("/products/6867065002")))
        .respond_with(ResponseTemplate::new(200).set_body_string(PRODUCT))
        .mount(&server)
        .await;

    let (hook, calls) = warmer(&[("_abck", "FRESH~-1~x")]);
    let stored = Session::from_cookies(
        [("_abck".to_string(), "LAPSED~-1~x".to_string())]
            .into_iter()
            .collect(),
    );
    let http = net_kit::http::build(farmers_api::client_spec()).expect("a client");
    let endpoints = Endpoints::defaults()
        .with_origin(server.uri())
        .with_search(server.uri());
    let client = Client::new(http, endpoints, stored).with_warmer(Some(hook));

    let product = client.product("6867065002").await.expect("a product");
    assert_eq!(product.sku, "6867065002");
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the browser was started once, on the denial"
    );
    assert_eq!(
        client.session().get("_abck"),
        Some("FRESH~-1~x"),
        "and its jar replaced the lapsed one"
    );
}

#[tokio::test]
async fn a_warm_up_does_not_sign_anybody_out() {
    // A browser that has loaded the home page hands back an anonymous `sid`
    // and secure-session cookies of its own. Adopting those wholesale would
    // overwrite the session of somebody who is signed in -- a warm-up that
    // quietly logs you out, which is about the worst shape this bug could
    // take.
    let server = MockServer::start().await;
    mount_home(&server).await;
    Mock::given(method("GET"))
        .and(path(rest_path("/products/6867065002")))
        .respond_with(ResponseTemplate::new(200).set_body_string(PRODUCT))
        .mount(&server)
        .await;

    let (hook, _) = warmer(&[
        ("_abck", "FROM~-1~BROWSER"),
        ("sid", "the-browsers-own-visit"),
        ("__Host-SecureSessionID-tbrowser", "transient"),
    ]);
    let signed_in = Session::from_cookies(
        [
            ("sid".to_string(), "the-signed-in-session".to_string()),
            (
                "__Host-SecureSessionID-paccount".to_string(),
                "the-account".to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    );
    let http = net_kit::http::build(farmers_api::client_spec()).expect("a client");
    let endpoints = Endpoints::defaults()
        .with_origin(server.uri())
        .with_search(server.uri());
    let client = Client::new(http, endpoints, signed_in).with_warmer(Some(hook));

    client.product("6867065002").await.expect("a product");
    let session = client.session();
    assert!(session.account(), "still signed in");
    assert_eq!(session.get("sid"), Some("the-signed-in-session"));
    assert_eq!(session.get("_abck"), Some("FROM~-1~BROWSER"), "but warmed");
    assert_eq!(session.get("__Host-SecureSessionID-tbrowser"), None);
}

/// The account header, as the storefront serves it for a signed-in visitor.
const ACCOUNT_HEADER: &str = r#"<span class="my-account-logged-in"></span><script>
    DDTrackingEvents.identityUserEvent('shopper@example.invalid','Ada','Lovelace')
    </script>"#;

/// A session that already holds a grant, warmed, as a stored one would be.
fn signed_in_session() -> Session {
    Session::from_cookies(
        [
            ("_abck".to_string(), "ABC~-1~x".to_string()),
            (
                "__Host-SecureSessionID-paccount".to_string(),
                "signed-in".to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    )
    .with_email(Some("shopper@example.invalid".into()))
}

fn client_with(server: &MockServer, session: Session) -> Client {
    let http = net_kit::http::build(farmers_api::client_spec()).expect("a client");
    let endpoints = Endpoints::defaults()
        .with_origin(server.uri())
        .with_search(server.uri());
    Client::new(http, endpoints, session)
}

#[tokio::test]
async fn signing_in_again_leaves_a_working_session_alone() {
    // Somebody who is not sure whether they are signed in runs `auth login`.
    // Spending their password to replace a session that is fine is the wrong
    // answer -- and the storefront would refuse the form anyway, since it
    // answers `/login` with a redirect for a session that has a grant.
    let server = MockServer::start().await;
    mount_home(&server).await;
    Mock::given(method("GET"))
        .and(path(pipeline_path("ViewUserAccount-AjaxHeader")))
        .respond_with(ResponseTemplate::new(200).set_body_string(ACCOUNT_HEADER))
        .mount(&server)
        .await;
    // Mounted so that using it is a test failure rather than a connection
    // error: the point is that the form is never reached.
    Mock::given(method("GET"))
        .and(path("/login"))
        .respond_with(ResponseTemplate::new(302).append_header("location", "/account"))
        .mount(&server)
        .await;

    let client = client_with(&server, signed_in_session());
    let account = client
        .login("shopper@example.invalid", "not-a-real-password")
        .await
        .expect("the session it already had");
    assert!(account.signed_in);
    assert_eq!(account.name().as_deref(), Some("Ada Lovelace"));

    let paths: Vec<String> = server
        .received_requests()
        .await
        .expect("recorded")
        .iter()
        .map(|r| r.url.path().to_string())
        .collect();
    assert!(
        !paths
            .iter()
            .any(|p| p.contains("login") || p.contains("Login")),
        "the sign-in form was never touched: {paths:?}"
    );
}

#[tokio::test]
async fn signing_in_as_somebody_else_does_not_keep_the_session_it_found() {
    // The same short-circuit, wrong, would leave somebody looking at another
    // person's account and reporting it as a successful sign-in. A different
    // address has to reach the form -- and reach it without the grant, or
    // Intershop redirects instead of serving it.
    let server = MockServer::start().await;
    mount_home(&server).await;
    Mock::given(method("GET"))
        .and(path(pipeline_path("ViewUserAccount-AjaxHeader")))
        .respond_with(ResponseTemplate::new(200).set_body_string(ACCOUNT_HEADER))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/login"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"<form name="LoginUserForm" method="post">
               <input type="hidden" name="SynchronizerToken" value="a-minted-token"/>
               </form>"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(pipeline_path("ViewUserAccount-ProcessLogin")))
        .and(body_string_contains(
            "ShopLoginForm_Login=someone.else%40example.invalid",
        ))
        .respond_with(
            ResponseTemplate::new(302)
                .append_header("location", "/account")
                .append_header(
                    "set-cookie",
                    "__Host-SecureSessionID-psomeone-else=the-other-one; Path=/",
                ),
        )
        .mount(&server)
        .await;

    let client = client_with(&server, signed_in_session());
    client
        .login("someone.else@example.invalid", "not-a-real-password")
        .await
        .expect("a sign-in");

    // The grant it arrived with must not have gone to the form: that is what
    // the redirect-instead-of-the-form failure was.
    let requests = server.received_requests().await.expect("recorded");
    let login_page = requests
        .iter()
        .find(|r| r.url.path() == "/login")
        .expect("the form was fetched");
    let sent = login_page
        .headers
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(!sent.contains("SecureSessionID-p"), "{sent}");
    assert!(
        sent.contains("_abck"),
        "but the warmth went with it: {sent}"
    );
}

#[tokio::test]
async fn a_sign_in_page_that_redirects_says_so_rather_than_quoting_the_page() {
    // What this looked like before: `HTTP 302: <html><head><meta ...` and a
    // page of Intershop boilerplate, for what is one sentence.
    let server = MockServer::start().await;
    mount_home(&server).await;
    Mock::given(method("GET"))
        .and(path("/login"))
        .respond_with(
            ResponseTemplate::new(302)
                .append_header("location", "https://www.farmers.co.nz/account")
                .set_body_string("<html><head><meta name=\"INTERSHOP\"/></head></html>"),
        )
        .mount(&server)
        .await;

    // No grant, so nothing short-circuits and the form is really asked for.
    let client = client(&server);
    let error = client
        .login("shopper@example.invalid", "not-a-real-password")
        .await
        .expect_err("a redirect is not a form");
    let message = error.to_string();
    assert!(message.contains("/account"), "{message}");
    assert!(message.contains("signed in"), "{message}");
    assert!(!message.contains("<html>"), "not the page: {message}");
}

#[tokio::test]
async fn a_denied_write_is_warmed_through_and_retried_like_every_read() {
    // The basket writes were the one transport with no retry -- it said it had
    // one and did not -- so they were the only commands that could never
    // recover from a jar that had lapsed between runs. Which is exactly when
    // they are used: a read warms the jar, a write is what comes next.
    let server = MockServer::start().await;
    mount_home(&server).await;
    Mock::given(method("POST"))
        .and(path(pipeline_path("ViewWishlist-AddItems")))
        .and(wiremock::matchers::header_regex("cookie", "LAPSED"))
        .respond_with(ResponseTemplate::new(403).set_body_string(DENY_BODY))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(pipeline_path("ViewWishlist-AddItems")))
        .respond_with(ResponseTemplate::new(200).set_body_string("<div>saved</div>"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(pipeline_path("ViewUserAccount-AjaxHeader")))
        .respond_with(ResponseTemplate::new(200).set_body_string(ACCOUNT_HEADER))
        .mount(&server)
        .await;

    let (hook, calls) = warmer(&[("_abck", "FRESH~-1~x")]);
    let mut cookies = signed_in_session().cookies();
    cookies.insert("_abck".into(), "LAPSED~-1~x".into());
    let client = client_with(&server, Session::from_cookies(cookies)).with_warmer(Some(hook));

    client.wishlist_add("6867065002").await.expect("saved");
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the browser was started once, on the denial"
    );
}
