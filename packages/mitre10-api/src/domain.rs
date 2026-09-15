//! What this crate hands back, in its own vocabulary rather than Hybris's.
//!
//! One thing here is worth knowing before reading anything else: **a store has
//! two codes.** `staticStoreCode` is numeric (`66`) and is what every API in
//! this crate takes -- the Algolia filters, `stockIndicator`, the cart's
//! `storeId`. `storeCode` is the SAP one (`X57`) and appears only as a product's
//! `preferredStoreId`. [`Store::code`] is the numeric one for that reason.

use serde::{Deserialize, Serialize};

/// A price, with the storefront's own formatting kept alongside the number.
///
/// `formatted` is carried rather than rebuilt because the site's currency
/// rendering is the thing a person recognises, and reproducing it from `value`
/// would go wrong on the rounded ones.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Money {
    pub value: f64,
    pub currency: String,
    pub formatted: Option<String>,
}

impl Money {
    pub fn new(value: f64, currency: impl Into<String>) -> Money {
        Money {
            value,
            currency: currency.into(),
            formatted: None,
        }
    }

    pub fn display(&self) -> String {
        self.formatted
            .clone()
            .unwrap_or_else(|| format!("${:.2}", self.value))
    }
}

/// A product as a grid row: what Algolia carries, which is enough to list.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Product {
    pub code: String,
    pub name: String,
    pub brand: Option<String>,
    /// The selling price now.
    pub price: Option<f64>,
    /// The ticket price, when it is higher than [`Product::price`].
    pub was_price: Option<f64>,
    pub size: Option<String>,
    pub colour: Option<String>,
    pub unit: Option<String>,
    pub url: Option<String>,
    pub image: Option<String>,
    /// Whether the product is sold online at all, nationwide.
    pub available_nationwide: bool,
    /// Numeric store codes that have it on the shelf. Straight from the index,
    /// so it is as fresh as the last reindex rather than live.
    pub stores_with_stock: Vec<String>,
    /// Numeric store codes that will do click and collect for it.
    pub click_and_collect: Vec<String>,
    /// The promotion badge the site would show, e.g. `WAS_NOW`.
    pub promotions: Vec<Promotion>,
}

impl Product {
    /// Whether the index thinks this store has it. Cheaper than
    /// [`crate::Client::stock`] and less authoritative -- it is the index's
    /// view, not the shop's.
    pub fn in_stock_at(&self, store: &str) -> bool {
        self.stores_with_stock.iter().any(|s| s == store)
    }

    /// The saving, when the ticket price is genuinely higher. The index sends
    /// the same number for both on everything that is not on special.
    pub fn saving(&self) -> Option<f64> {
        match (self.price, self.was_price) {
            (Some(now), Some(was)) if was > now => Some(was - now),
            _ => None,
        }
    }
}

/// A promotion badge, as the index carries it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Promotion {
    /// `WAS_NOW`, `MULTIBUY` and friends.
    pub badge: Option<String>,
    /// The catalogue offer this belongs to, e.g. `CT0327`.
    pub offer_id: Option<String>,
    pub starts: Option<String>,
    pub ends: Option<String>,
    pub text: Option<String>,
}

/// One page of a search or a browse.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Listing {
    pub products: Vec<Product>,
    pub total: u64,
    pub page: u64,
    pub pages: u64,
    pub page_size: u64,
    pub facets: Vec<Facet>,
    /// What was searched for, when anything was.
    pub query: Option<String>,
}

impl Listing {
    /// The zero-based index of the first product on this page, for a caller
    /// printing `25-48 of 173`.
    pub fn offset(&self) -> u64 {
        self.page * self.page_size
    }

