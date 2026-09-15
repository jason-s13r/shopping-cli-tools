//! Integration tests against a mock storefront.
//!
//! Aimed at what fails *silently* against the real site and so cannot be
//! caught by reading an error message: the CORS headers the API requires, the
//! anonymous/current path split, a browse filtering the wrong category level,
//! and renewal quietly not happening.

use mitre10_api::{Client, Endpoints, Query, Session, SessionStore};
use wiremock::matchers::{body_json, body_string_contains, header, method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn endpoints(server: &MockServer) -> Endpoints {
    // Seeded, so only the discovery test spends a call on /config/key.
    Endpoints::defaults()
        .with_api(server.uri())
        .with_search(server.uri())
        .with_algolia("TESTAPPID00", "not-a-real-search-key")
}

fn client(server: &MockServer, session: Session) -> Client {
    let http = net_kit::http::build(mitre10_api::client_spec()).expect("a client builds");
    Client::new(http, endpoints(server), session)
}

/// A session that looks signed in without any token being real.
fn signed_in() -> Session {
    Session {
        access_token: Some("not.a.jwt".into()),
        refresh_token: Some("NotARealToken".into()),
        expires_at: Some(net_kit::jwt::now_ms() + 3_600_000),
        email: Some("shopper@example.invalid".into()),
    }
}

fn algolia(hits: serde_json::Value, total: u64) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "results": [{
            "hits": hits, "nbHits": total, "page": 0, "nbPages": 1, "hitsPerPage": 24,
        }]
    }))
}

#[tokio::test]
async fn every_call_carries_the_origin_the_api_insists_on() {
    // Without these the API answers 403 "Invalid CORS request" -- which reads
    // as an auth failure and sends someone to sign in for no reason.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/occ/v2/mitre10/products/174969"))
        .and(header("origin", "https://www.mitre10.co.nz"))
        .and(header("referer", "https://www.mitre10.co.nz/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": "174969",
            "name": "Fence Finish",
            "price": { "value": 69.0, "currencyIso": "NZD", "formattedValue": "$69.00" },
        })))
        .expect(1)
        .mount(&server)
        .await;

    let product = client(&server, Session::default())
        .product("174969", None)
        .await
        .expect("the product answers");
    assert_eq!(product.code, "174969");
}

#[tokio::test]
async fn the_algolia_key_is_asked_for_once_and_then_reused() {
    // The id and key are the storefront's, not this crate's, so they are not
    // compiled in -- which means a broken config call has to read as one
    // rather than as an empty search.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/occ/v2/mitre10/config/key"))
        .and(query_param("key", "mitre10.algolia.index.config"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "key": "mitre10.algolia.index.config",
            "value": "{\"appId\": \"TESTAPPID00\",\"searchApiKey\": \"not-a-real-search-key\",\"hitsPerPage\": 24}",
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/1/indexes/*/queries"))
        .and(query_param("x-algolia-api-key", "not-a-real-search-key"))
        .and(query_param("x-algolia-application-id", "TESTAPPID00"))
        .respond_with(algolia(serde_json::json!([]), 0))
        .expect(2)
        .mount(&server)
        .await;

    let client = Client::new(
        net_kit::http::build(mitre10_api::client_spec()).expect("a client builds"),
        Endpoints::defaults()
            .with_api(server.uri())
            .with_search(server.uri()),
        Session::default(),
    );
    client
        .listing(&Query::search("hammer"))
        .await
        .expect("a listing");
    client
        .listing(&Query::search("nail"))
        .await
        .expect("a listing");
}

#[tokio::test]
async fn a_lapsed_login_does_not_stop_a_search() {
    // Search is anonymous, and asking the storefront for the Algolia key must
    // not make it otherwise: renewing first would fail the whole command on a
    // stale stored login that search never needed.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/occ/v2/mitre10/config/key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "value": "{\"appId\": \"TESTAPPID00\",\"searchApiKey\": \"not-a-real-search-key\"}",
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/1/indexes/*/queries"))
        .respond_with(algolia(serde_json::json!([]), 0))
        .expect(1)
        .mount(&server)
        .await;
    // No /oauth/token mock: a renewal attempt 404s and fails the call.

    let expired = Session {
        expires_at: Some(net_kit::jwt::now_ms() - 3_600_000),
        ..signed_in()
    };
    let client = Client::new(
        net_kit::http::build(mitre10_api::client_spec()).expect("a client builds"),
        Endpoints::defaults()
            .with_api(server.uri())
            .with_search(server.uri()),
        expired,
    );
    client
        .listing(&Query::search("hammer"))
        .await
        .expect("a listing");
}

#[tokio::test]
async fn a_browse_filters_the_level_the_category_code_belongs_to() {
    // Filtering lvl0 for a fineline code matches nothing, and an empty grid
    // reads as an empty category rather than a wrong query.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/1/indexes/*/queries"))
        .and(body_string_contains("categoryID.lvl2%3ARF7336"))
        .respond_with(algolia(
            serde_json::json!([{ "objectID": "269938", "name": "Storage Bin & Lid" }]),
            1,
        ))
        .expect(1)
        .mount(&server)
        .await;

    let listing = client(&server, Session::default())
        .listing(&Query::category("RF7336"))
        .await
        .expect("the index answers");
    assert_eq!(listing.products[0].code, "269938");
}

#[tokio::test]
async fn an_anonymous_client_uses_the_anonymous_cart_path() {
    // `users/current` without a token answers 401, and `users/anonymous` with
    // one silently builds a second cart nobody sees. The split has to follow
    // the session.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/occ/v2/mitre10/users/anonymous/carts"))
        .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
            "guid": "f4eaa13e", "totalItems": 0,
        })))
        .expect(1)
        .mount(&server)
        .await;

    let cart = client(&server, Session::default())
        .create_cart()
        .await
        .expect("a cart is created");
    assert_eq!(cart.id_for(false), Some("f4eaa13e"));
}

