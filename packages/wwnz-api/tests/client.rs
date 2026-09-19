//! The GraphQL client against a mock server.
//!
//! Every operation goes to the same path, so mocks are routed on the `op-name`
//! query parameter -- which is the only thing distinguishing them.

use net_kit::{ClientSpec, Fault};
use serde_json::json;
use wiremock::matchers::{body_partial_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};
use wwnz_api::{Change, Client, Endpoints, Filter, SearchBy, Session};

fn http() -> net_kit::wreq::Client {
    net_kit::http::build(ClientSpec::new(
        wwnz_api::EMULATION,
        net_kit::wreq::redirect::Policy::none(),
    ))
    .expect("building a client")
}

fn client(server: &MockServer) -> Client {
    Client::new(
        http(),
        Endpoints::default().with_origin(server.uri()),
        Session::guest("guest-token"),
    )
}

async fn mount(server: &MockServer, operation: &str, data: serde_json::Value) {
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .and(query_param("op-name", operation))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": data })))
        .mount(server)
        .await;
}

fn product(sku: &str, name: &str, dollars: f64) -> serde_json::Value {
    json!({
        "__typename": "ProductSummary",
        "sku": sku,
        "productName": name,
        "brand": "Woolworths",
        "slug": "a-slug",
        "variants": [{
            "variantKey": format!("{sku}-EA"),
            "unitOfMeasure": "EACH",
            "availabilityStatus": "IN_STOCK",
            "variantPrice": { "sellingPrice": dollars, "cupPrice": 2.25, "cupUnit": "1L" }
        }],
        "categoryHierarchyNames": { "lvl1": ["Fridge & Deli"] }
    })
}

#[tokio::test]
async fn search_maps_products_and_converts_dollars_to_exact_cents() {
    let server = MockServer::start().await;
    mount(
        &server,
        "ProductSearch",
        json!({ "My": { "products": {
            "results": [product("282768", "Blue Milk", 7.19)],
            "totalCount": 1, "totalPages": 1
        }}}),
    )
    .await;

    let result = client(&server)
        .search(&SearchBy::Keyword("milk".into()), 20, "RELEVANCE", false)
        .await
        .unwrap();
    let p = &result.products[0];
    // 7.19 is not representable in binary floating point; truncating would
    // report $7.18.
    assert_eq!(p.price_cents, Some(719));
    assert_eq!(p.variant_key, "282768-EA");
    assert_eq!(p.in_stock, Some(true));
    assert!(!p.sponsored);
}

#[tokio::test]
async fn an_ad_slot_is_kept_and_marked_and_an_unknown_row_is_skipped() {
    let server = MockServer::start().await;
    mount(
        &server,
        "ProductSearch",
        json!({ "My": { "products": {
            "results": [
                { "__typename": "SponsoredProduct", "sku": "1", "productName": "Ad", "variants": [] },
                { "__typename": "EditorialTile", "headline": "not a product" },
                product("2", "Real", 1.0)
            ],
            "totalCount": 2, "totalPages": 1
        }}}),
    )
    .await;

    let result = client(&server)
        .search(&SearchBy::Specials, 20, "RELEVANCE", true)
        .await
        .unwrap();
    assert_eq!(
        result.products.len(),
        2,
        "the tile is skipped, the ad is kept"
    );
    assert!(result.products[0].sponsored, "and marked");
}