    pub fn has_more(&self) -> bool {
        self.page + 1 < self.pages
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Facet {
    pub name: String,
    pub options: Vec<FacetOption>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FacetOption {
    pub value: String,
    pub count: u64,
}

/// A product page: everything the grid has plus what only OCC knows.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProductDetail {
    pub code: String,
    /// The site's own heading, which is longer than `name` and is what a
    /// person searched for.
    pub title: Option<String>,
    pub name: String,
    pub brand: Option<String>,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub price: Option<Money>,
    /// The ticket price when the product is on promotion.
    pub regular_price: Option<Money>,
    pub price_badge: Option<String>,
    pub rating: Option<f64>,
    pub reviews: Option<u64>,
    pub model_number: Option<String>,
    pub unit: Option<String>,
    pub net_content: Option<f64>,
    pub net_content_uom: Option<String>,
    /// The shop the prices and stock above are for.
    pub store: Option<String>,
    pub store_name: Option<String>,
    /// How many the preferred store has, when the storefront says.
    pub store_stock: Option<i64>,
    pub stock_indicator: Option<StockLevel>,
    pub click_and_collect: bool,
    pub home_delivery: bool,
    pub purchasable: bool,
    pub max_order_quantity: Option<i64>,
    pub categories: Vec<String>,
    pub breadcrumbs: Vec<String>,
    pub specifications: Vec<(String, String)>,
    pub images: Vec<String>,
    pub url: Option<String>,
    pub variants: Vec<Variant>,
}

/// One buyable variation of a configurable product.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Variant {
    pub code: String,
    pub name: Option<String>,
    pub price: Option<Money>,
    pub url: Option<String>,
}

/// How available a product is at one shop.
///
/// The storefront's own vocabulary, not a stock count: it will say `LOW_STOCK`
/// without saying how low. `/products/{code}` does carry a number for the
/// preferred store, which is why [`ProductDetail::store_stock`] exists and this
/// does not.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StockLevel {
    InStock,
    LowStock,
    OutOfStock,
    /// A value the storefront started sending that this crate does not know.
    /// Carried through rather than dropped so a new state costs a label, not
    /// the command.
    Unknown,
}

impl StockLevel {
    pub fn parse(raw: &str) -> StockLevel {
        match raw.to_ascii_uppercase().replace(['-', ' '], "_").as_str() {
            "IN_STOCK" | "INSTOCK" => StockLevel::InStock,
            "LOW_STOCK" | "LOWSTOCK" => StockLevel::LowStock,
            "OUT_OF_STOCK" | "OUTOFSTOCK" | "NO_STOCK" => StockLevel::OutOfStock,
            _ => StockLevel::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            StockLevel::InStock => "in stock",
            StockLevel::LowStock => "low stock",
            StockLevel::OutOfStock => "out of stock",
            StockLevel::Unknown => "unknown",
        }
    }

    pub fn is_available(self) -> bool {
        matches!(self, StockLevel::InStock | StockLevel::LowStock)
    }
}

/// One shop's answer for one product.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stock {
    /// The numeric store code.
    pub store: String,
    pub store_name: String,
    pub level: StockLevel,
}

/// A shop.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Store {
    /// The numeric code (`66`), which is what every other call takes.
    pub code: String,
    /// The SAP code (`X57`). Carried for completeness; nothing here takes it.
    pub sap_code: Option<String>,
    pub name: String,
    pub address: Option<Address>,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    /// The delivery-pricing group this shop's postcode falls in, which is what
    /// the Algolia `homeDelivery` filter matches on.
    pub postcode_group: Option<String>,
    pub today_hours: Option<String>,
    pub express_delivery: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Address {
    pub line1: Option<String>,
    pub line2: Option<String>,
    pub suburb: Option<String>,
    pub town: Option<String>,
    pub postcode: Option<String>,
    pub formatted: Option<String>,
}

/// How an order is to be got: collected from a shop, or delivered.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Fulfilment {
    ClickToCollect,
    HomeDelivery,
    ExpressDelivery,
}

