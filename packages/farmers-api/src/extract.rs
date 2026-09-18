//! Reading the half of this storefront that answers in HTML.
//!
//! The only module that knows what a selector is. Everything the Intershop
//! REST API and Constructor.io do not cover -- per-store stock, the sign-in
//! form, who is signed in -- comes back as a fragment of Bootstrap markup from
//! a `ViewX-` pipeline, and this turns it into types.
//!
//! Markup moves. Every function here answers `None` or an empty vector rather
//! than failing, and the callers turn that into [`crate::Error::NotInPage`]
//! with the name of what was being looked for -- which is the difference
//! between "Farmers changed their template" and a stack trace.

use scraper::{Html, Selector};

use crate::domain::{
    Account, Cart, CartLine, Money, Order, Store, StoreStock, VariationValue, Wishlist,
    WishlistItem,
};

/// Compile a selector, or treat it as matching nothing.
///
/// The selectors here are literals, so a failure means this file has a typo in
/// it and no input can fix that -- but a panic in a library over a cosmetic
/// bug in one extractor would take a whole command down with it.
fn select(selector: &str) -> Option<Selector> {
    Selector::parse(selector).ok()
}

fn text_of(element: scraper::ElementRef<'_>) -> String {
    element.text().collect::<String>().trim().to_string()
}

/// The first non-empty match, or `None`.
fn first_text(scope: scraper::ElementRef<'_>, selector: &str) -> Option<String> {
    let selector = select(selector)?;
    scope.select(&selector).map(text_of).find(|t| !t.is_empty())
}

/// Per-store availability, from `ViewCheckoutShipping-GetAvailableStoresAjax`.
///
/// The fragment is a Bootstrap accordion: a `panel-heading` carrying the name
/// and the verdict, then a sibling `panel-collapse` carrying the address and
/// the hours. The two are **not nested**, so they are selected separately and
/// paired by position -- which holds because the template emits one of each per
/// store, in order.
pub fn store_stock(html: &str) -> Vec<StoreStock> {
    let doc = Html::parse_fragment(html);
    let (Some(headings), Some(panels)) =
        (select("div.panel-heading"), select("div.panel-collapse"))
    else {
        return Vec::new();
    };

    let details: Vec<_> = doc.select(&panels).collect();
    doc.select(&headings)
        .enumerate()
        .filter_map(|(i, heading)| {
            let name = first_text(heading, ".store__name span")?;
            let status = first_text(heading, ".store-stock").unwrap_or_default();
            let panel = details.get(i);
            Some(StoreStock {
                store: Store {
                    name,
                    address: panel
                        .and_then(|p| first_text(*p, ".store-details"))
                        .map(tidy),
                    // The number is the link text as well as the `tel:` href,
                    // and the text is the formatted one.
                    phone: panel.and_then(|p| first_text(*p, ".store-phone a")),
                    hours: panel.map(|p| hours(*p)).unwrap_or_default(),
                },
                status,
            })
        })
        .collect()
}

/// Opening times, as day and hours pairs in the order the page lists them.
fn hours(panel: scraper::ElementRef<'_>) -> Vec<(String, String)> {
    let (Some(rows), Some(day), Some(time)) =
        (select(".store-hour-row"), select(".sday"), select(".stime"))
    else {
        return Vec::new();
    };
    panel
        .select(&rows)
        .filter_map(|row| {
            let day = row.select(&day).next().map(text_of)?;
            let time = row.select(&time).next().map(text_of)?;
            (!day.is_empty() && !time.is_empty()).then_some((day, time))
        })
        .collect()
}