#[tokio::test]
async fn a_signed_in_client_uses_the_current_cart_path_and_sends_its_bearer() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/occ/v2/mitre10/users/current/carts"))
        .and(header("authorization", "Bearer not.a.jwt"))
        .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
            "code": "3000744918", "guid": "f4eaa13e", "totalItems": 0,
        })))
        .expect(1)
        .mount(&server)
        .await;

    let cart = client(&server, signed_in())
        .create_cart()
        .await
        .expect("a cart is created");
    assert_eq!(
        cart.id_for(true),
        Some("3000744918"),
        "a signed-in cart is addressed by its code"
    );
}

#[tokio::test]
async fn an_expired_token_is_renewed_before_the_call_rather_than_after_it_fails() {
    // The point of renewing up front: the storefront's answer to an expired
    // token is a bare 401, which a caller would otherwise have to tell apart
    // from being signed out.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/authorizationserver/oauth/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "a.fresh.token",
            "refresh_token": "a-fresh-refresh",
            "expires_in": 10799,
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/occ/v2/mitre10/users/current"))
        .and(header("authorization", "Bearer a.fresh.token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "uid": "shopper@example.invalid", "firstName": "A", "lastName": "Shopper",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let stale = Session {
        access_token: Some("stale.jwt".into()),
        refresh_token: Some("NotARealToken".into()),
        expires_at: Some(net_kit::jwt::now_ms().saturating_sub(1)),
        email: None,
    };
    let customer = client(&server, stale)
        .customer()
        .await
        .expect("the renewal happens and the call goes through");
    assert_eq!(customer.email.as_deref(), Some("shopper@example.invalid"));
}

#[tokio::test]
async fn a_renewal_is_filed_so_the_next_command_does_not_repeat_it() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/authorizationserver/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "a.fresh.token",
            "refresh_token": "a-fresh-refresh",
            "expires_in": 10799,
        })))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().expect("a temp dir");
    let secrets = net_kit::Secrets::new("mitre10-test", net_kit::Backend::File, dir.path());
    let http = net_kit::http::build(mitre10_api::client_spec()).expect("a client builds");
    let client =
        Client::new(http, endpoints(&server), signed_in()).with_session_store(Some(SessionStore {
            secrets: net_kit::Secrets::new("mitre10-test", net_kit::Backend::File, dir.path()),
        }));

    client.renew().await.expect("the renewal succeeds");

    let filed = mitre10_api::StoredSession::load(&secrets)
        .expect("loads")
        .expect("was filed");
    assert_eq!(filed.session.access_token.as_deref(), Some("a.fresh.token"));
    assert_eq!(
        filed.session.refresh_token.as_deref(),
        Some("a-fresh-refresh"),
        "the reissued refresh token replaces the spent one"
    );
}

#[tokio::test]
async fn a_refused_refresh_token_is_reported_rather_than_retried() {
    // Retrying this loops: the storefront will refuse it every time, and only
    // a password gets past it.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/authorizationserver/oauth/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "error": "invalid_grant",
            "error_description": "Invalid refresh token",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let e = client(&server, signed_in())
        .renew()
        .await
        .expect_err("refused");
    assert!(matches!(e, mitre10_api::Error::LoginLapsed), "{e:?}");
    assert!(
        !e.is_lapsed(),
        "a lapsed login must not trigger another retry"
    );
}

