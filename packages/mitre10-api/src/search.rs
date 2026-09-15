//! Search and browse, which are Algolia rather than OCC.
//!
//! Every product grid on the site is an Algolia query, and browsing a category
//! is the same query with a `categoryID.lvlN` filter instead of a term. OCC's
//! own catalogue endpoints answer one product at a time and know nothing about
//! the facets the site offers, so a listing built on them would quietly
//! disagree with the website it is meant to mirror.
//!
//! The API key is search-only and the storefront publishes it in the query
//! string of every request it makes, so nothing here needs credentials. It is
//! not held in this crate either: [`crate::Client::algolia`] asks the
//! storefront for it.

use serde::Serialize;

/// How many products a page holds. The site asks for 24 browsing and 20
/// searching; one number is easier to reason about and neither is a limit.
pub const PAGE_SIZE: u64 = 24;

/// The product index. Its name ends in `_relevance` because it is the
/// relevance-ordered one; see [`Sort`].
pub const PRODUCT_INDEX: &str = "retail_products_relevance";

/// The orderings this crate offers by name.
///
/// Only `relevance` is confirmed against a capture of the live site. Algolia
/// replicas are conventionally named `<index>_<order>`, so the others very
/// likely exist, but an index that does not answers `404` -- which is why they
/// are not listed here and [`Sort::Index`] is the way to ask for one.
pub const SORTS: &[&str] = &["relevance"];

/// Which index answers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Relevance,
    /// A replica index by name, for an ordering this crate has not confirmed.
    Index(String),
}

impl Sort {
    pub fn parse(name: &str) -> Sort {
        match name.trim().to_ascii_lowercase().as_str() {
            "" | "relevance" | "default" => Sort::Relevance,
            other => Sort::Index(other.replace('-', "_")),
        }
    }

    /// The index to query.
    ///
    /// A bare word is taken as a replica suffix (`price_asc` becomes
    /// `retail_products_price_asc`); anything already carrying the prefix is
    /// passed through, so a full index name also works.
    pub fn index(&self) -> String {
        match self {
            Sort::Relevance => PRODUCT_INDEX.to_string(),
            Sort::Index(name) if name.starts_with("retail_") => name.clone(),
            Sort::Index(name) => format!("retail_products_{name}"),
        }
    }
}

/// Where the products must be gettable from.
///
/// The site always sets one of these, and leaving it off is what makes a
/// listing here disagree with the website: unfiltered, the index answers with
/// products no shop near you can supply.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Availability {
    /// A numeric store code. Matches the `clickAndCollect` array.
    pub collect_from: Option<String>,
    /// A postcode *group* id, not a postcode -- see
    /// [`crate::Client::postcode_group`].
    pub deliver_to: Option<String>,
}

impl Availability {
    pub fn collect(store: impl Into<String>) -> Availability {
        Availability {
            collect_from: Some(store.into()),
            ..Default::default()
        }
    }

    pub fn deliver(group: impl Into<String>) -> Availability {
        Availability {
            deliver_to: Some(group.into()),
            ..Default::default()
        }
    }

    fn clause(&self) -> Option<String> {
        let mut any = Vec::new();
        if let Some(store) = &self.collect_from {
            any.push(format!("clickAndCollect:{store}"));
        }
        if let Some(group) = &self.deliver_to {
            any.push(format!("homeDelivery:{group}"));
            any.push(format!("expressDelivery:{group}"));
        }
        (!any.is_empty()).then(|| format!("({})", any.join(" OR ")))
    }
}

/// What to ask the index.
#[derive(Clone, Debug, Default)]
pub struct Query {
    term: Option<String>,
    category: Option<String>,
    sort: Sort,
    availability: Availability,
    /// Numeric store code; products the index believes that shop has.
    in_stock_at: Option<String>,
    brand: Option<String>,
    colour: Option<String>,
    size: Option<String>,
    min_price: Option<f64>,
    max_price: Option<f64>,
    page: u64,
    page_size: Option<u64>,
}

/// The facets worth asking for, which are the ones the site's own sidebar
/// offers. Algolia returns counts only for facets named here.
const FACETS: &[&str] = &[
    "availableNationWide",
    "brandName",
    "colour",
    "essential",
    "prices.sortPrice",
    "size",
    "storesWithStock",
    "subFineline",
];

impl Query {
    /// A keyword search.
    pub fn search(term: impl Into<String>) -> Query {
        Query {
            term: Some(term.into()),
            ..Default::default()
        }
    }

    /// Everything in a category.
    ///
    /// The code's prefix decides which `categoryID.lvlN` is filtered, so a
    /// bare code is enough and no lookup is needed. A code with no level --
    /// a top menu entry such as `N2` -- filters nothing and is rejected by
    /// [`Query::category_level`] rather than silently matching everything.
    pub fn category(code: impl Into<String>) -> Query {
        Query {
            term: Some(String::new()),
            category: Some(code.into()),
            ..Default::default()
        }
    }