/// Collapse the whitespace a two-line address block arrives with, and drop the
/// non-breaking spaces the template pads it out with.
fn tidy(text: String) -> String {
    text.replace('\u{a0}', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The basket, from the `ViewMiniCart-Status` fragment.
///
/// That fragment rather than the cart page on purpose. It is about 3KB against
/// the page's 160KB, it needs no form token, and it carries every fact a
/// listing wants -- including the line item ids, which are what a remove or a
/// quantity change is addressed by. The cart page spreads the same data across
/// table columns and is far more fragile to read.
pub fn cart(html: &str) -> Cart {
    let doc = Html::parse_fragment(html);
    let root = doc.root_element();

    // The totals hang off the container as attributes, which is the sturdiest
    // thing in the fragment: they survive any amount of re-styling.
    let container = select("#miniCart").and_then(|s| doc.select(&s).next());
    let attr = |name: &str| container.and_then(|c| c.attr(name)).map(str::to_string);

    Cart {
        id: attr("data-cart-id").filter(|v| !v.is_empty()),
        currency: attr("data-currency").filter(|v| !v.is_empty()),
        // "3 items" -- the number is the only part worth keeping.
        count: first_text(root, "#mini-cart-count").and_then(|t| {
            t.split_whitespace()
                .next()
                .and_then(|n| n.parse::<i64>().ok())
        }),
        subtotal: attr("data-cart-subtotal").as_deref().and_then(Money::parse),
        total: attr("cart-total").as_deref().and_then(Money::parse),
        grand_total: attr("data-cart-grand-total")
            .as_deref()
            .and_then(Money::parse),
        lines: cart_lines(&doc),
    }
}

fn cart_lines(doc: &Html) -> Vec<CartLine> {
    let Some(rows) = select("div.product-row") else {
        return Vec::new();
    };
    doc.select(&rows)
        .filter_map(|row| {
            // The id lives only inside the remove link's `onclick`, as a query
            // parameter of a URL passed to a JavaScript function. Ugly, and
            // the only place it appears.
            let id = remove_pli(row)?;
            let link = select("a.product-image, .mini-product-title a")
                .and_then(|s| row.select(&s).find(|a| a.attr("href").is_some()));
            let url = link.and_then(|a| a.attr("href")).map(str::to_string);
            Some(CartLine {
                sku: sku_attr(row).or_else(|| url.as_deref().and_then(sku_from_url)),
                name: first_text(row, ".mini-product-title a")
                    .or_else(|| link.and_then(|a| a.attr("title")).map(str::to_string)),
                quantity: first_text(row, ".product-quantity").and_then(|t| t.parse().ok()),
                unit_price: price_attr(row, "data-sales-price"),
                list_price: price_attr(row, "data-list-price"),
                total: first_text(row, ".total-price")
                    .as_deref()
                    .and_then(Money::parse),
                options: cart_options(row),
                url,
                id,
            })
        })
        .collect()
}

/// The line item id, out of `ViewMiniCart-RemoveItemFromMinicart?RemovePLI=…`.
fn remove_pli(row: scraper::ElementRef<'_>) -> Option<String> {
    let selector = select("a.ico-remove-item")?;
    let onclick = row.select(&selector).find_map(|a| a.attr("onclick"))?;
    let after = onclick.split("RemovePLI=").nth(1)?;
    let id: String = after
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    (!id.is_empty()).then_some(id)
}

/// The SKU, which the remove link also carries as a plain attribute.
fn sku_attr(row: scraper::ElementRef<'_>) -> Option<String> {
    let selector = select("[data-remove-pli-minicart]")?;
    row.select(&selector)
        .find_map(|e| e.attr("data-remove-pli-minicart"))
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// A product URL ends in its code, after the last hyphen of the slug.
fn sku_from_url(url: &str) -> Option<String> {
    let last = url.trim_end_matches('/').rsplit('/').next()?;
    let code = last.rsplit('-').next()?;
    (!code.is_empty() && code.chars().all(|c| c.is_ascii_digit())).then(|| code.to_string())
}

fn price_attr(row: scraper::ElementRef<'_>, name: &str) -> Option<Money> {
    let selector = select("div.product-price")?;
    row.select(&selector)
        .find_map(|e| e.attr(name))
        .and_then(Money::parse)
}

/// `<span>Colour:</span>Grey` -- the label is an element and the value is the
/// bare text beside it, so this reads the block and splits on the colon rather
/// than selecting the value, which has no element of its own.
fn cart_options(row: scraper::ElementRef<'_>) -> Vec<VariationValue> {
    let Some(blocks) = select("div.cart-pli-data") else {
        return Vec::new();
    };
    row.select(&blocks)
        .filter_map(|block| {
            let text = tidy(text_of(block));
            let (axis, value) = text.split_once(':')?;
            let (axis, value) = (axis.trim(), value.trim());
            // Quantity lives in the same kind of block and is not a variation.
            if value.is_empty() || axis.eq_ignore_ascii_case("quantity") {
                return None;
            }
            Some(VariationValue {
                axis: axis.to_string(),
                value: value.to_string(),
            })
        })
        .collect()
}

/// The saved lists, from `/wishlists`.
///
/// **Read against an account with no lists.** The empty case is the one that
/// was captured, and it is the one this is sure of; the populated selectors
/// are read off the same template's class names and should be treated as
/// unverified until someone with a list runs it.
pub fn wishlists(html: &str) -> Vec<Wishlist> {
    let doc = Html::parse_document(html);
    let Some(rows) = select(".wishlist, .wishlist-item, li.wishlist-entry") else {
        return Vec::new();
    };
    doc.select(&rows)
        .filter_map(|row| {
            let name = first_text(row, ".wishlist-title, .wishlist-name, h2, h3")?;
            Some(Wishlist {
                id: row
                    .attr("data-wishlist-id")
                    .or_else(|| row.attr("id"))
                    .map(str::to_string),
                preferred: row.html().contains("preferred"),
                public: row.html().contains("public"),
                items: wishlist_items(row),
                name,
            })
        })
        .collect()
}

fn wishlist_items(scope: scraper::ElementRef<'_>) -> Vec<WishlistItem> {
    let Some(rows) = select(".product-row, .wishlist-product, .product-tile") else {
        return Vec::new();
    };
    scope
        .select(&rows)
        .map(|row| {
            let url = select("a")
                .and_then(|s| row.select(&s).find_map(|a| a.attr("href")))
                .map(str::to_string);
            WishlistItem {
                sku: url.as_deref().and_then(sku_from_url),
                name: first_text(row, ".product-title, .product-name, a"),
                price: first_text(row, ".product-price, .price")
                    .as_deref()
                    .and_then(Money::parse),
                url,
            }
        })
        .collect()
}

/// Whether a page is the "you have none of these" state rather than a list
/// this code failed to read.
///
/// The distinction matters: an empty vector could mean either, and telling
/// someone their order history is empty when the markup simply moved is the
/// wrong answer to give confidently.
pub fn says_empty(html: &str, noun: &str) -> bool {
    let doc = Html::parse_document(html);
    let Some(selector) = select("p, div.section") else {
        return false;
    };
    let noun = noun.to_lowercase();
    doc.select(&selector).map(text_of).any(|text| {
        // Collapsed first: the template wraps this sentence across lines, so
        // "have any" is not contiguous in the raw text.
        let text = tidy(text).to_lowercase();
        // "You do not have any Orders" / "Currently you don't have any wish
        // lists." -- one template, two wordings.
        (text.contains("do not have any") || text.contains("have any"))
            && text.contains(&noun)
            && text.len() < 200
    })
}

/// Past orders, from `/orders`.
///
/// **Read against an account with no orders**, like [`wishlists`]. The empty
/// case is verified; the row selectors are inferred from the template's class
/// names and are not.
pub fn orders(html: &str) -> Vec<Order> {
    let doc = Html::parse_document(html);
    let Some(rows) =
        select("tr.order, .order-history-item, .order-row, table.order-history tbody tr")
    else {
        return Vec::new();
    };
    doc.select(&rows)
        .filter_map(|row| {
            let number = first_text(row, ".order-number, .order-id, td:first-child")
                .map(|t| t.trim_start_matches('#').trim().to_string())
                .filter(|t| !t.is_empty())?;
            Some(Order {
                placed: first_text(row, ".order-date, .date"),
                status: first_text(row, ".order-status, .status"),
                total: first_text(row, ".order-total, .total")
                    .as_deref()
                    .and_then(Money::parse),
                url: select("a")
                    .and_then(|s| row.select(&s).find_map(|a| a.attr("href")))
                    .map(str::to_string),
                number,
            })
        })
        .collect()
}

/// The form token the sign-in page mints.
///
/// Intershop calls it a `SynchronizerToken`; it is an ordinary CSRF nonce and
/// the POST is refused without it. A page that carries none is not the sign-in
/// page -- which is what a bot check served in its place looks like, and why
/// [`crate::auth`] refuses to post credentials into it.
pub fn synchronizer_token(html: &str) -> Option<String> {
    let doc = Html::parse_document(html);
    let selector = select(r#"input[name="SynchronizerToken"]"#)?;
    doc.select(&selector)
        .find_map(|i| i.attr("value"))
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// What the sign-in page said was wrong, from the alert it renders back into
/// itself.
pub fn form_error(html: &str) -> Option<String> {
    let doc = Html::parse_document(html);
    let selector = select(".error-message, .alert-danger, .form-error, .login-error")?;
    doc.select(&selector)
        .map(text_of)
        .find(|t| !t.is_empty())
        .map(tidy)
}

/// The marker the account header renders for someone signed in.
const SIGNED_IN: &str = "my-account-logged-in";

/// Who the storefront thinks is asking, from `ViewUserAccount-AjaxHeader`.
///
/// Two independent things are read. The icon's class is the verdict -- the
/// template renders `my-account-logged-in` or `my-account-logged-out`, and
/// nothing else in the fragment distinguishes them. The name and email come
/// from an analytics call the same fragment inlines, which is the only place
/// the storefront states them without a second request.
pub fn account(html: &str) -> Account {
    let signed_in = html.contains(SIGNED_IN);
    let (email, first_name, last_name) = identity(html).unwrap_or_default();
    Account {
        signed_in,
        // Only when the header agrees: the script is the previous shopper's
        // if the fragment was cached, and claiming a name beside "signed out"
        // reads as a bug.
        email: signed_in.then_some(email).flatten(),
        first_name: signed_in.then_some(first_name).flatten(),
        last_name: signed_in.then_some(last_name).flatten(),
    }
}

/// `DDTrackingEvents.identityUserEvent('shopper@example.invalid','Ada','Lovelace')`
///
/// Parsed out of the inline script by hand. It is three single-quoted
/// arguments on one call, and running a JavaScript engine to read them would
/// be a preposterous amount of machinery for a comma.
type Identity = (Option<String>, Option<String>, Option<String>);

fn identity(html: &str) -> Option<Identity> {
    let start = html.find("identityUserEvent(")? + "identityUserEvent(".len();
    let rest = &html[start..];
    let end = rest.find(')')?;
    let mut args = rest[..end]
        .split(',')
        .map(|arg| arg.trim().trim_matches('\'').trim_matches('"').trim())
        .map(|arg| (!arg.is_empty()).then(|| arg.to_string()));
    Some((args.next()?, args.next().flatten(), args.next().flatten()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two stores, trimmed from a live `GetAvailableStoresAjax` answer.
    const STORES: &str = r##"
<div class="panel-heading">
<div class="store__name"><span>Newmarket</span>
<a href="#store1"><div class="view-store-details">Store Details </div></a></div>
<div class="store-stock">In Stock</div>
</div>
<div id="store1" class="panel-collapse store-infobox collapse"><div class="row">
<div class="store-address col-sm-6">
<div class="store-city"><span class="store-pin"></span> Newmarket</div>
<div class="store-details">309 Broadway<br/>
Newmarket&nbsp;</div>
<div class="store-phone">Ph: <a href="tel:09 441 3654">09 441 3654</a></div>
</div>
<div class="store-hours col-sm-6">
<div class="store-hour-row"><div class="sday">Monday</div><div class="stime">9:00 a.m. - 7:00 p.m.</div></div>
<div class="store-hour-row"><div class="sday">Sunday </div><div class="stime">10:00 a.m. - 7:00 p.m.</div></div>
</div>
</div></div>
<div class="panel-heading">
<div class="store__name"><span>Albany</span></div>
<div class="store-stock">Not In Stock</div>
</div>
<div id="store2" class="panel-collapse store-infobox collapse"><div class="row">
<div class="store-address col-sm-6"><div class="store-details">Don McKinnon Drive</div></div>
</div></div>"##;

    #[test]
    fn every_store_in_the_fragment_is_read_with_its_verdict() {
        let stock = store_stock(STORES);
        assert_eq!(stock.len(), 2);
        assert_eq!(stock[0].store.name, "Newmarket");
        assert!(stock[0].available());
        assert_eq!(stock[1].store.name, "Albany");
        assert!(!stock[1].available());
    }

    #[test]
    fn a_two_line_address_comes_back_as_one_line_without_the_padding() {
        // The template separates the lines with a <br/> and pads the end with
        // a non-breaking space, which would otherwise show up in a table cell.
        let stock = store_stock(STORES);
        assert_eq!(
            stock[0].store.address.as_deref(),
            Some("309 Broadway Newmarket")
        );
        assert_eq!(stock[0].store.phone.as_deref(), Some("09 441 3654"));
    }

    #[test]
    fn the_heading_and_the_detail_panel_are_paired_by_position() {
        // They are siblings rather than nested, so an off-by-one here would
        // quietly give every store the previous one's address.
        let stock = store_stock(STORES);
        assert_eq!(
            stock[1].store.address.as_deref(),
            Some("Don McKinnon Drive")
        );
        assert_eq!(stock[0].store.hours.len(), 2);
        assert_eq!(stock[0].store.hours[1].0, "Sunday");
    }

    #[test]
    fn a_store_with_no_detail_panel_is_still_reported() {
        // The verdict is the point of the call; an address is decoration.
        let stock = store_stock(
            r#"<div class="panel-heading"><div class="store__name"><span>Riccarton</span></div>
               <div class="store-stock">In Stock</div></div>"#,
        );
        assert_eq!(stock.len(), 1);
        assert_eq!(stock[0].store.address, None);
        assert!(stock[0].store.hours.is_empty());
    }

    #[test]
    fn markup_that_is_not_the_expected_fragment_yields_nothing_rather_than_failing() {
        assert!(store_stock("<html><body>Access Denied</body></html>").is_empty());
        assert!(store_stock("").is_empty());
    }

    /// A two-line mini-cart, trimmed from a live `ViewMiniCart-Status` answer.
    /// The ids are shortened and made obviously fake; the structure is not
    /// touched.
    const MINICART: &str = r##"
<div id="mini-cart-container"><a id="cart-tag" href="#miniCart">
<span id="mini-cart-count">3 items</span><span id="mini-cart-separator"> / </span>
<span id="mini-cart-total" class="mini-cart-price">$309.97</span></a>
<div class="mini-cart collapse" id="miniCart"
data-currency="NZD"
data-cart-id="notarealcartid00000001"
cart-total="$269.53"
data-cart-grand-total="$309.97"
data-cart-subtotal="$309.97">
<div class="product-rows-block"><div class="slider">
<div class="product-row quick-cart-row">
<div class="category-path hidden">Kitchen &amp; Dining &gt;&gt; Cookware</div>
<div class="mini-product-img"><a href="https://www.farmers.co.nz/kitchen-dining/a-stockpot-6718307">
<img class="product-image" alt="A stockpot product photo"/></a></div>
<div class="mini-product-info"><div class="mini-product-title"><div class="row">
<div class="col-xs-9"><a href="https://www.farmers.co.nz/kitchen-dining/a-stockpot-6718307"
title="Baccarat iD3 Stainless Steel Stockpot with Lid, 24cm">Baccarat iD3 Stainless Steel Stockpot with Lid, 24cm</a></div>
<div class="col-xs-3"><a class="ico-remove-item"
onclick="QuickCart.removeItem.call(['https://www.farmers.co.nz/INTERSHOP/web/WFS/Farmers-Shop-Site/en_NZ/-/NZD/ViewMiniCart-RemoveItemFromMinicart?RemovePLI=notarealpli0000000001','https://www.farmers.co.nz/INTERSHOP/web/WFS/Farmers-Shop-Site/en_NZ/-/NZD/ViewMiniCart-Status','https://www.farmers.co.nz/cart'])"
data-remove-pli-minicart="6718307"></a></div></div></div>
<div class="cart-pli-data"><span>Quantity:</span><span class="product-quantity">2</span></div>
<div class="product-price" data-sales-price="$219.99" data-list-price="$239.99">
<div><div class="total-price">$439.98</div></div></div>
</div></div>
<div class="product-row quick-cart-row">
<div class="mini-product-img"><a href="https://www.farmers.co.nz/men/robes/chisel-fleece-robe-charcoal-6867065002"></a></div>
<div class="mini-product-info"><div class="mini-product-title"><div class="row">
<div class="col-xs-9"><a href="https://www.farmers.co.nz/men/robes/chisel-fleece-robe-charcoal-6867065002">Chisel Fleece Robe, Charcoal</a></div>
<div class="col-xs-3"><a class="ico-remove-item"
onclick="QuickCart.removeItem.call(['https://www.farmers.co.nz/INTERSHOP/web/WFS/Farmers-Shop-Site/en_NZ/-/NZD/ViewMiniCart-RemoveItemFromMinicart?RemovePLI=notarealpli0000000002','x','y'])"
data-remove-pli-minicart="6867065002"></a></div></div></div>
<div class="cart-pli-data"><span>Quantity:</span><span class="product-quantity">1</span></div>
<div class="cart-pli-data"><span>Colour:</span>Grey<br></div>
<div class="cart-pli-data"><span>Size:</span>L-XL</div>
<div class="product-price" data-sales-price="$89.99" data-list-price="$89.99">
<div><div class="total-price">$89.99</div></div></div>
</div></div>
</div></div></div></div>"##;

    #[test]
    fn the_basket_is_read_out_of_the_mini_cart_fragment() {
        let cart = cart(MINICART);
        assert_eq!(cart.id.as_deref(), Some("notarealcartid00000001"));
        assert_eq!(cart.currency.as_deref(), Some("NZD"));
        assert_eq!(cart.count, Some(3));
        assert_eq!(cart.subtotal.unwrap().value, 309.97);
        assert_eq!(cart.grand_total.unwrap().value, 309.97);
        // `cart-total` is the discounted figure, and it is a different number.
        assert_eq!(cart.total.unwrap().value, 269.53);
        assert_eq!(cart.lines.len(), 2);
    }

    #[test]
    fn a_line_carries_the_id_that_changing_it_is_addressed_by() {
        // It exists only inside a JavaScript call in an onclick attribute, as
        // a query parameter of a URL. Without it nothing can be removed or
        // re-quantified, so this is the load-bearing extraction in the file.
        let cart = cart(MINICART);
        assert_eq!(cart.lines[0].id, "notarealpli0000000001");
        assert_eq!(cart.lines[1].id, "notarealpli0000000002");
        assert_eq!(
            cart.nth(1).map(|l| l.id.as_str()),
            Some("notarealpli0000000001")
        );
    }

    #[test]
    fn a_line_is_not_addressed_by_its_sku_because_two_lines_can_share_one() {
        // The SKU is carried as well, and it is a different thing.
        let cart = cart(MINICART);
        assert_eq!(cart.lines[0].sku.as_deref(), Some("6718307"));
        assert_ne!(
            cart.lines[0].sku.as_deref(),
            Some(cart.lines[0].id.as_str())
        );
    }

    #[test]
    fn a_line_keeps_its_quantity_prices_and_what_makes_it_different() {
        let cart = cart(MINICART);
        let pot = &cart.lines[0];
        assert_eq!(pot.quantity, Some(2));
        assert_eq!(pot.unit_price.as_ref().unwrap().value, 219.99);
        assert_eq!(pot.list_price.as_ref().unwrap().value, 239.99);
        assert_eq!(pot.total.as_ref().unwrap().value, 439.98);
        assert!(pot.options.is_empty(), "a stockpot has no size");

        let robe = &cart.lines[1];
        assert_eq!(robe.label(), "Colour Grey, Size L-XL");
        assert_eq!(cart.units(), 3);
    }

    #[test]
    fn quantity_is_not_mistaken_for_a_variation() {
        // It sits in an identical `cart-pli-data` block, so a naive split on
        // the colon would report every line as having a "Quantity" option.
        let cart = cart(MINICART);
        assert!(
            !cart.lines[1].options.iter().any(|o| o.axis == "Quantity"),
            "{:?}",
            cart.lines[1].options
        );
        assert_eq!(cart.lines[1].options.len(), 2);
    }

    #[test]
    fn an_empty_basket_is_a_cart_with_no_lines_rather_than_nothing() {
        let empty = r#"<div id="mini-cart-container"><span id="mini-cart-count">0 items</span>
            <div class="mini-cart" id="miniCart" data-currency="NZD" data-cart-id=""
            data-cart-subtotal="">Your cart is currently empty.</div></div>"#;
        let cart = cart(empty);
        assert!(cart.is_empty());
        assert_eq!(cart.count, Some(0));
        assert_eq!(cart.units(), 0);
        // Blank attributes are absent rather than empty strings.
        assert_eq!(cart.id, None);
        assert_eq!(cart.subtotal, None);
    }

    #[test]
    fn an_account_with_nothing_in_it_is_told_apart_from_markup_that_moved() {
        // An empty vector means either, and confidently telling someone their
        // order history is empty when the template changed is the wrong
        // answer.
        let none = r#"<div class="col-md-9"><h1>Order History</h1>
                      <p>You do not have any Orders</p></div>"#;
        assert!(says_empty(none, "orders"));
        assert!(!says_empty(none, "wish lists"));

        let lists = r#"<div class="section"><p class="flush">Currently you don't have
                       any wish lists.</p></div>"#;
        assert!(says_empty(lists, "wish lists"));

        // A page this code simply could not read says no such thing.
        assert!(!says_empty(
            "<table><tr><td>12345</td></tr></table>",
            "orders"
        ));
    }

    #[test]
    fn the_form_token_is_read_off_the_sign_in_page() {
        let html = r#"<form name="LoginUserForm" method="post">
            <input type="hidden" name="SynchronizerToken" value="0123456789abcdef01234567"/>
            <input name="ShopLoginForm_Login" value=""/></form>"#;
        assert_eq!(
            synchronizer_token(html).as_deref(),
            Some("0123456789abcdef01234567")
        );
    }

    #[test]
    fn a_page_with_no_token_is_reported_rather_than_posted_to_blind() {
        // A bot check served in place of the sign-in page looks exactly like
        // this, and posting a password into it would be worse than failing.
        assert_eq!(synchronizer_token("<html>Access Denied</html>"), None);
        assert_eq!(
            synchronizer_token(r#"<input name="SynchronizerToken" value=""/>"#),
            None
        );
    }

    #[test]
    fn the_account_header_says_who_is_signed_in() {
        let html = r#"<li class="users ajax-login-status">
            <span id="header-login-icon" class="glyphicon-user my-account-logged-in"></span>
            <script>$(document).ready(function(){
            DDTrackingEvents.identityUserEvent('shopper@example.invalid','Ada','Lovelace' )
            })</script></li>"#;
        let account = account(html);
        assert!(account.signed_in);
        assert_eq!(account.email.as_deref(), Some("shopper@example.invalid"));
        assert_eq!(account.name().as_deref(), Some("Ada Lovelace"));
    }

    #[test]
    fn a_signed_out_header_is_not_given_a_name_by_a_stale_script() {
        // `my-account-logged-out` contains neither marker by accident, and a
        // name printed next to "signed out" reads as a bug in this tool.
        let html = r#"<span class="my-account-logged-out"></span>
            <script>DDTrackingEvents.identityUserEvent('someone@example.invalid','A','B')</script>"#;
        let account = account(html);
        assert!(!account.signed_in);
        assert_eq!(account.email, None);
        assert_eq!(account.name(), None);
    }

    #[test]
    fn an_anonymous_header_has_no_identity_call_at_all() {
        let html = r#"<span class="my-account-logged-out"></span>
            <script>DDTrackingEvents.anonymousUserEvent();</script>"#;
        assert_eq!(account(html), Account::default());
    }

    #[test]
    fn the_sites_own_words_for_a_refused_sign_in_are_kept() {
        let html =
            r#"<div class="error-message">The email address or password is incorrect.</div>"#;
        assert_eq!(
            form_error(html).as_deref(),
            Some("The email address or password is incorrect.")
        );
        assert_eq!(form_error("<div>nothing wrong here</div>"), None);
    }
}
