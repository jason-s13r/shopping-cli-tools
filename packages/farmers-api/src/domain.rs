//! What this crate speaks, as opposed to what the three backends send.
//!
//! Vendor-shaped, not shared: there is no common domain crate and these types
//! do not try to be one. What they do enforce is the one fact that matters
//! about this catalogue -- **a product code is not a leaf**. `6867065` is a
//! master with no price of its own and `6867065002` is the size that can be
//! bought, and the search index returns the first while the basket takes the
//! second. [`Product::master`] is that distinction made checkable.
//!
//! Everything arrives optional. Half of it is scraped and the rest is an
//! undocumented JSON API, so a field Farmers renames should degrade to a
//! missing column rather than a failed command.

use serde::{Deserialize, Serialize};

/// An amount and what it is denominated in. Always NZD here, carried anyway
/// so a renderer never has to assume.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Money {
    pub value: f64,
    pub currency: String,
}

impl Money {
    pub fn new(value: f64, currency: impl Into<String>) -> Money {
        Money {
            value,
            currency: currency.into(),
        }
    }

    /// `$309.97`, `NZ$ 1,299.00`, `-$20.00` -- prices as the HTML half quotes
    /// them.
    ///
    /// Everything that is not a digit, a dot or a leading minus is dropped,
    /// which is what makes the thousands separator and the `NZ$` prefix
    /// harmless. A string with no digits in it at all -- `TBD`, which is what
    /// the cart says for delivery before an address is known -- is `None`
    /// rather than zero, because those are different facts.
    pub fn parse(text: &str) -> Option<Money> {
        let text = text.trim();
        let negative = text.starts_with('-');
        let digits: String = text
            .chars()
            .filter(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if digits.is_empty() {
            return None;
        }
        let value: f64 = digits.parse().ok()?;
        Some(Money::new(if negative { -value } else { value }, "NZD"))
    }

    /// `$89.99`. The symbol rather than the code, because every price on this
    /// storefront is in one currency and `NZD 89.99` reads as a conversion.
    pub fn display(&self) -> String {
        format!("${:.2}", self.value)
    }
}

/// One rendition of a product photo. The catalogue ships four sizes of each.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Image {
    pub url: String,
    /// `S`, `M`, `L` or `ZOOM`, as Intershop names them.
    pub size: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub primary: bool,
}

/// A step on a product's category path.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CategoryRef {
    pub id: String,
    pub name: String,
}

/// One value of one variation axis: `Size = L-XL`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VariationValue {
    pub axis: String,
    pub value: String,
}

/// A product, whether a master or one of its variants.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Product {
    pub sku: String,
    pub name: Option<String>,
    pub brand: Option<String>,
    pub short_description: Option<String>,
    pub long_description: Option<String>,
    /// The ticket price.
    pub list_price: Option<Money>,
    /// What it actually sells for. Equal to `list_price` for anything not
    /// reduced -- the API sends both regardless, so a saving has to be worked
    /// out rather than read off.
    pub sale_price: Option<Money>,
    /// A master's range, when its variants are not all one price.
    pub min_price: Option<Money>,
    pub max_price: Option<Money>,
    pub in_stock: Option<bool>,
    /// Units on hand for delivery, nationwide. Per-store figures come from
    /// [`StoreStock`] and are a different question.
    pub available_stock: Option<i64>,
    /// Set on a variant, naming the master it belongs to.
    pub master_sku: Option<String>,
    /// Whether this is the master rather than something buyable.
    pub master: bool,
    pub images: Vec<Image>,
    /// Root first, the product's own category last.
    pub category_path: Vec<CategoryRef>,
    /// On a variant: which size and colour this one is. On a master: every
    /// value any of its variants takes.
    pub variation_values: Vec<VariationValue>,
    /// Promotion titles, as the storefront words them.
    pub promotions: Vec<String>,
    pub rating: Option<f64>,
    pub review_count: Option<i64>,
    /// Working days to dispatch, as a range.
    pub ships_in: Option<(i64, i64)>,
}

impl Product {
    /// What it sells for, falling back to the ticket price.
    pub fn price(&self) -> Option<&Money> {
        self.sale_price.as_ref().or(self.list_price.as_ref())
    }

    /// The ticket price, but only when it is genuinely higher than the selling
    /// price.
    ///
    /// The API sends `listPrice` and `salePrice` as the same number for
    /// everything that is not reduced, so showing the pair unconditionally
    /// would claim a saving of zero on the whole catalogue.
    pub fn was(&self) -> Option<&Money> {
        let (list, sale) = (self.list_price.as_ref()?, self.sale_price.as_ref()?);
        (list.value > sale.value).then_some(list)
    }

