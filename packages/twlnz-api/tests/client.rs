//! The client against a mock storefront.
//!
//! Every fixture is a real response from a capture of the site, trimmed but not
//! reshaped, so these pin the behaviour of the actual markup rather than of
//! something convenient. Nothing here touches the network.
//!
//! Two exceptions, and they say so. `cart-removed.json` is the captured
//! envelope with lines put back into it: the capture only ever removed the last
//! line, so the interesting case -- a removal that leaves a basket behind --
//! had to be built. And `stores-region.json` keeps the real envelope but its
//! store records are invented, because the real ones carried Google Maps embed
//! URLs signed with The Warehouse's own API key.

use twlnz_api::{Client, Endpoints, Facet, Island, Pdp, Query, Session};
use wiremock::matchers::{body_string_contains, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("tests/fixtures/{name}")).expect(name)
}

/// The same page with its `verify` tokens re-stamped to now.
///
/// A captured page carries the timestamps it was captured with, so its tokens
/// are permanently stale and the client would refresh it before every write.
/// That behaviour is worth testing -- and is, below -- but it is not what the
/// retry tests are about.
fn freshened(name: &str) -> String {
    let now = net_kit::jwt::now_secs();
    let mut out = String::new();
    let text = fixture(name);
    let mut rest = text.as_str();
    while let Some(at) = rest.find("verify=") {
        out.push_str(&rest[..at + "verify=".len()]);
        rest = &rest[at + "verify=".len()..];
        let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        out.push_str(&now.to_string());
        rest = &rest[digits..];
    }
    out.push_str(rest);
    out
}

fn client(server: &MockServer) -> Client {
    let http = net_kit::http::build(twlnz_api::client_spec()).expect("http client");
    Client::new(
        http,
        Endpoints::default().with_origin(server.uri()),
        Session::default(),
    )
    .with_pace(net_kit::Pace::off())
}

fn html(name: &str) -> ResponseTemplate {
    body(fixture(name), "text/html; charset=utf-8")
}

fn body(text: String, content_type: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .set_body_string(text)
        .insert_header("content-type", content_type)
}

fn json(name: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .set_body_string(fixture(name))
        .insert_header("content-type", "application/json")
}

#[tokio::test]
async fn a_listing_window_parses_into_products() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/updategrid"))
        .respond_with(html("listing-window.html"))
        .mount(&server)
        .await;

    let listing = client(&server)
        .page(&Query::Keyword("blue".into()), 64, 32, None, &[])
        .await
        .unwrap();

    assert_eq!(listing.products.len(), 3);
    assert_eq!(listing.total, Some(3122), "read off the grid header");
    let first = &listing.products[0];
    assert!(!first.id.is_empty());
    assert!(!first.name.is_empty());
    assert!(first.price.value.is_some(), "a displayed price parses back");
    assert!(first.url.is_some());
}

#[tokio::test]
async fn a_throttled_request_is_retried_after_a_pause() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/updategrid"))
        .respond_with(ResponseTemplate::new(429))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/search/updategrid"))
        .respond_with(html("listing-window.html"))
        .expect(1)
        .mount(&server)
        .await;

    let listing = client(&server)
        .with_pace(net_kit::Pace::new(std::time::Duration::from_millis(10)))
        .page(&Query::Keyword("blue".into()), 64, 32, None, &[])
        .await
        .unwrap();
    assert_eq!(listing.products.len(), 3);
}

#[tokio::test]
async fn a_wait_longer_than_the_client_allows_is_reported_not_sat_out() {
    // Seven seconds is past the one-second ceiling, so the first 429 ends it
    // and the site's number is passed on for the person to act on.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/updategrid"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "7"))
        .expect(1)
        .mount(&server)
        .await;

    let Err(err) = client(&server)
        .with_pace(
            net_kit::Pace::new(std::time::Duration::from_millis(10))
                .with_max_backoff(std::time::Duration::from_secs(1)),
        )
        .page(&Query::Keyword("blue".into()), 64, 32, None, &[])
        .await
    else {
        panic!("a throttle should not read as a listing");
    };
    assert!(
        matches!(
            err,
            twlnz_api::Error::RateLimited {
                retry_after: Some(7)
            }
        ),
        "{err:?}"
    );
}

