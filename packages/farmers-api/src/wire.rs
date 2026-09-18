//! The JSON, exactly as the two JSON backends send it.
//!
//! Kept apart from [`crate::domain`] so a field Farmers renames costs one edit
//! here rather than a change to everything that reads a product. Almost every
//! field is `Option`: neither API is documented, and this crate would rather
//! print a dash than fail a command.
//!
//! Intershop's own shape is worth knowing. A list is `{"elements": [...]}`
//! whatever it lists, an `attributes` array is a bag of loosely typed
//! name/value pairs hanging off most records, and a cross-reference is a `uri`
//! of the form `Farmers-Shop-Site/-;loc=en_NZ/categories/51-03` -- a path
//! fragment, not a URL, with the id on the end.

use serde::Deserialize;

use crate::domain::{
    Account, Category, CategoryRef, Facet, FacetOption, Hit, Image, Listing, Money, Product,
    Suggestion, Variant, VariationValue,
};

// ---- Intershop ----

/// The envelope every Intershop list arrives in.
#[derive(Debug, Deserialize)]
pub struct Elements<T> {
    #[serde(default = "Vec::new")]
    pub elements: Vec<T>,
}

/// A loosely typed name/value pair. `value` is [`serde_json::Value`] because
/// the same array holds booleans, strings and numbers.
#[derive(Debug, Deserialize)]
pub struct Attribute {
    pub name: String,
    #[serde(default)]
    pub value: serde_json::Value,
}

fn attribute<'a>(attributes: &'a [Attribute], name: &str) -> Option<&'a serde_json::Value> {
    attributes.iter().find(|a| a.name == name).map(|a| &a.value)
}

#[derive(Debug, Deserialize)]
pub struct Price {
    pub value: Option<f64>,
    pub currency: Option<String>,
}