    /// What is saved, when anything is.
    pub fn saving(&self) -> Option<Money> {
        let was = self.was()?;
        let now = self.price()?;
        Some(Money::new(was.value - now.value, &now.currency))
    }

    /// The largest picture there is.
    pub fn image(&self) -> Option<&Image> {
        self.images
            .iter()
            .find(|i| i.size.as_deref() == Some("ZOOM"))
            .or_else(|| self.images.iter().find(|i| i.primary))
            .or_else(|| self.images.first())
    }

    /// The category path as `Men > Sleepwear, Robes & Slippers > Robes`.
    pub fn breadcrumb(&self) -> String {
        self.category_path
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>()
            .join(" > ")
    }
}

/// One buyable variant of a master.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Variant {
    pub sku: String,
    pub name: Option<String>,
    pub values: Vec<VariationValue>,
    pub in_stock: Option<bool>,
    pub available_stock: Option<i64>,
    /// The one the product page opens on.
    pub default: bool,
}

impl Variant {
    /// `Colour Grey, Size L-XL` -- what tells one variant from another.
    pub fn label(&self) -> String {
        self.values
            .iter()
            .map(|v| format!("{} {}", v.axis, v.value))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// A node of the category tree.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Category {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// The path the website serves it under, `men/sleepwear-robes-slippers`.
    pub path: Option<String>,
    /// How many products sit below it, which the API counts only on the
    /// branches that have children.
    pub product_count: Option<i64>,
    pub has_products: bool,
    pub children: Vec<Category>,
}

impl Category {
    /// Every category at or below this one, depth first, each with the names
    /// above it.
    pub fn flatten(&self) -> Vec<(Vec<String>, &Category)> {
        let mut out = Vec::new();
        self.walk(&mut Vec::new(), &mut out);
        out
    }

    fn walk<'a>(&'a self, above: &mut Vec<String>, out: &mut Vec<(Vec<String>, &'a Category)>) {
        out.push((above.clone(), self));
        above.push(self.name.clone());
        for child in &self.children {
            child.walk(above, out);
        }
        above.pop();
    }
}

/// One product as the search index describes it.
///
/// Deliberately not a [`Product`]: the index carries no price at all, and a
/// type that could hold one but never does would have every renderer printing
/// a dash where a price belongs. Join to [`Product`] by SKU for money.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Hit {
    /// The master code. Not buyable on its own -- see [`Hit::variants`].
    pub sku: String,
    pub name: String,
    pub brand: Option<String>,
    /// The path on the website, for a `--json` consumer that wants to link.
    pub url: Option<String>,
    pub image: Option<String>,
    /// `In stock`, `Out of stock`, in the index's own words.
    pub stock_status: Option<String>,
    /// The buyable codes under this one, which is what a basket takes.
    pub variants: Vec<String>,
    /// The category ids it is filed under, joinable to [`Category::id`].
    pub categories: Vec<String>,
}

/// A facet the index offered, and what is in it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Facet {
    /// The name a filter is keyed by: `manufacturername`.
    pub name: String,
    /// The name a person sees: `Brand`.
    pub display_name: String,
    pub options: Vec<FacetOption>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FacetOption {
    pub value: String,
    pub display_name: String,
    pub count: i64,
}

/// A page of search or browse results.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Listing {
    pub hits: Vec<Hit>,
    pub total: i64,
    /// Zero-based, as this crate counts. The service counts from one; the
    /// conversion happens in [`crate::search`] so nothing above it has to know.
    pub page: u64,
    pub facets: Vec<Facet>,
    /// Where a merchandising rule sent this term instead of answering it.
    ///
    /// `lego` is not searched -- it redirects to `/toys/lego-construction`.
    /// A caller that ignores this gets an empty result set and no clue why.
    pub redirect: Option<String>,
}

/// What the site would offer for a partial term.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Suggestion {
    pub value: String,
    /// `Search Suggestions` or `Products`, as the index groups them.
    pub section: String,
}

/// A Farmers store.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Store {
    pub name: String,
    pub address: Option<String>,
    pub phone: Option<String>,
    /// Day and opening times, in the order the page lists them.
    pub hours: Vec<(String, String)>,
}

/// Whether one store has one product.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoreStock {
    pub store: Store,
    /// `In Stock` / `Not In Stock`, in the storefront's own words, because it
    /// quotes no number.
    pub status: String,
}

impl StoreStock {
    pub fn available(&self) -> bool {
        // The negative is the one worth matching: the positive has appeared as
        // both "In Stock" and "In Stock - Limited".
        !self.status.to_lowercase().contains("not in stock")
    }
}