#[tokio::test]
async fn a_hybris_errors_body_is_read_before_the_status_code() {
    // Hybris says *why* in `errors[]` and the status is usually a bare 400.
    // Reporting "HTTP 400" instead loses the only useful half.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/occ/v2/mitre10/users/anonymous/carts/f4eaa13e/entries",
        ))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "errors": [{ "message": "Product 174969 is not available", "type": "CartError" }]
        })))
        .mount(&server)
        .await;

    let e = client(&server, Session::default())
        .cart_add("f4eaa13e", "174969", 1, "66", true)
        .await
        .expect_err("refused");
    assert!(e.to_string().contains("not available"), "{e}");
    assert!(e.to_string().contains("addToCart"), "{e}");
}

#[tokio::test]
async fn the_quantity_goes_on_the_wire_as_a_whole_number() {
    // `1.0` is refused outright -- "Request body is invalid or missing", with
    // nothing to say the quantity was the part it disliked.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/occ/v2/mitre10/users/anonymous/carts/f4eaa13e/entries",
        ))
        .and(body_json(serde_json::json!({
            "quantity": 2,
            "product": { "code": "174969" },
            "storeId": "66",
            "giftCardAmount": 0,
            "clickToCollect": true,
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/occ/v2/mitre10/users/anonymous/carts/f4eaa13e"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "guid": "f4eaa13e",
            "entries": [{ "entryNumber": 0, "quantity": 2, "product": { "code": "174969" } }]
        })))
        .mount(&server)
        .await;

    let cart = client(&server, Session::default())
        .cart_add("f4eaa13e", "174969", 2, "66", true)
        .await
        .expect("the entry is accepted");
    assert_eq!(cart.cart.lines[0].quantity, 2);
    assert_eq!(cart.notice, None, "a clean change explains nothing");
}

#[tokio::test]
async fn an_unknown_product_is_named_rather_than_left_as_a_404() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/occ/v2/mitre10/products/999999"))
        .respond_with(ResponseTemplate::new(404).set_body_string("Not Found"))
        .mount(&server)
        .await;

    let e = client(&server, Session::default())
        .product("999999", None)
        .await
        .expect_err("refused");
    assert!(
        matches!(e, mitre10_api::Error::NoSuchProduct(ref c) if c == "999999"),
        "{e:?}"
    );
}

#[tokio::test]
async fn a_postcode_group_is_a_bare_number_not_json() {
    // The one endpoint here that does not answer JSON. Parsing it as JSON
    // fails in a way that reads as a moved schema.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/occ/v2/mitre10/c/postcodegroupid-postcode"))
        .and(query_param("postCode", "0176"))
        .respond_with(ResponseTemplate::new(200).set_body_string("5"))
        .expect(1)
        .mount(&server)
        .await;

    let group = client(&server, Session::default())
        .postcode_group("0176")
        .await
        .expect("answers");
    assert_eq!(group, "5");
}

#[tokio::test]
async fn the_batch_endpoint_gets_padded_codes_in_chunks() {
    // Sent unpadded, it answers an empty products array rather than an error,
    // so the whole batch disappears silently.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/occ/v2/mitre10/products/priceForProducts"))
        .respond_with(|req: &Request| {
            let url = req.url.as_str();
            assert!(
                url.contains("000000000000174969"),
                "codes must be zero-padded to eighteen digits: {url}"
            );
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "products": [{ "code": "174969", "name": "Fence Finish" }]
            }))
        })
        .expect(2)
        .mount(&server)
        .await;

    // Nineteen codes: one full batch of eighteen and a remainder.
    let codes: Vec<String> = std::iter::repeat_n("174969".to_string(), 19).collect();
    let products = client(&server, Session::default())
        .products(&codes, Some("66"), None)
        .await
        .expect("both batches answer");
    assert_eq!(products.len(), 2, "one product back per batch");
}

#[tokio::test]
async fn the_account_surface_refuses_before_spending_a_request() {
    // There is no token and no refresh token, so the call cannot succeed.
    // Sending it anyway would report a 401 instead of "not signed in".
    let server = MockServer::start().await;
    let e = client(&server, Session::default())
        .customer()
        .await
        .expect_err("refused");
    assert!(matches!(e, mitre10_api::Error::NotSignedIn), "{e:?}");
    assert_eq!(server.received_requests().await.expect("recorded").len(), 0);
}