#[tokio::test]
async fn throttling_that_outlasts_the_retries_is_a_rate_limit() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/updategrid"))
        .respond_with(ResponseTemplate::new(429))
        .expect(3)
        .mount(&server)
        .await;

    let Err(err) = client(&server)
        .with_pace(net_kit::Pace::new(std::time::Duration::from_millis(10)).with_retries(2))
        .page(&Query::Keyword("blue".into()), 64, 32, None, &[])
        .await
    else {
        panic!("a throttle should not read as a listing");
    };
    assert!(err.is_rate_limited(), "{err:?}");
}

#[tokio::test]
async fn a_keyword_that_redirects_into_a_category_is_reported_as_having_done_so() {
    // The site answers "lego" with the LEGO category page rather than with
    // search results. Nothing in the status or the markup says so -- it is a
    // 200 of ordinary tiles either way -- so the landing URL is the only tell,
    // and a caller that never sees it cannot know it asked one question and was
    // answered another.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/updategrid"))
        .respond_with(ResponseTemplate::new(302).insert_header(
            "location",
            format!("{}/c/toys-baby/top-brands/lego", server.uri()),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/c/toys-baby/top-brands/lego"))
        .respond_with(html("listing-window.html"))
        .mount(&server)
        .await;

    let listing = client(&server)
        .page(&Query::Keyword("lego".into()), 0, 32, None, &[])
        .await
        .unwrap();

    assert_eq!(
        listing.category.as_deref(),
        Some("toys-baby/top-brands/lego")
    );
    assert_eq!(listing.products.len(), 3, "the tiles still parsed");
}

#[tokio::test]
async fn paging_stops_at_the_end_rather_than_asking_for_a_window_past_it() {
    // The grid header says 3,122 of 3,122, so there is nothing after this page
    // even though the caller asked for more than it holds.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/updategrid"))
        .respond_with(html("listing-last-page.html"))
        .mount(&server)
        .await;

    let listing = client(&server)
        .search(&Query::Category("toysbaby".into()), 100, None, &[])
        .await
        .unwrap();

    assert_eq!(listing.products.len(), 1);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn an_empty_listing_is_an_empty_result_not_an_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/updategrid"))
        .respond_with(html("listing-empty.html"))
        .mount(&server)
        .await;

    let listing = client(&server)
        .search(&Query::Keyword("nothing at all".into()), 50, None, &[])
        .await
        .unwrap();
    assert!(listing.products.is_empty());
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn the_island_and_the_callers_facets_both_reach_the_request() {
    // The regression this guards: numbering the caller's refinements from 1
    // while the island also claims slot 1 drops one of them silently.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/updategrid"))
        .and(query_param("prefn1", "islandAvailability"))
        .and(query_param("prefv1", "southIsland"))
        .and(query_param("prefn2", "brand"))
        .and(query_param("prefv2", "Example Brand"))
        .and(query_param("srule", "price-low-to-high"))
        .respond_with(html("listing-window.html"))
        .mount(&server)
        .await;

    let listing = client(&server)
        .with_island(Some(Island::South))
        .page(
            &Query::Keyword("blue".into()),
            0,
            32,
            Some("price-low-to-high"),
            &[Facet::new("brand", "Example Brand")],
        )
        .await
        .unwrap();
    assert_eq!(listing.products.len(), 3);
}

#[tokio::test]
async fn a_product_page_yields_the_tokens_its_actions_need() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/p/p/R3059518.html"))
        .respond_with(html("product-page.html"))
        .mount(&server)
        .await;

    let pdp = client(&server).pdp("R3059518").await.unwrap();
    assert!(pdp.actions.add_to_cart.is_some());
    assert!(pdp.actions.add_to_wishlist.is_some());
    assert!(pdp.actions.store_stock.is_some());
    assert!(pdp.minted_at().is_some(), "the token carries its own age");
}

#[tokio::test]
async fn an_in_store_only_variant_is_not_reported_as_sold_out() {
    // The observed case that made availability two-dimensional: no online
    // stock, orderable in a shop.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/products/variation"))
        .respond_with(json("variation-in-store-only.json"))
        .mount(&server)
        .await;

    let pdp = Pdp::parse(
        "RM110166766-8M",
        r#"<script type="application/ld+json">{"@type":"Product","name":"Tee"}</script>
           <a href="/products/variation?pid=RM110166766-8M&verify=1-x">v</a>"#,
    )
    .unwrap();

    let detail = client(&server)
        .select(&pdp, "color", "GRN M")
        .await
        .unwrap();

    assert_eq!(detail.product.availability.online, Some(false));
    assert_eq!(detail.product.availability.in_store, Some(true));
    assert_eq!(detail.product.availability.summary(), "in store");
    assert!(
        detail.product.availability.orderable() == Some(true),
        "a shelf full of stock is not sold out"
    );
    // The axes come back with both questions answered separately.
    let size = detail
        .axes
        .iter()
        .find(|a| a.id == "size")
        .expect("a size axis");
    assert!(size.values.iter().any(|v| v.selectable && !v.orderable));
}

#[tokio::test]
async fn a_write_refused_for_a_stale_token_is_retried_once_against_a_fresh_page() {
    // The two-step made to earn its keep: the first add is refused because the
    // token aged out, and the client re-reads the page rather than surfacing an
    // error the caller can do nothing about.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/p/p/R3059518.html"))
        .respond_with(body(
            freshened("product-page.html"),
            "text/html; charset=utf-8",
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/cart/add-product"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"error":true,"msg":"The verify token has expired."}"#)
                .insert_header("content-type", "application/json"),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/cart/add-product"))
        .respond_with(json("cart.json"))
        .mount(&server)
        .await;

    let client = client(&server);
    let pdp = client.pdp("R3059518").await.unwrap();
    let cart = client.add_to_cart(&pdp, 1).await.unwrap();

    assert!(!cart.lines.is_empty());
    let paths: Vec<String> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| r.url.path().to_string())
        .collect();
    assert_eq!(
        paths,
        vec![
            "/p/p/R3059518.html",
            "/cart/add-product",
            "/p/p/R3059518.html",
            "/cart/add-product"
        ],
        "the retry re-reads the page, because only the token can have expired"
    );
}

#[tokio::test]
async fn an_ordinary_refusal_is_reported_rather_than_retried() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/p/p/R1.html"))
        .respond_with(body(
            freshened("product-page.html"),
            "text/html; charset=utf-8",
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/cart/add-product"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"error":true,"msg":"This product is out of stock."}"#)
                .insert_header("content-type", "application/json"),
        )
        .mount(&server)
        .await;

    let client = client(&server);
    let pdp = client.pdp("R1").await.unwrap();
    let err = client.add_to_cart(&pdp, 1).await.unwrap_err();
    assert!(err.to_string().contains("out of stock"), "{err}");
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        2,
        "no retry: a fresh token would fail the same way"
    );
}

#[tokio::test]
async fn the_cart_reads_back_as_lines_and_a_total() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/cart/minicart"))
        .respond_with(json("minicart.json"))
        .mount(&server)
        .await;

    let cart = client(&server).cart().await.unwrap();
    assert_eq!(cart.lines.len(), 2);
    assert_eq!(cart.quantity, 2);
    assert!(cart.subtotal.is_some());
    assert!(cart.lines.iter().all(|l| !l.uuid.is_empty()));
}

#[tokio::test]
async fn stores_come_back_as_records_for_a_region() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/on/demandware.store/Sites-twl-Site/default/Stores-FindStores",
        ))
        .and(query_param("region", "NZ-AUK"))
        .respond_with(json("stores-region.json"))
        .mount(&server)
        .await;

    // Resolved by name, not just by code.
    let stores = client(&server).stores("Auckland").await.unwrap();
    assert_eq!(stores.len(), 3);
    assert!(stores.iter().all(|s| !s.id.is_empty()));
    // Both states of each optional field, so a parse that hardcoded one would
    // fail rather than pass by luck.
    assert!(stores.iter().any(|s| s.click_and_collect == Some(true)));
    assert!(stores.iter().any(|s| s.click_and_collect == Some(false)));
    assert!(stores[0].hours_today.is_some());
    assert!(stores.iter().any(|s| s.hours_today.is_none()));
}