#[tokio::test]
async fn a_page_of_nothing_but_ad_slots_is_not_the_end_of_the_results() {
    let server = MockServer::start().await;
    // Page 0 is all non-products; page 1 has the real ones.
    Mock::given(method("POST"))
        .and(query_param("op-name", "ProductSearch"))
        .and(body_partial_json(
            json!({ "variables": { "searchInput": { "byKeyword": { "pageIndex": 0 } } } }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({ "data": { "My": { "products": {
                "results": [{ "__typename": "EditorialTile" }],
                "totalCount": 1, "totalPages": 2
            }}}}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(query_param("op-name", "ProductSearch"))
        .and(body_partial_json(
            json!({ "variables": { "searchInput": { "byKeyword": { "pageIndex": 1 } } } }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({ "data": { "My": { "products": {
                "results": [product("2", "Real", 1.0)],
                "totalCount": 1, "totalPages": 2
            }}}}),
        ))
        .mount(&server)
        .await;

    let result = client(&server)
        .search(&SearchBy::Keyword("milk".into()), 20, "RELEVANCE", false)
        .await
        .unwrap();
    assert_eq!(
        result.products.len(),
        1,
        "it kept paging past the ad-only page"
    );
}

#[tokio::test]
async fn stores_are_mapped_with_their_distance() {
    let server = MockServer::start().await;
    mount(
        &server,
        "SearchLocations",
        json!({ "locations": { "locations": [{
            "id": "1", "storeId": "9048", "name": "Regent Woolworths", "distance": 2.34,
            "address": { "lines": { "line1": "1 Example St" }, "locality": { "suburb": "Regent", "city": "Whangarei" } }
        }]}}),
    )
    .await;

    let stores = client(&server).stores(Some("regent"), 10).await.unwrap();
    assert_eq!(
        stores[0].id, "9048",
        "the store id wins over the location id"
    );
    assert_eq!(stores[0].distance_km, Some(2.34));
    assert_eq!(stores[0].suburb.as_deref(), Some("Regent"));
}

#[tokio::test]
async fn the_cart_separates_the_line_total_from_the_order_subtotal() {
    let server = MockServer::start().await;
    mount(
        &server,
        "CustomerCart",
        json!({ "customerCart": {
            "key": "cart-1",
            "totalItemQuantity": 2.5,
            "totalUniqueProductSku": 2,
            "pricing": {
                "orderSubtotal": { "afterDiscountAsCents": 5000 },
                "productSubtotal": { "afterDiscountAsCents": 4100 }
            },
            "lineItems": [{
                "sku": "282768", "productVariantSku": "282768-EA", "quantity": 1,
                "lineTotal": { "afterDiscountAsCents": 719 },
                "product": { "brand": "Anchor", "variants": [{ "name": "Blue Milk", "key": "282768-EA" }] }
            }],
            "fulfilment": { "fulfilmentProposition": { "method": "Pickup", "store": { "storeId": "9048", "name": "Regent Woolworths" } } }
        }}),
    )
    .await;

    let cart = client(&server).cart().await.unwrap();
    assert_eq!(cart.items_cents, Some(4100), "the lines alone");
    assert_eq!(cart.subtotal_cents, Some(5000), "products plus fees");
    assert_eq!(cart.total_items, 2.5, "quantities, not distinct products");
    assert_eq!(cart.store_name.as_deref(), Some("Regent Woolworths"));
}

#[tokio::test]
async fn a_line_whose_variant_type_carries_no_name_is_named_from_search() {
    let server = MockServer::start().await;
    mount(
        &server,
        "CustomerCart",
        json!({ "customerCart": {
            "key": "cart-1",
            "lineItems": [
                {
                    "sku": "282768", "productVariantSku": "282768-EA", "quantity": 1,
                    "product": { "brand": "Anchor", "variants": [{ "name": "Blue Milk", "key": "282768-EA" }] }
                },
                // General merchandise: the document spreads no fragment that
                // matches, so the variant arrives with neither name nor key.
                {
                    "sku": "639411", "productVariantSku": "639411-EA", "quantity": 1,
                    "product": { "brand": null, "variants": [{}] }
                }
            ]
        }}),
    )
    .await;
    mount(
        &server,
        "ProductSearch",
        json!({ "My": { "products": {
            "results": [product("639411", "Nylon Tongs 23cm", 7.00)],
            "totalCount": 1, "totalPages": 1
        }}}),
    )
    .await;

    let cart = client(&server).cart().await.unwrap();
    assert_eq!(
        cart.lines[0].name, "Blue Milk",
        "named lines are left alone"
    );
    assert_eq!(cart.lines[1].name, "Nylon Tongs 23cm");
    assert_eq!(cart.lines[1].sku, "639411");
    assert_eq!(cart.lines[1].brand.as_deref(), Some("Woolworths"));
}

#[tokio::test]
async fn a_line_search_cannot_name_is_still_listed() {
    let server = MockServer::start().await;
    mount(
        &server,
        "CustomerCart",
        json!({ "customerCart": {
            "key": "cart-1",
            "lineItems": [{
                "sku": "639411", "productVariantSku": "639411-EA", "quantity": 1,
                "lineTotal": { "afterDiscountAsCents": 700 },
                "product": { "variants": [{}] }
            }]
        }}),
    )
    .await;
    mount(
        &server,
        "ProductSearch",
        json!({ "My": { "products": { "results": [], "totalCount": 0, "totalPages": 1 } } }),
    )
    .await;

    let cart = client(&server).cart().await.unwrap();
    assert_eq!(cart.lines.len(), 1);
    assert_eq!(cart.lines[0].name, "", "blank, but the row survives");
    assert_eq!(cart.lines[0].total_cents, Some(700));
}

#[tokio::test]
async fn setting_quantities_sends_variant_keys_and_returns_the_new_cart() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(query_param("op-name", "SetCartLineItemQuantity"))
        .and(body_partial_json(json!({ "variables": { "input": {
            "cartLineItemQuantityUpdates": [{ "variantKey": "282768-EA", "quantity": 3 }]
        }}})))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({ "data": { "setCartLineItemQuantity": { "key": "cart-1", "lineItems": [] } } }),
        ))
        .mount(&server)
        .await;

    let cart = client(&server)
        .cart_set(&[Change {
            variant_key: "282768-EA".into(),
            quantity: 3.0,
        }])
        .await
        .unwrap();
    assert_eq!(cart.id.as_deref(), Some("cart-1"));
}

#[tokio::test]
async fn order_history_pages_and_maps_its_destination() {
    let server = MockServer::start().await;
    mount(
        &server,
        "Orders",
        json!({ "orders": {
            "totalCount": 1, "totalPages": 1,
            "results": [{
                "orderNumber": "WN100061750",
                "createdDateTime": "2026-09-02T22:44:49.280Z",
                "orderStatus": "IN_PROGRESS",
                "isAmendable": false,
                "total": { "afterDiscountInCents": 43225 },
                "fulfilments": [{ "method": "delivery", "startTime": "2026-09-03T17:30:00.000+12:00",
                                  "address": { "lines": { "line1": "1 Example St" } } }]
            }]
        }}),
    )
    .await;

    let page = client(&server).orders(20, Filter::All).await.unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.orders[0].number, "WN100061750");
    assert_eq!(page.orders[0].total_cents, Some(43225));
    assert_eq!(page.orders[0].destination.as_deref(), Some("1 Example St"));
}

#[tokio::test]
async fn one_order_comes_back_with_its_lines_and_fees() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(query_param("op-name", "OrderDetails"))
        .and(body_partial_json(
            json!({ "variables": { "orderNumber": "WN100061750" } }),
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "data": { "order": {
                "orderNumber": "WN100061750",
                "orderStatus": "IN_PROGRESS",
                "createdDateTime": "2026-09-02T22:44:49.280Z",
                "isAmendable": false,
                "orderTotalInCents": 0,
                "estimatedTotalInCents": 43225,
                "fees": [
                    { "type": "standardDeliveryFee", "amountInCents": 900 },
                    { "type": "bagFee", "amountInCents": 150 }
                ],
                "fulfilments": [{
                    "method": "delivery", "kind": "delivery-truck",
                    "startTime": "2026-09-03T17:30:00.000+12:00",
                    "endTime": "2026-09-03T20:00:00.000+12:00",
                    "fulfilmentLocation": { "name": "Regent Woolworths", "storeId": "9048" }
                }],
                "lineItems": [{
                    "productId": "133211", "productKey": "133211", "skuId": "133211-EA",
                    "quantity": 6, "totalPriceAsCents": 563,
                    "unitPriceAfterDiscountAsCents": 94, "totalSavingAsCents": 1,
                    "product": { "name": "Woolworths Fresh Bananas" }
                }]
            }}})),
        )
        .mount(&server)
        .await;

    let order = client(&server).order("WN100061750").await.unwrap();
    assert_eq!(order.number, "WN100061750");
    // An order still being picked reports a zero total; the estimate is real.
    assert_eq!(order.total_cents, Some(0));
    assert_eq!(order.total(), Some(43225));
    assert_eq!(order.fees.len(), 2);
    assert_eq!(order.lines[0].variant_key, "133211-EA");
    assert_eq!(order.lines[0].quantity, 6.0);
    assert_eq!(order.location_store_id.as_deref(), Some("9048"));
}