/// The basket.
///
/// Read from the mini-cart fragment rather than the cart page: it is one small
/// request that carries every fact below, where the page is 160KB of markup
/// with the same data spread across table columns.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cart {
    /// The basket's own id. Not needed to change it -- the session is what
    /// identifies it -- but worth carrying so two runs can be told apart.
    pub id: Option<String>,
    pub currency: Option<String>,
    /// What the header shows, which counts *units* and not lines.
    pub count: Option<i64>,
    pub subtotal: Option<Money>,
    /// After discounts. Named as the storefront names it rather than
    /// "discounted": it is the number the checkout carries forward.
    pub total: Option<Money>,
    pub grand_total: Option<Money>,
    pub lines: Vec<CartLine>,
}

impl Cart {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Units across every line, from the lines themselves.
    ///
    /// [`Cart::count`] is what the header claims; this is what the rows add up
    /// to. They agree in every capture, and where they do not the rows are the
    /// evidence.
    pub fn units(&self) -> i64 {
        self.lines.iter().filter_map(|l| l.quantity).sum()
    }

    /// A line by the id the remove and update calls take.
    pub fn line(&self, id: &str) -> Option<&CartLine> {
        self.lines.iter().find(|l| l.id == id)
    }

    /// A line by its position as a listing shows it, counting from 1.
    ///
    /// The ids are opaque 24-character strings, so a person changing a
    /// quantity is never going to type one.
    pub fn nth(&self, item: usize) -> Option<&CartLine> {
        self.lines.get(item.checked_sub(1)?)
    }
}

/// One line of the basket.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CartLine {
    /// The product line item id, which is what every change to this line is
    /// addressed by. Opaque, and **not** the SKU.
    pub id: String,
    pub sku: Option<String>,
    pub name: Option<String>,
    pub url: Option<String>,
    pub quantity: Option<i64>,
    /// Per unit, after any discount.
    pub unit_price: Option<Money>,
    /// Per unit, before it.
    pub list_price: Option<Money>,
    /// The line's own total.
    pub total: Option<Money>,
    /// Which size and colour this line is, as the basket labels them.
    pub options: Vec<VariationValue>,
}

impl CartLine {
    /// `Colour Grey, Size L-XL`, or empty for something with no variants.
    pub fn label(&self) -> String {
        self.options
            .iter()
            .map(|v| format!("{} {}", v.axis, v.value))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// A saved list. Farmers has *several* per account, unlike the other
/// storefronts here, and one of them is the preferred one that a bare
/// "add to wishlist" writes to.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Wishlist {
    pub id: Option<String>,
    pub name: String,
    /// Where a plain add goes.
    pub preferred: bool,
    /// Shareable by link.
    pub public: bool,
    pub items: Vec<WishlistItem>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WishlistItem {
    pub sku: Option<String>,
    pub name: Option<String>,
    pub url: Option<String>,
    pub price: Option<Money>,
}

/// One past order, as the account's order history lists it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Order {
    pub number: String,
    /// As the page prints it. Not parsed into a date: the format is the
    /// storefront's to change, and a wrong date is worse than the text.
    pub placed: Option<String>,
    pub status: Option<String>,
    pub total: Option<Money>,
    /// The page for this order, where the history links to one.
    pub url: Option<String>,
}

/// Who the storefront thinks is asking.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Account {
    pub signed_in: bool,
    pub email: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
}