#[tokio::test]
async fn an_unknown_region_is_refused_before_a_request_is_made() {
    let server = MockServer::start().await;
    let err = client(&server).stores("Chatham Islands").await.unwrap_err();
    assert!(err.to_string().contains("Chatham Islands"), "{err}");
    assert!(
        server.received_requests().await.unwrap().is_empty(),
        "the finder answers an unknown region with an empty list, which reads as \
         \"no stores here\" -- so it is caught first"
    );
}

fn stock_pdp() -> Pdp {
    Pdp::parse(
        "R3059518",
        r#"<script type="application/ld+json">{"@type":"Product","name":"Thing"}</script>
           <a href="/products/stores?pid=R3059518&verify=1-c">stock</a>"#,
    )
    .unwrap()
}

#[tokio::test]
async fn per_store_stock_is_read_out_of_the_markup_the_endpoint_wraps() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/products/stores"))
        .respond_with(json("store-stock-modal.json"))
        .mount(&server)
        .await;

    let stock = client(&server).stock(&stock_pdp(), None).await.unwrap();
    assert!(!stock.is_empty());
    assert!(stock.iter().all(|s| !s.store_name.is_empty()));
    assert!(stock.iter().any(|s| s.in_stock == Some(true)));
}

#[tokio::test]
async fn narrowing_stock_to_a_region_follows_the_modals_own_signed_link() {
    // Three requests, not two. The regional endpoint will not accept a token
    // minted for the product page -- it answers `Cross-Origin Request Blocked`
    // -- so the modal has to be read for the link it carries for that region.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/products/stores"))
        .respond_with(json("store-stock-modal.json"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/products/stores/region"))
        .and(query_param("region", "NZ-NTL"))
        // The token the modal signed, not one built from the page's.
        .and(query_param(
            "verify",
            "1788496546-p1f/rDiQ4ypTamrWuYhPU0TyLMUJyLkBn1idxN3Ltdk=",
        ))
        .respond_with(json("store-stock.json"))
        .mount(&server)
        .await;

    let stock = client(&server)
        .stock(&stock_pdp(), Some("NZ-NTL"))
        .await
        .unwrap();

    assert!(stock.iter().any(|s| s.in_stock == Some(true)), "one has it");
    assert!(
        stock.iter().any(|s| s.in_stock == Some(false)),
        "and one does not, so the two states are told apart"
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn a_region_the_modal_does_not_offer_names_the_ones_it_does() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/products/stores"))
        .respond_with(json("store-stock-modal.json"))
        .mount(&server)
        .await;

    let Err(err) = client(&server).stock(&stock_pdp(), Some("NZ-XXX")).await else {
        panic!("a throttle should not read as a listing");
    };
    assert!(err.to_string().contains("NZ-AUK"), "{err}");
}

#[tokio::test]
async fn the_department_tree_parses() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/on/demandware.store/Sites-twl-Site/default/Category-GetMultipleNavigationHierarchy",
        ))
        .respond_with(json("categories.json"))
        .mount(&server)
        .await;

    let cats = client(&server)
        .categories(&["homegarden", "clothingshoesaccessories"], 0)
        .await
        .unwrap();
    assert!(cats.len() >= 2);
    assert!(cats.iter().all(|c| !c.id.is_empty()));
    // The path is what a landing page is fetched by, and is not the id.
    let home = cats.iter().find(|c| c.id == "homegarden").expect("a root");
    assert_eq!(home.path.as_deref(), Some("/c/home-garden-appliances"));
}