#[tokio::test]
async fn a_missing_order_is_reported_rather_than_returning_an_empty_one() {
    let server = MockServer::start().await;
    mount(&server, "OrderDetails", json!({ "order": null })).await;

    let err = client(&server).order("WN999").await.unwrap_err();
    assert!(err.to_string().contains("WN999"), "{err}");
}

#[tokio::test]
async fn an_unauthenticated_extension_on_a_200_is_read_as_not_signed_in() {
    // The API signals this with a code on a *successful* response, which is why
    // matching English in an error chain was never going to be reliable.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(query_param("op-name", "CustomerCart"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "errors": [{ "message": "Unexpected error", "extensions": { "code": "AUTH_NOT_AUTHENTICATED" } }]
        })))
        .mount(&server)
        .await;

    let err = client(&server).cart().await.unwrap_err();
    assert!(matches!(err, wwnz_api::Error::NotSignedIn), "{err:?}");
    assert_eq!(err.auth(), Some(net_kit::AuthFault::Missing));
    assert!(err.is_lapsed());
}

#[tokio::test]
async fn a_401_naming_session_expired_is_told_apart_from_never_signing_in() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(query_param("op-name", "CustomerCart"))
        .respond_with(ResponseTemplate::new(401).set_body_string(r#"{"error":"session_expired"}"#))
        .mount(&server)
        .await;

    let err = client(&server).cart().await.unwrap_err();
    assert!(matches!(err, wwnz_api::Error::SessionExpired), "{err:?}");
    assert_eq!(err.auth(), Some(net_kit::AuthFault::Expired));
}

#[tokio::test]
async fn a_client_with_no_way_to_sign_in_again_says_so_rather_than_looping() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(query_param("op-name", "CustomerCart"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "errors": [{ "extensions": { "code": "AUTH_NOT_AUTHENTICATED" } }]
        })))
        .mount(&server)
        .await;

    // No Reauth attached, so renew() refuses instead of retrying forever.
    let err = client(&server).renew().await.unwrap_err();
    assert!(
        matches!(err, wwnz_api::Error::SessionUnrenewable),
        "{err:?}"
    );
}