impl Price {
    fn money(self) -> Option<Money> {
        Some(Money::new(
            self.value?,
            self.currency.unwrap_or_else(|| "NZD".into()),
        ))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireImage {
    pub effective_url: Option<String>,
    /// Not `typeId`: Intershop capitalises the whole abbreviation, so
    /// `rename_all` alone looks for a field that is not there.
    #[serde(rename = "typeID")]
    pub type_id: Option<String>,
    pub image_actual_width: Option<u32>,
    pub image_actual_height: Option<u32>,
    #[serde(default)]
    pub primary_image: bool,
}

#[derive(Debug, Deserialize)]
pub struct PathEntry {
    pub id: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultCategory {
    #[serde(default)]
    pub category_path: Vec<PathEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VariationAttribute {
    pub name: Option<String>,
    pub value: Option<serde_json::Value>,
}

impl VariationAttribute {
    fn value(self) -> Option<VariationValue> {
        let raw = self.value?;
        // Loosely typed like everything else in this API: a size arrives as a
        // string, and a numeric one as a number.
        let value = match raw {
            serde_json::Value::String(s) => s,
            serde_json::Value::Null => return None,
            other => other.to_string(),
        };
        Some(VariationValue {
            axis: self.name?,
            value,
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct Promotion {
    pub title: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireProduct {
    pub sku: Option<String>,
    pub product_name: Option<String>,
    pub name: Option<String>,
    pub short_description: Option<String>,
    pub long_description: Option<String>,
    pub manufacturer: Option<String>,
    pub list_price: Option<Price>,
    pub sale_price: Option<Price>,
    pub min_sale_price: Option<Price>,
    pub max_sale_price: Option<Price>,
    pub in_stock: Option<bool>,
    /// A number on a product record and a **string** on a variation record.
    /// Same field, same API, two types -- so it is read loosely and parsed.
    pub available_stock: Option<serde_json::Value>,
    pub product_master: Option<bool>,
    /// Not `productMasterSku`, for the same reason as `typeID` above. Silent
    /// when wrong: every variant reports no master and nothing else changes.
    #[serde(rename = "productMasterSKU")]
    pub product_master_sku: Option<String>,
    #[serde(default)]
    pub images: Vec<WireImage>,
    pub default_category: Option<DefaultCategory>,
    #[serde(default)]
    pub variable_variation_attributes: Vec<VariationAttribute>,
    #[serde(default)]
    pub variation_attribute_values: Vec<VariationAttribute>,
    #[serde(default)]
    pub promotions: Vec<Promotion>,
    pub average_rating: Option<String>,
    pub number_of_reviews: Option<i64>,
    pub ready_for_shipment_min: Option<i64>,
    pub ready_for_shipment_max: Option<i64>,
}

/// Read a count that arrives as a number from one endpoint and a string from
/// another.
fn loose_i64(value: Option<&serde_json::Value>) -> Option<i64> {
    match value? {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

impl WireProduct {
    pub fn into_product(self) -> Product {
        let master = self.product_master.unwrap_or(false);
        // A master describes its variants' values; a variant describes its own.
        // One field or the other is populated, never both.
        let values = if self.variable_variation_attributes.is_empty() {
            self.variation_attribute_values
        } else {
            self.variable_variation_attributes
        };
        Product {
            sku: self.sku.unwrap_or_default(),
            name: self.product_name.or(self.name),
            brand: self.manufacturer.filter(|b| !b.is_empty()),
            short_description: self.short_description.filter(|d| !d.is_empty()),
            long_description: self.long_description.filter(|d| !d.is_empty()),
            list_price: self.list_price.and_then(Price::money),
            sale_price: self.sale_price.and_then(Price::money),
            min_price: self.min_sale_price.and_then(Price::money),
            max_price: self.max_sale_price.and_then(Price::money),
            in_stock: self.in_stock,
            available_stock: loose_i64(self.available_stock.as_ref()),
            master_sku: self.product_master_sku,
            master,
            images: self
                .images
                .into_iter()
                .filter_map(|i| {
                    Some(Image {
                        url: i.effective_url?,
                        size: i.type_id,
                        width: i.image_actual_width,
                        height: i.image_actual_height,
                        primary: i.primary_image,
                    })
                })
                .collect(),
            category_path: self
                .default_category
                .map(|c| c.category_path)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|e| {
                    Some(CategoryRef {
                        id: e.id?,
                        name: e.name.unwrap_or_default(),
                    })
                })
                .collect(),
            variation_values: values
                .into_iter()
                .filter_map(VariationAttribute::value)
                .collect(),
            promotions: self
                .promotions
                .into_iter()
                .filter_map(|p| p.title)
                .collect(),
            // Sent as a string, and `0.0` on everything unrated -- which is not
            // the same fact as "rated zero", so it is dropped.
            rating: self
                .average_rating
                .and_then(|r| r.parse::<f64>().ok())
                .filter(|r| *r > 0.0),
            review_count: self.number_of_reviews,
            ships_in: self.ready_for_shipment_min.zip(self.ready_for_shipment_max),
        }
    }
}

/// One entry of `/products/{sku}/variations`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireVariation {
    pub uri: Option<String>,
    pub title: Option<String>,
    #[serde(default)]
    pub attributes: Vec<Attribute>,
    #[serde(default)]
    pub variable_variation_attribute_values: Vec<VariationAttribute>,
    pub in_stock: Option<bool>,
    pub available_stock: Option<serde_json::Value>,
}

impl WireVariation {
    pub fn into_variant(self) -> Option<Variant> {
        Some(Variant {
            // The only place the variant's code appears: there is no `sku`
            // field on this record, just the cross-reference it hangs off.
            sku: id_of(self.uri.as_deref()?)?,
            name: self.title,
            default: attribute(&self.attributes, "defaultVariation")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            values: self
                .variable_variation_attribute_values
                .into_iter()
                .filter_map(VariationAttribute::value)
                .collect(),
            in_stock: self.in_stock,
            available_stock: loose_i64(self.available_stock.as_ref()),
        })
    }
}

/// The id off the end of an Intershop cross-reference.
///
/// These are path fragments rather than URLs --
/// `Farmers-Shop-Site/-;loc=en_NZ/products/6867065002` -- so this takes the
/// last segment rather than parsing. A trailing slash or a query would defeat
/// that, and neither has ever appeared on one.
pub fn id_of(uri: &str) -> Option<String> {
    uri.rsplit('/').find(|s| !s.is_empty()).map(str::to_string)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireCategory {
    pub id: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub attributes: Vec<Attribute>,
    #[serde(default)]
    pub has_online_products: bool,
    pub online_products_count_in_sub_categories: Option<i64>,
    #[serde(default)]
    pub sub_categories: Vec<WireCategory>,
}

impl WireCategory {
    pub fn into_category(self) -> Category {
        Category {
            id: self.id.unwrap_or_default(),
            name: self.name.unwrap_or_default(),
            // Every category carries this, and on the top-level ones it is the
            // catalogue's internal note rather than anything a shopper would
            // read -- "DO NOT DELETE - Farmers Catalog for Brands".
            description: self
                .description
                .filter(|d| !d.is_empty() && !d.contains("DO NOT DELETE")),
            path: attribute(&self.attributes, "URLRewrite")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            product_count: self.online_products_count_in_sub_categories,
            has_products: self.has_online_products,
            children: self
                .sub_categories
                .into_iter()
                .map(WireCategory::into_category)
                .collect(),
        }
    }
}

// ---- Constructor.io ----

#[derive(Debug, Deserialize)]
pub struct SearchEnvelope {
    pub response: SearchResponse,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SearchResponse {
    #[serde(default = "Vec::new")]
    pub results: Vec<WireHit>,
    #[serde(default)]
    pub total_num_results: i64,
    #[serde(default = "Vec::new")]
    pub facets: Vec<WireFacet>,
    /// Present instead of results when a merchandising rule claims the term.
    pub redirect: Option<Redirect>,
}

#[derive(Debug, Deserialize)]
pub struct Redirect {
    pub data: Option<RedirectData>,
}

#[derive(Debug, Deserialize)]
pub struct RedirectData {
    pub url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WireHit {
    /// The product title. Named `value` because this index is generic over
    /// what it holds.
    pub value: Option<String>,
    pub data: Option<HitData>,
}

#[derive(Debug, Deserialize)]
pub struct HitData {
    pub id: Option<String>,
    pub url: Option<String>,
    #[serde(default)]
    pub sku: Vec<String>,
    pub image_url: Option<String>,
    pub manufacturername: Option<String>,
    pub stockstatus: Option<String>,
    #[serde(default)]
    pub group_ids: Vec<String>,
}

impl WireHit {
    pub fn into_hit(self) -> Option<Hit> {
        let data = self.data?;
        let sku = data.id?;
        Some(Hit {
            name: self.value.unwrap_or_default(),
            brand: data.manufacturername.filter(|b| !b.is_empty()),
            url: data.url,
            image: data.image_url,
            stock_status: data.stockstatus,
            // The master's own code leads its own `sku` list; what is left is
            // the buyable variants.
            variants: data.sku.into_iter().filter(|s| *s != sku).collect(),
            categories: data.group_ids,
            sku,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct WireFacet {
    pub name: Option<String>,
    pub display_name: Option<String>,
    #[serde(default = "Vec::new")]
    pub options: Vec<WireFacetOption>,
    #[serde(default)]
    pub hidden: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct WireFacetOption {
    pub value: Option<String>,
    pub display_name: Option<String>,
    #[serde(default)]
    pub count: i64,
}

impl WireFacet {
    fn into_facet(self) -> Option<Facet> {
        let name = self.name?;
        Some(Facet {
            options: self
                .options
                .into_iter()
                .filter_map(|o| {
                    let value = o.value?;
                    Some(FacetOption {
                        display_name: o.display_name.unwrap_or_else(|| value.clone()),
                        value,
                        count: o.count,
                    })
                })
                .collect(),
            // Several facets are named only by their index key, so the
            // "display" name is the raw one and there is nothing better.
            display_name: self.display_name.unwrap_or_else(|| name.clone()),
            name,
        })
    }
}

impl SearchResponse {
    pub fn into_listing(self, page: u64) -> Listing {
        Listing {
            hits: self
                .results
                .into_iter()
                .filter_map(WireHit::into_hit)
                .collect(),
            total: self.total_num_results,
            page,
            facets: self
                .facets
                .into_iter()
                .filter(|f| !f.hidden)
                .filter_map(WireFacet::into_facet)
                .collect(),
            redirect: self.redirect.and_then(|r| r.data).and_then(|d| d.url),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct AutocompleteEnvelope {
    #[serde(default)]
    pub sections: std::collections::BTreeMap<String, Vec<WireSuggestion>>,
}

#[derive(Debug, Deserialize)]
pub struct WireSuggestion {
    pub value: Option<String>,
}

impl AutocompleteEnvelope {
    pub fn into_suggestions(self) -> Vec<Suggestion> {
        self.sections
            .into_iter()
            .flat_map(|(section, items)| {
                items.into_iter().filter_map(move |i| {
                    Some(Suggestion {
                        value: i.value?,
                        section: section.clone(),
                    })
                })
            })
            .collect()
    }
}

/// Everything a wire response can become, for the one caller that wants the
/// account out of a scraped fragment rather than out of JSON.
pub fn account(signed_in: bool) -> Account {
    Account {
        signed_in,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VARIANT: &str = r#"{
        "sku": "6867065002",
        "productName": "Chisel Fleece Robe, Charcoal",
        "manufacturer": "Chisel",
        "availableStock": 31,
        "inStock": true,
        "productMaster": false,
        "productMasterSKU": "6867065",
        "averageRating": "0.0",
        "readyForShipmentMin": 3,
        "readyForShipmentMax": 7,
        "listPrice": { "value": 89.99, "currency": "NZD" },
        "salePrice": { "value": 69.99, "currency": "NZD" },
        "images": [
            { "effectiveUrl": "/a_W110_H143.jpg", "typeID": "S", "primaryImage": true },
            { "effectiveUrl": "/a_W1200_H1565.jpg", "typeID": "ZOOM", "primaryImage": true }
        ],
        "defaultCategory": {
            "categoryPath": [
                { "name": "Men", "id": "51-03" },
                { "name": "Robes", "id": "51-030302" }
            ]
        },
        "variableVariationAttributes": [
            { "name": "Colour", "value": "Grey" },
            { "name": "Size", "value": "L-XL" }
        ],
        "promotions": [ { "title": "Spend & Save" } ]
    }"#;

    #[test]
    fn a_variant_carries_everything_a_listing_needs() {
        let p: WireProduct = serde_json::from_str(VARIANT).expect("parses");
        let p = p.into_product();
        assert_eq!(p.sku, "6867065002");
        assert_eq!(p.brand.as_deref(), Some("Chisel"));
        assert_eq!(p.available_stock, Some(31));
        assert_eq!(p.master_sku.as_deref(), Some("6867065"));
        assert!(!p.master);
        assert_eq!(p.saving().unwrap().display(), "$20.00");
        assert_eq!(p.breadcrumb(), "Men > Robes");
        assert_eq!(
            p.image().map(|i| i.url.as_str()),
            Some("/a_W1200_H1565.jpg")
        );
        assert_eq!(p.promotions, vec!["Spend & Save".to_string()]);
        assert_eq!(p.ships_in, Some((3, 7)));
    }

    #[test]
    fn an_unrated_product_has_no_rating_rather_than_a_rating_of_zero() {
        // The API sends "0.0" for everything nobody has reviewed. Carrying it
        // through would print half the catalogue as rated zero stars.
        let p: WireProduct = serde_json::from_str(VARIANT).unwrap();
        assert_eq!(p.into_product().rating, None);
    }

    #[test]
    fn available_stock_parses_whether_it_arrives_as_a_number_or_a_string() {
        // `/products/{sku}` sends a number and `/variations` sends a string,
        // for the same field. A typed `i64` would fail one of the two.
        let as_number: WireProduct = serde_json::from_str(r#"{"availableStock": 31}"#).unwrap();
        assert_eq!(as_number.into_product().available_stock, Some(31));
        let as_string: WireProduct = serde_json::from_str(r#"{"availableStock": "14"}"#).unwrap();
        assert_eq!(as_string.into_product().available_stock, Some(14));
    }

    #[test]
    fn a_variation_takes_its_code_from_the_only_place_it_appears() {
        // There is no `sku` field on a variation record at all -- just the
        // cross-reference it hangs off.
        let raw = r#"{
            "uri": "Farmers-Shop-Site/-;loc=en_NZ/products/6867065001",
            "title": "Chisel Fleece Robe, Charcoal",
            "attributes": [ { "name": "defaultVariation", "value": true } ],
            "variableVariationAttributeValues": [ { "name": "Size", "value": "S-M" } ],
            "inStock": true,
            "availableStock": "14"
        }"#;
        let v: WireVariation = serde_json::from_str(raw).unwrap();
        let v = v.into_variant().expect("has a uri");
        assert_eq!(v.sku, "6867065001");
        assert!(v.default);
        assert_eq!(v.available_stock, Some(14));
        assert_eq!(v.label(), "Size S-M");
    }

    #[test]
    fn a_variation_with_no_cross_reference_is_dropped_rather_than_given_a_blank_code() {
        // A blank SKU would be passed to the basket and fail as "no such
        // product", which says nothing about what went wrong.
        let v: WireVariation = serde_json::from_str(r#"{"title": "x"}"#).unwrap();
        assert!(v.into_variant().is_none());
    }

    #[test]
    fn a_categorys_internal_note_is_not_shown_as_its_description() {
        let raw = r#"{
            "id": "Brands", "name": "Brand",
            "description": "DO NOT DELETE - Farmers Catalog for Brands",
            "attributes": [ { "name": "URLRewrite", "value": "brand" } ],
            "hasOnlineProducts": false,
            "onlineProductsCountInSubCategories": 29893
        }"#;
        let c: WireCategory = serde_json::from_str(raw).unwrap();
        let c = c.into_category();
        assert_eq!(c.description, None);
        assert_eq!(c.path.as_deref(), Some("brand"));
        assert_eq!(c.product_count, Some(29893));
    }

    #[test]
    fn a_search_hit_separates_the_master_from_what_can_be_bought() {
        // The index returns the master first in its own `sku` list; a caller
        // that adds `id` to a basket is adding something unbuyable.
        let raw = r#"{
            "value": "Chisel Fleece Robe, Charcoal",
            "data": {
                "id": "6867065",
                "sku": ["6867065", "6867065001", "6867065002"],
                "manufacturername": "Chisel",
                "stockstatus": "In stock",
                "url": "men/robes/chisel-fleece-robe-charcoal-6867065",
                "group_ids": ["51-03", "Chisel"]
            }
        }"#;
        let h: WireHit = serde_json::from_str(raw).unwrap();
        let h = h.into_hit().expect("has an id");
        assert_eq!(h.sku, "6867065");
        assert_eq!(
            h.variants,
            vec!["6867065001".to_string(), "6867065002".to_string()]
        );
        assert_eq!(h.brand.as_deref(), Some("Chisel"));
    }

    #[test]
    fn a_redirect_is_carried_rather_than_read_as_an_empty_result() {
        // `lego` is not searched; it is redirected. A caller that ignored this
        // would report "no results" for one of the busiest terms on the site.
        let raw = r#"{ "response": {
            "results": [], "total_num_results": 0,
            "redirect": { "data": { "url": "/toys/lego-construction" } }
        } }"#;
        let e: SearchEnvelope = serde_json::from_str(raw).unwrap();
        let listing = e.response.into_listing(0);
        assert!(listing.hits.is_empty());
        assert_eq!(listing.redirect.as_deref(), Some("/toys/lego-construction"));
    }

    #[test]
    fn a_hidden_facet_is_not_offered() {
        let raw = r#"{ "response": { "results": [], "total_num_results": 0, "facets": [
            { "name": "manufacturername", "display_name": "Brand", "hidden": false,
              "options": [ { "value": "Chisel", "display_name": "Chisel", "count": 26 } ] },
            { "name": "internal", "display_name": "Internal", "hidden": true, "options": [] }
        ] } }"#;
        let e: SearchEnvelope = serde_json::from_str(raw).unwrap();
        let listing = e.response.into_listing(0);
        assert_eq!(listing.facets.len(), 1);
        assert_eq!(listing.facets[0].display_name, "Brand");
        assert_eq!(listing.facets[0].options[0].count, 26);
    }

    #[test]
    fn suggestions_keep_the_section_they_came_from() {
        let raw = r#"{ "sections": {
            "Search Suggestions": [ { "value": "robe" } ],
            "Products": [ { "value": "Whistle Sleep Printed Robe" } ]
        } }"#;
        let e: AutocompleteEnvelope = serde_json::from_str(raw).unwrap();
        let s = e.into_suggestions();
        assert_eq!(s.len(), 2);
        assert!(s.iter().any(|s| s.section == "Products"));
    }

    #[test]
    fn a_cross_reference_gives_up_its_id() {
        assert_eq!(
            id_of("Farmers-Shop-Site/-;loc=en_NZ/categories/51-030302").as_deref(),
            Some("51-030302")
        );
        assert_eq!(id_of("").as_deref(), None);
    }

    #[test]
    fn an_answer_missing_every_optional_field_still_parses() {
        // The point of the exercise: a renamed field should cost a column, not
        // a command.
        let p: WireProduct = serde_json::from_str("{}").expect("parses");
        let p = p.into_product();
        assert_eq!(p.sku, "");
        assert_eq!(p.price(), None);
        assert!(p.images.is_empty());
    }
}
