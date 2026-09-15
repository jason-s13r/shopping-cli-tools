//! Turning what the two services send into [`crate::domain`] types.
//!
//! The Algolia half is read as a map with per-field accessors rather than a
//! struct. Index documents are denormalised marketing data: `colour` is a
//! string on one product and `null` on the next, `size` arrives as a number
//! where the unit is implied, and `prices.labels` is absent entirely on
//! anything not on promotion. A struct fails the whole hit on any one
//! mismatch, so a renamed field would cost the command rather than a column.
//!
//! The OCC half is a real API and holds its shapes, so it is deserialised
//! properly -- but every field stays optional, because Hybris omits rather
//! than nulls.

use serde::Deserialize;

use crate::domain::{
    Address, Cart, CartLine, Category, Customer, Delivery, Facet, FacetOption, Listing, Money,
    Order, OrderLine, OrderPage, Product, ProductDetail, Promotion, Stock, StockLevel, Store,
    Variant, Wishlist, WishlistItem,
};
use crate::endpoints::{Algolia, ALGOLIA_CONFIG_KEY};
use crate::error::{Error, Result};

// ---------------------------------------------------------------- accessors

/// A string, however it was typed. Treats the empty string as absent.
pub fn str_of(v: &serde_json::Value, key: &str) -> Option<String> {
    let text = match v.get(key)? {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        _ => return None,
    };
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// A number, whether it arrived as one or as a string.
pub fn f64_of(v: &serde_json::Value, key: &str) -> Option<f64> {
    match v.get(key)? {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

pub fn i64_of(v: &serde_json::Value, key: &str) -> Option<i64> {
    f64_of(v, key).map(|n| n as i64)
}

/// A boolean, whether it arrived as one, as `0`/`1`, or as `"true"`.
pub fn bool_of(v: &serde_json::Value, key: &str) -> Option<bool> {
    match v.get(key)? {
        serde_json::Value::Bool(b) => Some(*b),
        serde_json::Value::Number(n) => Some(n.as_f64()? != 0.0),
        serde_json::Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "1" => Some(true),
            "false" | "no" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// The first of several spellings that is present, for a field the vendor has
/// renamed between templates.
pub fn any_str(v: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| str_of(v, k))
}

/// An array of codes, which the index sends as numbers.
fn codes(v: &serde_json::Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|i| match i {
                    serde_json::Value::Number(n) => Some(n.to_string()),
                    serde_json::Value::String(s) => Some(s.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

// ------------------------------------------------------------------ algolia

/// One page of results, as the multi-query endpoint answers.
pub fn listing(body: &serde_json::Value, query: Option<String>) -> Result<Listing> {
    let result = body
        .get("results")
        .and_then(|r| r.get(0))
        .ok_or_else(|| Error::Shape("the search index answered with no results block".into()))?;

    let hits = result
        .get("hits")
        .and_then(|h| h.as_array())
        .map(|hits| hits.iter().map(product).collect())
        .unwrap_or_default();

    Ok(Listing {
        products: hits,
        total: f64_of(result, "nbHits").unwrap_or_default() as u64,
        page: f64_of(result, "page").unwrap_or_default() as u64,
        pages: f64_of(result, "nbPages").unwrap_or_default() as u64,
        page_size: f64_of(result, "hitsPerPage").unwrap_or(crate::search::PAGE_SIZE as f64) as u64,
        facets: facets(result),
        query,
    })
}

/// One index document.
pub fn product(hit: &serde_json::Value) -> Product {
    let prices = hit
        .get("prices")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    // The index carries the ticket price and the promotional one separately,
    // and sends the promo field on everything -- equal to the ticket price
    // when nothing is on special. Which is "now" therefore depends on whether
    // they differ, not on which fields are present.
    let rrp = f64_of(&prices, "nationalRRP").or_else(|| f64_of(&prices, "localRRP"));
    let promo = f64_of(&prices, "nationalPromo").or_else(|| f64_of(&prices, "specificPromos"));
    let (price, was_price) = match (rrp, promo) {
        (Some(rrp), Some(promo)) if promo < rrp => (Some(promo), Some(rrp)),
        (rrp, _) => (rrp, None),
    };

    Product {
        // The index keys a product by the id in its URL rather than a `code`
        // field, so fall back to parsing it out of the path.
        code: any_str(hit, &["objectID", "code", "productCode"])
            .or_else(|| str_of(hit, "url").and_then(|u| code_from_url(&u)))
            .unwrap_or_default(),
        name: any_str(hit, &["name", "title"]).unwrap_or_default(),
        brand: any_str(hit, &["brandName", "brand"]),
        price,
        was_price,
        size: str_of(hit, "size"),
        colour: any_str(hit, &["colour", "supplierColour"]),
        unit: str_of(hit, "unit"),
        url: str_of(hit, "url"),
        image: hit
            .get("images")
            .and_then(|i| any_str(i, &["img515Wx515H", "img300Wx300H", "img96Wx96H"])),
        available_nationwide: bool_of(hit, "availableNationWide").unwrap_or(false),
        stores_with_stock: codes(hit, "storesWithStock"),
        click_and_collect: codes(hit, "clickAndCollect"),
        promotions: promotions(&prices),
    }
}

fn promotions(prices: &serde_json::Value) -> Vec<Promotion> {
    prices
        .get("labels")
        .and_then(|l| l.as_array())
        .map(|labels| {
            labels
                .iter()
                .map(|l| Promotion {
                    badge: str_of(l, "priceBadge"),
                    offer_id: str_of(l, "offerId"),
                    starts: str_of(l, "startDate"),
                    ends: str_of(l, "endDate"),
                    text: str_of(l, "tickerTapeText"),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `/shop/wattyl-fence-finish-paint-10l/p/174969` -> `174969`.
pub fn code_from_url(url: &str) -> Option<String> {
    let tail = url.rsplit('/').next()?.trim();
    (!tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit())).then(|| tail.to_string())
}

fn facets(result: &serde_json::Value) -> Vec<Facet> {
    let Some(facets) = result.get("facets").and_then(|f| f.as_object()) else {
        return Vec::new();
    };
    let mut out: Vec<Facet> = facets
        .iter()
        .map(|(name, values)| {
            let mut options: Vec<FacetOption> = values
                .as_object()
                .map(|v| {
                    v.iter()
                        .map(|(value, count)| FacetOption {
                            value: value.clone(),
                            count: count.as_f64().unwrap_or_default() as u64,
                        })
                        .collect()
                })
                .unwrap_or_default();
            // Commonest first, then alphabetical: the site's own ordering, and
            // a map's iteration order is not stable enough to print.
            options.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
            Facet {
                name: name.clone(),
                options,
            }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The query-suggestions index, which keys its documents by the phrase.
pub fn suggestions(body: &serde_json::Value) -> Vec<String> {
    body.get("results")
        .and_then(|r| r.get(0))
        .and_then(|r| r.get("hits"))
        .and_then(|h| h.as_array())
        .map(|hits| {
            hits.iter()
                .filter_map(|h| any_str(h, &["query", "objectID"]))
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------- OCC

#[derive(Debug, Deserialize)]
pub struct OccErrors {
    #[serde(default)]
    pub errors: Vec<OccError>,
}

#[derive(Debug, Deserialize)]
pub struct OccError {
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
}

/// Hybris reports business failures as an `errors[]` array with a non-2xx
/// status, so the body is worth reading before the status is reported.
pub fn occ_error(operation: &'static str, body: &str) -> Option<Error> {
    let parsed: OccErrors = serde_json::from_str(body).ok()?;
    let first = parsed.errors.into_iter().next()?;
    Some(Error::Occ {
        operation,
        message: first
            .message
            .or(first.kind)
            .unwrap_or_else(|| "no reason given".into()),
    })
}

/// The Algolia application out of `/config/key`.
///
/// `value` is a JSON *string*, not an object, so it is parsed twice. Read as a
/// map: the same property also carries index names and page sizes this crate
/// does not use, and a struct would fail the lot when one of those changes.
pub fn algolia_config(v: &serde_json::Value) -> Result<Algolia> {
    let raw = str_of(v, "value")
        .ok_or_else(|| Error::Shape(format!("{ALGOLIA_CONFIG_KEY} carried no value")))?;
    let config: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| Error::decode(format!("reading {ALGOLIA_CONFIG_KEY}"), e))?;
    match (str_of(&config, "appId"), str_of(&config, "searchApiKey")) {
        (Some(app_id), Some(search_key)) => Ok(Algolia::new(app_id, search_key)),
        _ => Err(Error::Shape(format!(
            "{ALGOLIA_CONFIG_KEY} named no Algolia application"
        ))),
    }
}

fn money(v: &serde_json::Value) -> Option<Money> {
    let value = f64_of(v, "value")?;
    Some(Money {
        value,
        currency: str_of(v, "currencyIso").unwrap_or_else(|| "NZD".into()),
        formatted: str_of(v, "formattedValue"),
    })
}

fn money_at(v: &serde_json::Value, key: &str) -> Option<Money> {
    money(v.get(key)?)
}

pub fn product_detail(v: &serde_json::Value) -> Result<ProductDetail> {
    let code = str_of(v, "code")
        .ok_or_else(|| Error::Shape("the product answer carried no code".into()))?;

    let specifications = v
        .get("mitre10ProductSpecifications")
        .and_then(|s| s.get("entry"))
        .and_then(|e| e.as_array())
        .map(|entries| {
            entries
                .iter()
                .filter_map(|e| Some((any_str(e, &["key", "name"])?, any_str(e, &["value"])?)))
                .collect()
        })
        .unwrap_or_default();

    let images = v
        .get("images")
        .and_then(|i| i.as_array())
        .map(|images| {
            images
                .iter()
                .filter(|i| str_of(i, "format").is_none_or(|f| f.contains("1500") || f == "zoom"))
                .filter_map(|i| str_of(i, "url"))
                .collect()
        })
        .unwrap_or_default();

    Ok(ProductDetail {
        code,
        title: str_of(v, "title"),
        name: any_str(v, &["name", "title"]).unwrap_or_default(),
        brand: any_str(v, &["brandName", "subBrandName"]),
        summary: str_of(v, "summary"),
        description: any_str(v, &["longDescription", "description", "shortDescription"]),
        price: money_at(v, "price"),
        // Only a genuine reduction: Hybris sends `regularPrice` equal to
        // `price` on everything that is not on promotion.
        regular_price: match (money_at(v, "price"), money_at(v, "regularPrice")) {
            (Some(now), Some(was)) if was.value > now.value => Some(was),
            _ => None,
        },
        price_badge: v.get("priceLabel").and_then(|l| str_of(l, "priceBadge")),
        rating: f64_of(v, "averageRating"),
        reviews: f64_of(v, "numberOfReviews").map(|n| n as u64),
        model_number: str_of(v, "modelNumber"),
        unit: any_str(v, &["uom", "ccDisplayName"]),
        net_content: f64_of(v, "netContent"),
        net_content_uom: str_of(v, "netContentUOM"),
        store: str_of(v, "preferredStoreId"),
        store_name: str_of(v, "preferredStore"),
        store_stock: i64_of(v, "preferredStoreStock")
            .or_else(|| v.get("stock").and_then(|s| i64_of(s, "stockLevel"))),
        stock_indicator: str_of(v, "stockIndicator")
            .or_else(|| v.get("stock").and_then(|s| str_of(s, "stockLevelStatus")))
            .map(|s| StockLevel::parse(&s)),
        click_and_collect: bool_of(v, "clickToCollect").unwrap_or(false),
        home_delivery: bool_of(v, "homeDelivery").unwrap_or(false),
        purchasable: bool_of(v, "purchasable").unwrap_or(false),
        max_order_quantity: i64_of(v, "maxOrderQuantity"),
        categories: v
            .get("categories")
            .and_then(|c| c.as_array())
            .map(|cs| cs.iter().filter_map(|c| str_of(c, "code")).collect())
            .unwrap_or_default(),
        breadcrumbs: v
            .get("breadcrumbs")
            .and_then(|b| b.as_array())
            .map(|bs| bs.iter().filter_map(|b| str_of(b, "name")).collect())
            .unwrap_or_default(),
        specifications,
        images,
        url: any_str(v, &["url", "canonicalUrl"]),
        variants: v
            .get("variantOptions")
            .and_then(|o| o.as_array())
            .map(|os| {
                os.iter()
                    .map(|o| Variant {
                        code: str_of(o, "code").unwrap_or_default(),
                        name: any_str(o, &["name", "displayName"]),
                        price: money_at(o, "priceData"),
                        url: str_of(o, "url"),
                    })
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// A `priceForProducts` or `recommendedProducts` batch, which answers with the
/// same product shape as the detail endpoint.
pub fn product_batch(v: &serde_json::Value) -> Vec<ProductDetail> {
    v.get("products")
        .and_then(|p| p.as_array())
        .map(|ps| ps.iter().filter_map(|p| product_detail(p).ok()).collect())
        .unwrap_or_default()
}

pub fn stock(v: &serde_json::Value) -> Vec<Stock> {
    v.get("stockIndicator")
        .and_then(|s| s.as_array())
        .map(|entries| {
            entries
                .iter()
                .filter_map(|e| {
                    let store = str_of(e, "storeCode")?;
                    Some(Stock {
                        store,
                        store_name: str_of(e, "storeDisplayName").unwrap_or_default(),
                        level: str_of(e, "stockIndicator")
                            .map(|s| StockLevel::parse(&s))
                            .unwrap_or(StockLevel::Unknown),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn address(v: &serde_json::Value) -> Address {
    Address {
        line1: str_of(v, "line1"),
        line2: str_of(v, "line2"),
        suburb: str_of(v, "town").or_else(|| str_of(v, "district")),
        town: str_of(v, "city").or_else(|| str_of(v, "town")),
        postcode: str_of(v, "postalCode"),
        formatted: any_str(v, &["formattedAddress", "nzPostalAddress"]),
    }
}

pub fn store(v: &serde_json::Value) -> Store {
    let addr = v.get("address");
    Store {
        // `name` is the numeric code on this endpoint and `displayName` the
        // human one -- the opposite of what the field names suggest.
        code: any_str(v, &["staticStoreCode", "name"]).unwrap_or_default(),
        sap_code: str_of(v, "storeCode"),
        name: any_str(v, &["displayName", "name"]).unwrap_or_default(),
        address: addr.map(address),
        phone: addr.and_then(|a| str_of(a, "phone")),
        email: addr.and_then(|a| str_of(a, "email")),
        latitude: v.get("geoPoint").and_then(|g| f64_of(g, "latitude")),
        longitude: v.get("geoPoint").and_then(|g| f64_of(g, "longitude")),
        postcode_group: str_of(v, "postcodeGroup"),
        today_hours: v
            .get("todayOpeningHours")
            .and_then(|h| any_str(h, &["formattedHour", "openingTime"])),
        express_delivery: bool_of(v, "expressDeliveryEnabled").unwrap_or(false),
    }
}

pub fn stores(v: &serde_json::Value) -> Vec<Store> {
    v.get("myStoreList")
        .or_else(|| v.get("stores"))
        .and_then(|s| s.as_array())
        .map(|ss| ss.iter().map(store).collect())
        .unwrap_or_default()
}

pub fn category(v: &serde_json::Value) -> Result<Category> {
    let code = str_of(v, "code")
        .ok_or_else(|| Error::Shape("the category answer carried no code".into()))?;
    Ok(Category {
        code,
        name: str_of(v, "name").unwrap_or_default(),
        url: str_of(v, "url"),
        meta_title: str_of(v, "metaTitle"),
        meta_description: str_of(v, "metaDescription"),
        breadcrumbs: v
            .get("breadcrumbs")
            .and_then(|b| b.as_array())
            .map(|bs| bs.iter().filter_map(|b| str_of(b, "name")).collect())
            .unwrap_or_default(),
    })
}

/// Decode the HTML entities the storefront leaves in display names.
///
/// Mitre 10's own data is double-encoded and then truncated: the click and
/// collect mode comes back as `Click &#38 Collect`, semicolon and all missing.
/// Numeric entities are decoded with or without the terminator; the named ones
/// are the handful that show up in product and category names.
pub fn entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at + 1..];
        let end = tail
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '#')
            .unwrap_or(tail.len());
        let (name, after) = tail.split_at(end);
        // The terminator is optional here only because theirs is missing.
        let after = after.strip_prefix(';').unwrap_or(after);
        match decode(name) {
            Some(c) => {
                out.push(c);
                rest = after;
            }
            None => {
                out.push('&');
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

fn decode(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" | "#39" => Some('\''),
        "nbsp" => Some(' '),
        _ => {
            let digits = name.strip_prefix('#')?;
            let code = match digits.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => digits.parse().ok()?,
            };
            char::from_u32(code)
        }
    }
}

pub fn cart(v: &serde_json::Value) -> Cart {
    Cart {
        code: str_of(v, "code"),
        guid: str_of(v, "guid"),
        total_items: f64_of(v, "totalItems").unwrap_or_default() as u64,
        total: money_at(v, "totalPrice"),
        subtotal: money_at(v, "subTotal"),
        discounts: money_at(v, "totalDiscounts"),
        delivery_cost: money_at(v, "deliveryCost"),
        delivery: v.get("deliveryMode").map(|d| Delivery {
            code: str_of(d, "code"),
            name: str_of(d, "name").map(|n| entities(&n)),
            cost: money_at(d, "deliveryCost"),
        }),
        // Hybris returns entries in whatever order it touched them, which puts
        // the line the user just changed first. They are numbered, so show
        // them numbered.
        lines: v
            .get("entries")
            .and_then(|e| e.as_array())
            .map(|es| {
                let mut lines: Vec<_> = es.iter().map(cart_line).collect();
                lines.sort_by_key(|l: &crate::CartLine| l.entry_number);
                lines
            })
            .unwrap_or_default(),
        vouchers: v
            .get("appliedVouchers")
            .and_then(|a| a.as_array())
            .map(|vs| {
                vs.iter()
                    .filter_map(|v| any_str(v, &["code", "voucherCode"]))
                    .collect()
            })
            .unwrap_or_default(),
        pickup_store: v
            .get("pickupLocation")
            .and_then(|p| any_str(p, &["displayName", "name"]))
            .or_else(|| str_of(v, "ctccollectToStoreDisplayName")),
    }
}

fn cart_line(v: &serde_json::Value) -> CartLine {
    let product = v.get("product");
    CartLine {
        entry_number: i64_of(v, "entryNumber").unwrap_or(-1),
        code: product.and_then(|p| str_of(p, "code")).unwrap_or_default(),
        name: product
            .and_then(|p| any_str(p, &["name", "title"]))
            .unwrap_or_default(),
        quantity: i64_of(v, "quantity").unwrap_or_default(),
        unit_price: money_at(v, "basePrice"),
        total: money_at(v, "totalPrice"),
        url: product.and_then(|p| str_of(p, "url")),
        pickup_store: v
            .get("deliveryPointOfService")
            .and_then(|p| any_str(p, &["displayName", "name"])),
        updateable: bool_of(v, "updateable").unwrap_or(true),
    }
}

pub fn customer(v: &serde_json::Value) -> Customer {
    Customer {
        uid: str_of(v, "uid"),
        name: str_of(v, "name"),
        first_name: str_of(v, "firstName"),
        last_name: str_of(v, "lastName"),
        email: any_str(v, &["uid", "displayUid", "email"]).filter(|e| e.contains('@')),
        addresses: v
            .get("addresses")
            .and_then(|a| a.as_array())
            .map(|as_| as_.iter().map(address).collect())
            .unwrap_or_default(),
    }
}

pub fn orders(v: &serde_json::Value) -> OrderPage {
    let pagination = v.get("pagination");
    OrderPage {
        orders: v
            .get("orders")
            .and_then(|o| o.as_array())
            .map(|os| os.iter().map(order).collect())
            .unwrap_or_default(),
        total: pagination
            .and_then(|p| f64_of(p, "totalResults"))
            .unwrap_or_default() as u64,
        page: pagination
            .and_then(|p| f64_of(p, "currentPage"))
            .unwrap_or_default() as u64,
        pages: pagination
            .and_then(|p| f64_of(p, "totalPages"))
            .unwrap_or_default() as u64,
    }
}

pub fn order(v: &serde_json::Value) -> Order {
    Order {
        code: any_str(v, &["code", "guid"]).unwrap_or_default(),
        placed: any_str(v, &["placed", "created"]),
        status: any_str(v, &["statusDisplay", "status"]),
        total: money_at(v, "total").or_else(|| money_at(v, "totalPrice")),
        lines: v
            .get("entries")
            .and_then(|e| e.as_array())
            .map(|es| {
                es.iter()
                    .map(|e| OrderLine {
                        code: e
                            .get("product")
                            .and_then(|p| str_of(p, "code"))
                            .unwrap_or_default(),
                        name: e
                            .get("product")
                            .and_then(|p| any_str(p, &["name", "title"]))
                            .unwrap_or_default(),
                        quantity: i64_of(e, "quantity").unwrap_or_default(),
                        total: money_at(e, "totalPrice"),
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

pub fn wishlist(v: &serde_json::Value) -> Wishlist {
    Wishlist {
        items: v
            .get("entries")
            .or_else(|| v.get("wishlistEntries"))
            .and_then(|e| e.as_array())
            .map(|es| {
                es.iter()
                    .filter_map(|e| {
                        let product = e.get("product").unwrap_or(e);
                        Some(WishlistItem {
                            code: str_of(product, "code")?,
                            name: any_str(product, &["name", "title"]).unwrap_or_default(),
                            price: money_at(product, "price"),
                            url: str_of(product, "url"),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// What a cart change says when the storefront did not do it in full.
///
/// A quantity capped to available stock arrives as a **`200`** carrying
/// `errorMessage`, so nothing about the response marks it as a refusal and
/// the command looks like a silent no-op.
pub fn cart_notice(v: &serde_json::Value) -> Option<String> {
    let message = str_of(v, "errorMessage")?;
    let message = message.trim();
    (!message.is_empty()).then(|| entities(message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_hit_keeps_its_fields_when_one_of_them_is_the_wrong_type() {
        // The whole reason this is a map and not a struct: `size` arrives as a
        // number on some documents and a string on others, and a struct would
        // lose the name, brand and price of the whole hit over it.
        let hit = json!({
            "objectID": "174969",
            "name": "Fence Finish Paint 10L",
            "brandName": "Wattyl",
            "size": 10,
            "colour": null,
            "prices": { "nationalRRP": 79.0, "nationalPromo": 69.0 },
        });
        let p = product(&hit);
        assert_eq!(p.name, "Fence Finish Paint 10L");
        assert_eq!(p.brand.as_deref(), Some("Wattyl"));
        assert_eq!(p.size.as_deref(), Some("10"));
        assert_eq!(p.colour, None);
        assert_eq!(p.price, Some(69.0));
        assert_eq!(p.was_price, Some(79.0));
    }

    #[test]
    fn a_full_price_product_has_no_was_price() {
        // The index sends `nationalPromo` on everything, equal to the ticket
        // price when nothing is on special. Reading it as a promotion would
        // print "was $79.00, save $0.00" on the whole catalogue.
        let hit = json!({
            "objectID": "1",
            "prices": { "nationalRRP": 79.0, "nationalPromo": 79.0 },
        });
        let p = product(&hit);
        assert_eq!(p.price, Some(79.0));
        assert_eq!(p.was_price, None);
        assert_eq!(p.saving(), None);
    }

    #[test]
    fn a_hit_with_no_object_id_is_keyed_from_its_url() {
        let hit = json!({ "url": "/shop/wattyl-fence-finish-paint-10l/p/174969" });
        assert_eq!(product(&hit).code, "174969");
        assert_eq!(
            code_from_url("/shop/a-thing/p/174969"),
            Some("174969".into())
        );
        assert_eq!(code_from_url("/shop/a-category/c/RF7336"), None);
    }

    #[test]
    fn store_codes_arrive_as_numbers_and_are_kept_as_strings() {
        // They are matched against the numeric store code, which is a string
        // everywhere else; comparing 66 to "66" would silently never match.
        let hit = json!({ "objectID": "1", "storesWithStock": [66, 25, 42] });
        let p = product(&hit);
        assert_eq!(p.stores_with_stock, ["66", "25", "42"]);
        assert!(p.in_stock_at("66"));
        assert!(!p.in_stock_at("99"));
    }

    #[test]
    fn a_listing_carries_the_paging_the_index_reported() {
        let body = json!({
            "results": [{
                "hits": [{ "objectID": "1", "name": "A thing" }],
                "nbHits": 173, "page": 1, "nbPages": 8, "hitsPerPage": 24,
                "facets": { "brandName": { "Nouveau": 12, "Acme": 30 } },
            }]
        });
        let l = listing(&body, Some("thing".into())).expect("parses");
        assert_eq!(l.total, 173);
        assert_eq!(l.offset(), 24, "page 1 starts at 24");
        assert!(l.has_more());
        // Commonest first, so the printed facet is useful without scrolling.
        assert_eq!(l.facets[0].options[0].value, "Acme");
        assert_eq!(l.facets[0].options[0].count, 30);
    }

    #[test]
    fn an_answer_with_no_results_block_is_a_shape_error_not_an_empty_page() {
        // An empty listing and a moved schema must not read the same: one is
        // "nothing matched", the other is "this crate needs updating".
        let e = listing(&json!({ "message": "forbidden" }), None).expect_err("refused");
        assert!(matches!(e, Error::Shape(_)), "{e:?}");
    }

    #[test]
    fn a_store_takes_its_numeric_code_from_the_field_called_name() {
        // `name` is "66" and `displayName` is the human one -- the opposite of
        // what the field names suggest, and the code every other call takes.
        let v = json!({
            "name": "66",
            "displayName": "Mitre 10 MEGA Whangārei",
            "storeCode": "X57",
            "staticStoreCode": "66",
            "address": { "phone": "09 430 4009", "postalCode": "0110" },
            "geoPoint": { "latitude": -35.73, "longitude": 174.32 },
        });
        let s = store(&v);
        assert_eq!(s.code, "66");
        assert_eq!(s.sap_code.as_deref(), Some("X57"));
        assert_eq!(s.name, "Mitre 10 MEGA Whangārei");
        assert_eq!(s.phone.as_deref(), Some("09 430 4009"));
    }

    #[test]
    fn stock_answers_every_store_in_one_call() {
        let v = json!({
            "stockIndicator": [
                { "stockIndicator": "LOW_STOCK", "storeCode": "66", "storeDisplayName": "Whangārei" },
                { "stockIndicator": "IN_STOCK", "storeCode": "42", "storeDisplayName": "Napier" },
            ]
        });
        let s = stock(&v);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].level, StockLevel::LowStock);
        assert!(s[1].level.is_available());
    }

    #[test]
    fn a_cart_line_is_addressed_by_the_entry_number_hybris_gave_it() {
        // Update and remove take this number, not the product code; defaulting
        // it to 0 would edit the first line of the cart instead of failing.
        let v = json!({
            "code": "3000744918",
            "guid": "f4eaa13e",
            "totalItems": 1,
            "totalPrice": { "value": 19.59, "currencyIso": "NZD", "formattedValue": "$19.59" },
            "entries": [{
                "entryNumber": 3,
                "quantity": 2,
                "product": { "code": "269938", "name": "Storage Bin & Lid" },
                "totalPrice": { "value": 11.59, "currencyIso": "NZD" },
            }],
        });
        let c = cart(&v);
        assert_eq!(c.id_for(true), Some("3000744918"));
        assert_eq!(c.lines[0].entry_number, 3);
        assert_eq!(c.lines[0].quantity, 2);
        assert_eq!(c.total.expect("a total").display(), "$19.59");

        let orphan = cart(&json!({ "entries": [{ "product": { "code": "1" } }] }));
        assert_eq!(orphan.lines[0].entry_number, -1, "never silently zero");
    }

    #[test]
    fn a_regular_price_is_only_kept_when_it_is_actually_higher() {
        let on_special = json!({
            "code": "174969",
            "price": { "value": 69.0, "currencyIso": "NZD" },
            "regularPrice": { "value": 79.0, "currencyIso": "NZD" },
        });
        let d = product_detail(&on_special).expect("parses");
        assert_eq!(d.regular_price.map(|m| m.value), Some(79.0));

        let full_price = json!({
            "code": "174969",
            "price": { "value": 79.0, "currencyIso": "NZD" },
            "regularPrice": { "value": 79.0, "currencyIso": "NZD" },
        });
        let d = product_detail(&full_price).expect("parses");
        assert_eq!(d.regular_price, None);
    }

    #[test]
    fn an_occ_errors_body_becomes_a_named_refusal() {
        let e = occ_error(
            "addToCart",
            r#"{"errors":[{"message":"Product is not available","type":"CartEntryError"}]}"#,
        )
        .expect("recognised");
        assert!(e.to_string().contains("addToCart"), "{e}");
        assert!(e.to_string().contains("not available"), "{e}");
        assert!(occ_error("addToCart", "not json").is_none());
        assert!(occ_error("addToCart", r#"{"errors":[]}"#).is_none());
    }

    #[test]
    fn loose_accessors_take_a_value_however_it_was_typed() {
        let v = json!({ "n": "4.6", "m": 5, "b": 1, "s": 10, "blank": "  " });
        assert_eq!(f64_of(&v, "n"), Some(4.6));
        assert_eq!(f64_of(&v, "m"), Some(5.0));
        assert_eq!(bool_of(&v, "b"), Some(true));
        assert_eq!(str_of(&v, "s").as_deref(), Some("10"));
        assert_eq!(str_of(&v, "blank"), None, "whitespace reads as absent");
        assert_eq!(any_str(&v, &["missing", "s"]).as_deref(), Some("10"));
    }

    #[test]
    fn the_storefronts_broken_ampersand_is_decoded() {
        // Their own data, double-encoded and then missing its terminator.
        assert_eq!(entities("Click &#38 Collect"), "Click & Collect");
        assert_eq!(entities("Click &#38; Collect"), "Click & Collect");
        assert_eq!(entities("Home &amp; Storage"), "Home & Storage");
        assert_eq!(entities("Rolling &#x26; Latched"), "Rolling & Latched");
    }

    #[test]
    fn text_that_is_not_an_entity_is_left_exactly_as_it_came() {
        // A bare ampersand is ordinary in a product name and must survive.
        assert_eq!(entities("Nuts & Bolts"), "Nuts & Bolts");
        assert_eq!(entities("50% off & more"), "50% off & more");
        assert_eq!(entities("R&D"), "R&D");
        assert_eq!(entities(""), "");
    }

    #[test]
    fn a_capped_quantity_is_reported_rather_than_looking_like_a_no_op() {
        // The storefront answers 200 and quietly caps the line, so without
        // this the command prints an unchanged cart and explains nothing.
        let capped = serde_json::json!({
            "errorMessage": "Unfortunately we can only supply 1 right now.",
            "quantity": 1,
            "quantityAdded": 0,
            "statusCode": "lowStock",
        });
        assert_eq!(
            cart_notice(&capped).as_deref(),
            Some("Unfortunately we can only supply 1 right now.")
        );

        // A clean change says nothing, and an empty message is not a message.
        assert_eq!(cart_notice(&serde_json::json!({ "quantity": 3 })), None);
        assert_eq!(
            cart_notice(&serde_json::json!({ "errorMessage": "  " })),
            None
        );
    }

    #[test]
    fn cart_lines_come_back_in_line_number_order() {
        // Hybris returns the entry it just touched first, which puts line 1
        // above line 0 in a numbered table.
        let c = cart(&serde_json::json!({
            "guid": "f4eaa13e",
            "entries": [
                { "entryNumber": 2, "quantity": 1, "product": { "code": "c" } },
                { "entryNumber": 0, "quantity": 1, "product": { "code": "a" } },
                { "entryNumber": 1, "quantity": 1, "product": { "code": "b" } },
            ]
        }));
        let order: Vec<i64> = c.lines.iter().map(|l| l.entry_number).collect();
        assert_eq!(order, vec![0, 1, 2]);
    }
}