#[tokio::test]
async fn setting_the_store_returns_the_name_it_bound_to() {
    let server = MockServer::start().await;
    mount(
        &server,
        "SetCartShoppingMode",
        json!({ "setCartShoppingMode": { "shoppingMode": { "pickupLocation": { "id": "9048", "name": "Regent Woolworths" } } } }),
    )
    .await;

    let name = client(&server).set_store("9048").await.unwrap();
    assert_eq!(name.as_deref(), Some("Regent Woolworths"));
}

#[tokio::test]
async fn the_department_tree_comes_back_nested() {
    let server = MockServer::start().await;
    mount(
        &server,
        "GetAllCategories",
        json!({ "My": { "categories": {
            "key": "root", "name": "All", "slug": "all", "level": 0,
            "children": [{ "key": "9-1", "name": "Fridge & Deli", "displaySlug": "fridge-deli", "level": 1,
                           "children": [{ "key": "9-1-1", "name": "Milk", "slug": "milk", "level": 2 }] }]
        }}}),
    )
    .await;

    let tree = client(&server).categories().await.unwrap();
    assert_eq!(tree.children[0].slug, "fridge-deli", "displaySlug wins");
    assert_eq!(tree.find("milk").unwrap().key, "9-1-1");
    assert_eq!(tree.flatten().len(), 3);
}