impl Fulfilment {
    /// The spelling `changeDeliveryMode` takes.
    pub fn code(self) -> &'static str {
        match self {
            Fulfilment::ClickToCollect => "clickToCollect",
            Fulfilment::HomeDelivery => "homeDelivery",
            Fulfilment::ExpressDelivery => "expressDelivery",
        }
    }

    pub fn parse(s: &str) -> Option<Fulfilment> {
        let key: String = s
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_lowercase())
            .collect();
        match key.as_str() {
            "collect" | "clicktocollect" | "clickandcollect" | "ctc" | "pickup" => {
                Some(Fulfilment::ClickToCollect)
            }
            "delivery" | "homedelivery" | "hd" | "deliver" => Some(Fulfilment::HomeDelivery),
            "express" | "expressdelivery" => Some(Fulfilment::ExpressDelivery),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Fulfilment::ClickToCollect => "click and collect",
            Fulfilment::HomeDelivery => "home delivery",
            Fulfilment::ExpressDelivery => "express delivery",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Cart {
    /// The order number a signed-in cart gets. Anonymous carts have none.
    pub code: Option<String>,
    /// The anonymous cart's identifier, which is what its URLs take.
    pub guid: Option<String>,
    pub total_items: u64,
    pub total: Option<Money>,
    pub subtotal: Option<Money>,
    pub discounts: Option<Money>,
    pub delivery_cost: Option<Money>,
    pub delivery: Option<Delivery>,
    pub lines: Vec<CartLine>,
    pub vouchers: Vec<String>,
    /// The shop a click-and-collect cart is collected from.
    pub pickup_store: Option<String>,
}

impl Cart {
    /// What the URLs for this cart take: the order code where there is one,
    /// The identifier the cart paths take, which depends on who is asking.
    ///
    /// An anonymous cart is addressed by its **guid** and a signed-in one by
    /// its **code**. Both are present on both, so nothing about a cart says
    /// which to use -- and getting it wrong answers `Cart not found`, which
    /// reads as an expired basket rather than the wrong handle.
    /// [`crate::Client::cart_id`] knows the session and should be preferred.
    pub fn id_for(&self, signed_in: bool) -> Option<&str> {
        let (first, then) = if signed_in {
            (&self.code, &self.guid)
        } else {
            (&self.guid, &self.code)
        };
        first.as_deref().or(then.as_deref())
    }
}

/// A cart after a change, and what the storefront said about it.
///
/// The two are separate because a cap is not an error: the cart came back and
/// is worth showing, but so is the reason it is not what was asked for.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CartChange {
    pub cart: Cart,
    /// Set when the storefront did something other than what was asked --
    /// "we can only supply 1 right now", most often. Arrives on a `200`.
    pub notice: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CartLine {
    /// Hybris's own line number, which is what update and remove take.
    pub entry_number: i64,
    pub code: String,
    pub name: String,
    /// Whole units. Hybris refuses a fractional quantity outright -- a `1.0`
    /// on the wire comes back as "Request body is invalid or missing".
    pub quantity: i64,
    pub unit_price: Option<Money>,
    pub total: Option<Money>,
    pub url: Option<String>,
    /// The shop this line is collected from, when it differs from the cart's.
    pub pickup_store: Option<String>,
    pub updateable: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Delivery {
    pub code: Option<String>,
    pub name: Option<String>,
    pub cost: Option<Money>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Customer {
    pub uid: Option<String>,
    pub name: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub email: Option<String>,
    pub addresses: Vec<Address>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct OrderPage {
    pub orders: Vec<Order>,
    pub total: u64,
    pub page: u64,
    pub pages: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Order {
    pub code: String,
    pub placed: Option<String>,
    pub status: Option<String>,
    pub total: Option<Money>,
    pub lines: Vec<OrderLine>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct OrderLine {
    pub code: String,
    pub name: String,
    /// Whole units. Hybris refuses a fractional quantity outright -- a `1.0`
    /// on the wire comes back as "Request body is invalid or missing".
    pub quantity: i64,
    pub total: Option<Money>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Wishlist {
    pub items: Vec<WishlistItem>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WishlistItem {
    pub code: String,
    pub name: String,
    pub price: Option<Money>,
    pub url: Option<String>,
}

/// One category, as `/categories/{code}` returns it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Category {
    pub code: String,
    pub name: String,
    pub url: Option<String>,
    pub meta_title: Option<String>,
    pub meta_description: Option<String>,
    /// Ancestors, outermost first. Hybris sends these and not the children.
    pub breadcrumbs: Vec<String>,
}

impl Category {
    /// Which `categoryID.lvlN` field in the search index holds this code.
    /// See [`CategoryNode::level`].
    pub fn level(&self) -> Option<u8> {
        CategoryNode::level_of(&self.code)
    }
}

/// A node of the navigation tree, which is where the category hierarchy lives.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CategoryNode {
    pub code: String,
    pub name: String,
    pub url: Option<String>,
    pub children: Vec<CategoryNode>,
}

impl CategoryNode {
    /// Which `categoryID.lvlN` field in the search index holds a code.
    ///
    /// The prefix letter *is* the depth: `RD` is a department (`lvl0`), `RS` a
    /// section (`lvl1`), `RF` a fineline (`lvl2`), `RC` a class (`lvl3`).
    /// Confirmed against the site's own browse queries. `N1`..`N9` are the top
    /// menu entries and are content pages rather than catalogue categories, so
    /// they have no level and cannot be browsed directly.
    pub fn level_of(code: &str) -> Option<u8> {
        match code.get(..2)?.to_ascii_uppercase().as_str() {
            "RD" => Some(0),
            "RS" => Some(1),
            "RF" => Some(2),
            "RC" => Some(3),
            _ => None,
        }
    }

    pub fn level(&self) -> Option<u8> {
        CategoryNode::level_of(&self.code)
    }

    /// Depth-first walk, with the path of names taken to reach each node.
    pub fn walk(&self, f: &mut impl FnMut(&CategoryNode, &[String])) {
        let mut path = Vec::new();
        self.walk_inner(&mut path, f);
    }

    fn walk_inner(&self, path: &mut Vec<String>, f: &mut impl FnMut(&CategoryNode, &[String])) {
        f(self, path);
        path.push(self.name.clone());
        for child in &self.children {
            child.walk_inner(path, f);
        }
        path.pop();
    }

    /// The first node with this code, anywhere beneath here.
    pub fn find(&self, code: &str) -> Option<&CategoryNode> {
        if self.code.eq_ignore_ascii_case(code) {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(code))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_category_codes_prefix_is_its_depth_in_the_index() {
        // The whole reason a bare code can be browsed without a lookup. Both
        // of these were confirmed against the site's own queries:
        // RF7336 -> categoryID.lvl2, RS2158 -> categoryID.lvl1.
        assert_eq!(CategoryNode::level_of("RD8002"), Some(0));
        assert_eq!(CategoryNode::level_of("RS2158"), Some(1));
        assert_eq!(CategoryNode::level_of("RF7336"), Some(2));
        assert_eq!(CategoryNode::level_of("RC37340"), Some(3));
    }

    #[test]
    fn a_top_menu_entry_has_no_level_because_it_is_not_a_catalogue_category() {
        // `N2` is the Garden landing page. Browsing it would need a real
        // category code, so answering `Some(0)` here would build a query that
        // silently matches nothing.
        assert_eq!(CategoryNode::level_of("N2"), None);
        assert_eq!(CategoryNode::level_of(""), None);
    }

    #[test]
    fn a_saving_is_only_claimed_when_the_price_actually_fell() {
        let mut p = Product {
            price: Some(69.0),
            was_price: Some(79.0),
            ..Default::default()
        };
        assert_eq!(p.saving(), Some(10.0));
        p.was_price = Some(69.0);
        assert_eq!(
            p.saving(),
            None,
            "the index sends both for a full-price item"
        );
    }

    #[test]
    fn an_unrecognised_stock_word_is_carried_rather_than_guessed() {
        assert_eq!(StockLevel::parse("LOW_STOCK"), StockLevel::LowStock);
        assert_eq!(StockLevel::parse("lowStock"), StockLevel::LowStock);
        assert_eq!(StockLevel::parse("SOMETHING_NEW"), StockLevel::Unknown);
        assert!(!StockLevel::Unknown.is_available());
        assert!(StockLevel::LowStock.is_available());
    }

    #[test]
    fn a_fulfilment_parses_from_what_people_type_and_renders_what_occ_takes() {
        assert_eq!(
            Fulfilment::parse("collect"),
            Some(Fulfilment::ClickToCollect)
        );
        assert_eq!(
            Fulfilment::parse("Click and Collect"),
            Some(Fulfilment::ClickToCollect)
        );
        assert_eq!(
            Fulfilment::parse("delivery"),
            Some(Fulfilment::HomeDelivery)
        );
        assert_eq!(Fulfilment::parse("post"), None);
        assert_eq!(Fulfilment::ClickToCollect.code(), "clickToCollect");
    }

    #[test]
    fn a_cart_is_addressed_by_the_handle_its_caller_is_entitled_to() {
        // Both handles are on both carts, so this cannot be inferred from the
        // cart: anonymous paths take the guid, signed-in paths the code.
        let cart = Cart {
            code: Some("3000744918".into()),
            guid: Some("f4eaa13e".into()),
            ..Default::default()
        };
        assert_eq!(cart.id_for(false), Some("f4eaa13e"), "anonymous: the guid");
        assert_eq!(cart.id_for(true), Some("3000744918"), "signed in: the code");

        // A cart with only one of them uses it either way.
        let fresh = Cart {
            guid: Some("f4eaa13e".into()),
            ..Default::default()
        };
        assert_eq!(fresh.id_for(true), Some("f4eaa13e"));
    }

    #[test]
    fn a_tree_walk_carries_the_path_that_reached_each_node() {
        let tree = CategoryNode {
            code: "N2".into(),
            name: "Garden".into(),
            children: vec![CategoryNode {
                code: "RD2002".into(),
                name: "Garden Equipment".into(),
                children: vec![CategoryNode {
                    code: "RS2158".into(),
                    name: "Rugs & Mats".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };

        let mut seen = Vec::new();
        tree.walk(&mut |node, path| seen.push((node.code.clone(), path.join(" > "))));
        assert_eq!(
            seen,
            vec![
                ("N2".into(), String::new()),
                ("RD2002".into(), "Garden".into()),
                ("RS2158".into(), "Garden > Garden Equipment".into()),
            ]
        );
        assert_eq!(
            tree.find("RS2158").map(|n| n.name.as_str()),
            Some("Rugs & Mats")
        );
        assert!(tree.find("RF9999").is_none());
    }
}