#[tokio::test]
async fn an_account_only_call_says_so_rather_than_failing_obscurely() {
    let server = MockServer::start().await;
    let pdp = Pdp::parse(
        "R1",
        r#"<script type="application/ld+json">{"@type":"Product","name":"Thing"}</script>
           <gep-add-to-wishlist url="/wishlist-add-product?pid=R1&verify=1-b"></gep-add-to-wishlist>"#,
    )
    .unwrap();

    let err = client(&server).add_to_wishlist(&pdp).await.unwrap_err();
    assert!(err.to_string().contains("not signed in"), "{err}");
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_page_whose_tokens_have_aged_out_is_re_read_before_the_write_is_tried() {
    // The captured page is genuinely old, so its tokens would be refused.
    // Spending a request to find that out is worse than spending one to avoid
    // it, so the client re-reads first.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/p/p/R3059518.html"))
        .respond_with(html("product-page.html"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/cart/add-product"))
        .respond_with(json("cart.json"))
        .mount(&server)
        .await;

    let client = client(&server);
    let pdp = client.pdp("R3059518").await.unwrap();
    assert!(
        pdp.stale(300),
        "a captured page is long past any sane max age"
    );

    client.add_to_cart(&pdp, 1).await.unwrap();
    let paths: Vec<String> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| r.url.path().to_string())
        .collect();
    assert_eq!(
        paths,
        vec![
            "/p/p/R3059518.html",
            "/p/p/R3059518.html",
            "/cart/add-product"
        ],
        "re-read first, then one write"
    );
}

#[tokio::test]
async fn a_cart_add_posts_a_form_and_keeps_the_token_in_the_query() {
    // Both halves matter. It is a POST -- as a GET the site answers 500 with
    // nothing to go on -- and the `verify` token stays in the query string
    // rather than moving into the body, which is where the page put it.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/p/p/R3059518.html"))
        .respond_with(body(
            freshened("product-page.html"),
            "text/html; charset=utf-8",
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/cart/add-product"))
        .and(query_param("pid", "R3059518"))
        .and(body_string_contains("quantity=3"))
        .and(body_string_contains("context=PDP"))
        .respond_with(json("cart.json"))
        .mount(&server)
        .await;

    let client = client(&server);
    let pdp = client.pdp("R3059518").await.unwrap();
    let cart = client.add_to_cart(&pdp, 3).await.unwrap();
    assert!(!cart.lines.is_empty());

    let request = &server.received_requests().await.unwrap()[1];
    assert!(
        request.url.query().unwrap_or_default().contains("verify="),
        "the signed token stays where the page wrote it"
    );
}

#[tokio::test]
async fn the_minicart_is_read_as_a_background_request_not_as_a_page() {
    // It carries no token, but it is still an XHR endpoint: fetched as a page
    // the site refuses it, which is a confusing thing for a read to do.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/cart/minicart"))
        .respond_with(json("minicart.json"))
        .mount(&server)
        .await;

    client(&server).cart().await.unwrap();
    let request = &server.received_requests().await.unwrap()[0];
    assert_eq!(
        request.headers.get("x-requested-with").unwrap(),
        "fetch",
        "the site tells a background request from a navigation by its headers"
    );
    assert_eq!(
        request.headers.get("sec-fetch-mode").unwrap(),
        "same-origin"
    );
}

#[tokio::test]
async fn a_removal_reads_the_basket_it_left_behind() {
    // The fifth name for one model. `Cart-RemoveProductLineItem` answers with
    // `basket`, and reading only the other four made every removal report an
    // empty cart while the site had quietly kept the rest.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/cart/minicart"))
        .respond_with(json("minicart.json"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/on/demandware.store/Sites-twl-Site/default/Cart-RemoveProductLineItem",
        ))
        .respond_with(json("cart-removed.json"))
        .mount(&server)
        .await;

    let client = client(&server);
    let mut before = client.cart().await.unwrap();
    let cart = client.remove_line(&before.lines.remove(0)).await.unwrap();

    assert_eq!(cart.lines.len(), 2, "the lines that survived the removal");
    assert_eq!(
        cart.quantity, 3,
        "the basket's own count, not the zero the envelope repeats for the line it took out"
    );
    assert_eq!(cart.subtotal.as_deref(), Some("$24.97"));
}

// ---- the wishlist ----

/// A client whose cookies speak for an account.
///
/// The wishlist is the one part of the storefront that genuinely belongs to a
/// person, so every call here refuses a guest before it reaches the network.
fn signed_in(server: &MockServer) -> Client {
    let cookies =
        std::collections::BTreeMap::from([("cc-nx_twl".to_string(), "signed-in".to_string())]);
    let http = net_kit::http::build(twlnz_api::client_spec()).expect("http client");
    Client::new(
        http,
        Endpoints::default().with_origin(server.uri()),
        Session::from_cookies(cookies),
    )
    .with_pace(net_kit::Pace::off())
}

#[tokio::test]
async fn the_wishlist_page_reads_as_saved_items() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/wishlist"))
        .respond_with(html("wishlist-page.html"))
        .mount(&server)
        .await;

    let wishlist = signed_in(&server).wishlist().await.unwrap();

    assert_eq!(wishlist.total, Some(2), "the page's own heading");
    assert!(wishlist.complete(), "two rows for a list of two");
    let first = &wishlist.items[0];
    assert_eq!(first.id, "R2837766");
    assert_eq!(
        first.uuid, "69aa74651283fdb1388801709a",
        "the row, not the product"
    );
    assert_eq!(first.name, "Workspace 925 Mobile 3 Drawer");
    assert_eq!(first.price.label().as_deref(), Some("$279.00"));
    assert_eq!(first.stock.as_deref(), Some("In stock"));

    // The second row is a saved *variant*, which is the case that makes the
    // labels worth carrying: without them two colours of one shirt are the
    // same row printed twice.
    let second = &wishlist.items[1];
    assert_eq!(second.quantity, 2, "the row's own quantity, not a default");
    assert_eq!(second.variation, ["Green Dark", "S"]);
}

#[tokio::test]
async fn a_saved_row_carries_the_token_that_puts_it_in_the_basket() {
    // The wishlist is where this crate's two-step collapses: the add-to-cart
    // token is minted into the row, so nothing has to fetch a product page.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/wishlist"))
        .respond_with(html("wishlist-page.html"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/cart/add-product"))
        .respond_with(json("cart.json"))
        .mount(&server)
        .await;

    let client = signed_in(&server);
    let wishlist = client.wishlist().await.unwrap();
    let saved = &wishlist.items[0];
    // The message says what is missing rather than printing the URL: it is a
    // signed one, and a failing test should not put a token in the log.
    assert!(
        saved
            .add_to_cart
            .as_deref()
            .is_some_and(|u| u.contains("verify=")),
        "the saved row carries no signed add-to-cart url"
    );

    let cart = client.add_saved_to_cart(saved, 1).await.unwrap();
    assert!(!cart.lines.is_empty());
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        2,
        "the page and the add -- no product page in between"
    );
}

#[tokio::test]
async fn a_wishlist_write_is_addressed_by_the_row_rather_than_the_product() {
    // And the removal is a GET while the quantity change beside it is a POST.
    // Both are the wishlist's own controllers, both are called by the same
    // script, and posting to this one is answered with a 500 that says nothing
    // -- so the method is pinned here rather than left to look incidental.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/wishlist"))
        .respond_with(html("wishlist-page.html"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/on/demandware.store/Sites-twl-Site/default/Wishlist-RemoveProduct",
        ))
        .and(query_param("uuid", "69aa74651283fdb1388801709a"))
        .and(query_param("pid", "R2837766"))
        .respond_with(body(
            r#"{"action":"Wishlist-RemoveProduct","success":true}"#.to_string(),
            "application/json",
        ))
        .mount(&server)
        .await;

    let client = signed_in(&server);
    let wishlist = client.wishlist().await.unwrap();
    client
        .remove_from_wishlist(&wishlist.items[0])
        .await
        .unwrap();
}

#[tokio::test]
async fn a_write_the_site_says_did_not_work_is_not_reported_as_done() {
    // These controllers answer with a flag and nothing else, so `success:false`
    // on a 200 is the only failure there is to see.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/wishlist"))
        .respond_with(html("wishlist-page.html"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/on/demandware.store/Sites-twl-Site/default/Wishlist-UpdateProductQuantity",
        ))
        .respond_with(body(
            r#"{"action":"Wishlist-UpdateProductQuantity","success":false}"#.to_string(),
            "application/json",
        ))
        .mount(&server)
        .await;

    let client = signed_in(&server);
    let wishlist = client.wishlist().await.unwrap();
    let err = client
        .set_wishlist_quantity(&wishlist.items[0], 3)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("refused"), "{err}");
}

#[tokio::test]
async fn a_page_that_is_not_the_wishlist_is_not_an_empty_wishlist() {
    // A lapsed session is served the sign-in wall with a 200. Reporting that as
    // "nothing saved" would be a lie about the account rather than about the
    // request.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/wishlist"))
        .respond_with(body(
            "<html><body><h1>Sign in</h1></body></html>".to_string(),
            "text/html",
        ))
        .mount(&server)
        .await;

    let err = signed_in(&server).wishlist().await.unwrap_err();
    assert!(err.to_string().contains("the wishlist"), "{err}");
}

#[tokio::test]
async fn the_wishlist_refuses_a_guest_before_it_asks() {
    let server = MockServer::start().await;
    let err = client(&server).wishlist().await.unwrap_err();
    assert!(err.to_string().contains("not signed in"), "{err}");
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_session_the_storefront_has_stopped_recognising_is_reported_as_such() {
    // The tell is where the request landed, not what the page said: a guest
    // asking for the account page is redirected to the sign-in page, and that
    // survives every rewording of the markup on it.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/account"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("location", format!("{}/login", server.uri())),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/login"))
        .respond_with(body(
            "<form><input name=\"dwfrm_login_email\"></form>".into(),
            "text/html",
        ))
        .mount(&server)
        .await;

    assert!(!signed_in(&server).verify().await.unwrap());
}

#[tokio::test]
async fn a_session_that_is_still_good_needs_no_sign_in() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/account"))
        .respond_with(body("<h1>My account</h1>".into(), "text/html"))
        .mount(&server)
        .await;

    // Only the one request: a probe that renewed on its own would spend a
    // password to answer a question about whether it needed to.
    assert!(signed_in(&server).verify().await.unwrap());
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}