impl Account {
    pub fn name(&self) -> Option<String> {
        match (&self.first_name, &self.last_name) {
            (Some(first), Some(last)) => Some(format!("{first} {last}")),
            (Some(one), None) | (None, Some(one)) => Some(one.clone()),
            (None, None) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn priced(list: f64, sale: f64) -> Product {
        Product {
            list_price: Some(Money::new(list, "NZD")),
            sale_price: Some(Money::new(sale, "NZD")),
            ..Default::default()
        }
    }

    #[test]
    fn a_saving_is_only_claimed_when_the_price_actually_fell() {
        // The API sends both prices for everything, equal when nothing is
        // reduced -- so an unconditional pair would claim a $0.00 saving on the
        // whole catalogue.
        assert_eq!(priced(89.99, 89.99).was(), None);
        assert_eq!(priced(89.99, 89.99).saving(), None);

        let reduced = priced(89.99, 69.99);
        assert_eq!(reduced.price().map(|m| m.value), Some(69.99));
        assert_eq!(reduced.was().map(|m| m.value), Some(89.99));
        assert_eq!(reduced.saving().unwrap().display(), "$20.00");
    }

    #[test]
    fn a_product_with_only_a_ticket_price_still_has_a_price() {
        let p = Product {
            list_price: Some(Money::new(10.0, "NZD")),
            ..Default::default()
        };
        assert_eq!(p.price().map(|m| m.value), Some(10.0));
        assert_eq!(p.was(), None, "nothing to compare against");
    }

    #[test]
    fn the_biggest_picture_wins_and_a_product_may_have_none() {
        let p = Product {
            images: vec![
                Image {
                    url: "/s.jpg".into(),
                    size: Some("S".into()),
                    width: None,
                    height: None,
                    primary: true,
                },
                Image {
                    url: "/z.jpg".into(),
                    size: Some("ZOOM".into()),
                    width: None,
                    height: None,
                    primary: true,
                },
            ],
            ..Default::default()
        };
        assert_eq!(p.image().map(|i| i.url.as_str()), Some("/z.jpg"));
        assert_eq!(Product::default().image(), None);
    }

    #[test]
    fn stock_is_read_off_the_negative_because_the_positive_varies() {
        // "In Stock" and "In Stock - Limited" are both available; matching the
        // positive exactly would call the second one sold out.
        let store = Store::default();
        let stock = |s: &str| StoreStock {
            store: store.clone(),
            status: s.into(),
        };
        assert!(stock("In Stock").available());
        assert!(stock("In Stock - Limited").available());
        assert!(!stock("Not In Stock").available());
        assert!(!stock("not in stock").available());
    }

    #[test]
    fn a_flattened_tree_carries_the_names_above_each_node() {
        let tree = Category {
            id: "51-03".into(),
            name: "Men".into(),
            children: vec![Category {
                id: "51-0303".into(),
                name: "Sleepwear".into(),
                children: vec![Category {
                    id: "51-030302".into(),
                    name: "Robes".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let flat = tree.flatten();
        assert_eq!(flat.len(), 3);
        assert_eq!(flat[0].0, Vec::<String>::new());
        assert_eq!(flat[2].0, vec!["Men".to_string(), "Sleepwear".to_string()]);
        assert_eq!(flat[2].1.id, "51-030302");
    }

    #[test]
    fn a_variant_is_labelled_by_what_makes_it_different() {
        let v = Variant {
            sku: "6867065002".into(),
            name: None,
            values: vec![
                VariationValue {
                    axis: "Colour".into(),
                    value: "Grey".into(),
                },
                VariationValue {
                    axis: "Size".into(),
                    value: "L-XL".into(),
                },
            ],
            in_stock: Some(true),
            available_stock: Some(31),
            default: true,
        };
        assert_eq!(v.label(), "Colour Grey, Size L-XL");
    }

    #[test]
    fn a_price_is_read_out_of_whatever_the_markup_wrapped_it_in() {
        assert_eq!(Money::parse("$309.97").unwrap().value, 309.97);
        assert_eq!(Money::parse("NZ$ 1,299.00").unwrap().value, 1299.00);
        assert_eq!(Money::parse("  $89.99  ").unwrap().value, 89.99);
        assert_eq!(Money::parse("-$20.00").unwrap().value, -20.00);
    }

    #[test]
    fn a_price_that_is_not_a_number_is_absent_rather_than_zero() {
        // The cart says "TBD" for delivery until an address is known, and
        // "$0.00 delivery" is a different claim from "not worked out yet".
        assert_eq!(Money::parse("TBD"), None);
        assert_eq!(Money::parse(""), None);
        assert_eq!(Money::parse("—"), None);
    }

    #[test]
    fn a_cart_counts_units_from_its_rows_rather_than_trusting_the_header() {
        let line = |id: &str, qty: i64| CartLine {
            id: id.into(),
            quantity: Some(qty),
            ..Default::default()
        };
        let cart = Cart {
            count: Some(99),
            lines: vec![line("a", 2), line("b", 1)],
            ..Default::default()
        };
        assert_eq!(cart.units(), 3, "the rows are the evidence");
        assert!(!cart.is_empty());
        assert_eq!(cart.line("b").map(|l| l.id.as_str()), Some("b"));
        assert_eq!(cart.line("nope"), None);
    }

    #[test]
    fn a_cart_line_is_addressable_by_the_number_a_listing_showed() {
        // The real ids are opaque 24-character strings; nobody is typing one.
        let line = |id: &str| CartLine {
            id: id.into(),
            ..Default::default()
        };
        let cart = Cart {
            lines: vec![line("first"), line("second")],
            ..Default::default()
        };
        assert_eq!(cart.nth(1).map(|l| l.id.as_str()), Some("first"));
        assert_eq!(cart.nth(2).map(|l| l.id.as_str()), Some("second"));
        assert_eq!(cart.nth(3), None);
        // Counting from 1, so 0 is a mistake rather than the first row.
        assert_eq!(cart.nth(0), None);
    }

    #[test]
    fn an_account_with_half_a_name_still_has_one() {
        let mut a = Account {
            signed_in: true,
            ..Default::default()
        };
        assert_eq!(a.name(), None);
        a.first_name = Some("Ada".into());
        assert_eq!(a.name().as_deref(), Some("Ada"));
        a.last_name = Some("Lovelace".into());
        assert_eq!(a.name().as_deref(), Some("Ada Lovelace"));
    }
}