    pub fn with_sort(mut self, sort: Sort) -> Query {
        self.sort = sort;
        self
    }

    pub fn with_availability(mut self, availability: Availability) -> Query {
        self.availability = availability;
        self
    }

    pub fn in_stock_at(mut self, store: Option<String>) -> Query {
        self.in_stock_at = store;
        self
    }

    pub fn with_brand(mut self, brand: Option<String>) -> Query {
        self.brand = brand;
        self
    }

    pub fn with_colour(mut self, colour: Option<String>) -> Query {
        self.colour = colour;
        self
    }

    pub fn with_size(mut self, size: Option<String>) -> Query {
        self.size = size;
        self
    }

    pub fn with_price(mut self, min: Option<f64>, max: Option<f64>) -> Query {
        self.min_price = min;
        self.max_price = max;
        self
    }

    pub fn with_page(mut self, page: u64) -> Query {
        self.page = page;
        self
    }

    pub fn with_page_size(mut self, size: Option<u64>) -> Query {
        self.page_size = size;
        self
    }

    pub fn page(&self) -> u64 {
        self.page
    }

    pub fn page_size(&self) -> u64 {
        self.page_size.unwrap_or(PAGE_SIZE)
    }

    pub fn term(&self) -> Option<&str> {
        self.term.as_deref().filter(|t| !t.is_empty())
    }

    pub fn index(&self) -> String {
        self.sort.index()
    }

    /// The category being browsed and the index level that holds it.
    pub fn category_level(&self) -> Option<(&str, u8)> {
        let code = self.category.as_deref()?;
        let level = crate::CategoryNode::level_of(code)?;
        Some((code, level))
    }

    /// Algolia's `filters` string.
    fn filters(&self) -> String {
        let mut clauses = vec!["online:true".to_string()];
        if let Some(clause) = self.availability.clause() {
            clauses.push(clause);
        }
        if let Some((code, level)) = self.category_level() {
            clauses.push(format!("categoryID.lvl{level}:{code}"));
        }
        clauses.join(" AND ")
    }

    /// Algolia's `facetFilters`, one group per attribute. Separate groups are
    /// ANDed, which is what the site does for a brand *and* a colour.
    fn facet_filters(&self) -> Vec<Vec<String>> {
        [
            self.brand.as_ref().map(|v| format!("brandName:{v}")),
            self.colour.as_ref().map(|v| format!("colour:{v}")),
            self.size.as_ref().map(|v| format!("size:{v}")),
            self.in_stock_at
                .as_ref()
                .map(|v| format!("storesWithStock:{v}")),
        ]
        .into_iter()
        .flatten()
        .map(|f| vec![f])
        .collect()
    }

    fn numeric_filters(&self) -> Vec<String> {
        [
            self.min_price.map(|v| format!("prices.sortPrice>={v}")),
            self.max_price.map(|v| format!("prices.sortPrice<={v}")),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    /// The `params` string of one record query, form-encoded as Algolia wants.
    pub fn params(&self) -> String {
        let mut params = vec![
            ("query".to_string(), self.term.clone().unwrap_or_default()),
            ("hitsPerPage".to_string(), self.page_size().to_string()),
            ("page".to_string(), self.page.to_string()),
            ("filters".to_string(), self.filters()),
            (
                "facets".to_string(),
                serde_json::to_string(FACETS).unwrap_or_default(),
            ),
            ("maxValuesPerFacet".to_string(), "100".to_string()),
            (
                "ruleContexts".to_string(),
                serde_json::to_string(&["standard"]).unwrap_or_default(),
            ),
        ];

        let facet_filters = self.facet_filters();
        if !facet_filters.is_empty() {
            params.push((
                "facetFilters".to_string(),
                serde_json::to_string(&facet_filters).unwrap_or_default(),
            ));
        }
        let numeric = self.numeric_filters();
        if !numeric.is_empty() {
            params.push((
                "numericFilters".to_string(),
                serde_json::to_string(&numeric).unwrap_or_default(),
            ));
        }

        params
            .iter()
            .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
            .collect::<Vec<_>>()
            .join("&")
    }

    /// The request body: one record query in the list Algolia's multi-query
    /// endpoint expects.
    pub fn body(&self) -> serde_json::Value {
        serde_json::json!({
            "requests": [{
                "indexName": self.index(),
                "params": self.params(),
            }]
        })
    }
}

/// A suggestions query against the query-suggestions index.
#[derive(Debug, Serialize)]
pub struct Suggest {
    term: String,
    limit: u64,
}

impl Suggest {
    pub fn new(term: impl Into<String>, limit: u64) -> Suggest {
        Suggest {
            term: term.into(),
            limit,
        }
    }

    pub fn body(&self) -> serde_json::Value {
        serde_json::json!({
            "requests": [{
                "indexName": format!("{PRODUCT_INDEX}_query_suggestions"),
                "params": format!(
                    "query={}&hitsPerPage={}",
                    encode(&self.term),
                    self.limit
                ),
            }]
        })
    }
}

/// Percent-encoding for a form value. `url::form_urlencoded` would pull the
/// whole serialiser in for two call sites.
fn encode(value: &str) -> String {
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

    fn params_of(query: &Query) -> std::collections::HashMap<String, String> {
        query
            .params()
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .map(|(k, v)| (decode(k), decode(v)))
            .collect()
    }

    fn decode(value: &str) -> String {
        let bytes = value.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).expect("ascii");
                out.push(u8::from_str_radix(hex, 16).expect("hex"));
                i += 3;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        String::from_utf8(out).expect("utf8")
    }

    #[test]
    fn browsing_filters_the_level_the_code_belongs_to() {
        // The site's own query for this category was
        // `categoryID.lvl2:RF7336`. Filtering the wrong level matches nothing
        // and reads as an empty category rather than a bug.
        let p = params_of(&Query::category("RF7336"));
        assert_eq!(
            p["filters"], "online:true AND categoryID.lvl2:RF7336",
            "a fineline is lvl2"
        );
        assert_eq!(p["query"], "", "browsing is an empty term, not the name");

        let p = params_of(&Query::category("RS2158"));
        assert_eq!(p["filters"], "online:true AND categoryID.lvl1:RS2158");
    }

    #[test]
    fn a_category_with_no_level_filters_nothing_rather_than_guessing() {
        // `N2` is a landing page, not a catalogue category. Guessing lvl0 here
        // would answer with an unrelated department's products.
        let q = Query::category("N2");
        assert_eq!(q.category_level(), None);
        assert_eq!(params_of(&q)["filters"], "online:true");
    }

    #[test]
    fn availability_becomes_the_or_clause_the_site_sends() {
        let q = Query::search("paint").with_availability(Availability {
            collect_from: Some("66".into()),
            deliver_to: Some("5".into()),
        });
        assert_eq!(
            params_of(&q)["filters"],
            "online:true AND (clickAndCollect:66 OR homeDelivery:5 OR expressDelivery:5)"
        );
    }

    #[test]
    fn each_facet_is_its_own_group_so_they_and_together() {
        // One group of two would be an OR: "Nouveau or white" rather than
        // "Nouveau and white", which is not what the sidebar does.
        let q = Query::category("RF7336")
            .with_brand(Some("Nouveau".into()))
            .in_stock_at(Some("66".into()));
        let filters: Vec<Vec<String>> =
            serde_json::from_str(&params_of(&q)["facetFilters"]).expect("json");
        assert_eq!(
            filters,
            vec![vec!["brandName:Nouveau"], vec!["storesWithStock:66"]]
        );
    }

    #[test]
    fn a_price_range_becomes_numeric_filters_on_the_sort_price() {
        let q = Query::category("RF7336").with_price(Some(13.0), Some(19.0));
        let numeric: Vec<String> =
            serde_json::from_str(&params_of(&q)["numericFilters"]).expect("json");
        assert_eq!(numeric, ["prices.sortPrice>=13", "prices.sortPrice<=19"]);

        let q = Query::category("RF7336").with_price(None, None);
        assert!(!q.params().contains("numericFilters"), "omitted when unset");
    }

    #[test]
    fn a_sort_name_becomes_a_replica_index_and_relevance_is_the_base() {
        assert_eq!(
            Sort::parse("relevance").index(),
            "retail_products_relevance"
        );
        assert_eq!(Sort::parse("").index(), "retail_products_relevance");
        assert_eq!(
            Sort::parse("price-asc").index(),
            "retail_products_price_asc",
            "a bare word is a replica suffix"
        );
        assert_eq!(
            Sort::parse("retail_articles_relevance").index(),
            "retail_articles_relevance",
            "a full index name is passed through"
        );
    }

    #[test]
    fn a_term_with_punctuation_survives_encoding() {
        let q = Query::search("10L Wattyl & fence #1");
        assert_eq!(params_of(&q)["query"], "10L Wattyl & fence #1");
    }

    #[test]
    fn the_body_names_one_index_and_carries_the_params() {
        let body = Query::search("paint").with_page(2).body();
        let request = &body["requests"][0];
        assert_eq!(request["indexName"], "retail_products_relevance");
        let params = request["params"].as_str().expect("a string");
        assert!(params.contains("page=2"), "{params}");
        assert!(params.contains("hitsPerPage=24"), "{params}");
    }
}
