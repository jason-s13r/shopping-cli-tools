//! Finding things, which is not the Intershop API's job.
//!
//! **The REST API cannot search.** `/products?searchTerm=lego` does not error
//! and does not filter -- it answers `total: 10000` and returns the catalogue in
//! its natural order. That silent-wrong-answer is the reason this module
//! exists, and the reason nothing in this crate ever sends `searchTerm`.
//!
//! Search, autocomplete and category browse are Constructor.io: a different
//! company on a different host, with no bot manager in front of it and no
//! warm-up needed. Its results carry stock status, brand and facets but **no
//! price at all**, so anything that shows money joins the hits back to
//! [`crate::Client::product`] by SKU.
//!
//! Browse takes the same category ids Intershop uses -- `51-0303` -- plus
//! `Brands/{name}` for a brand's own page, so a [`crate::domain::Category`] out
//! of the tree can be handed straight to [`Query::browse`].

use crate::endpoints::{encode, query_string, Endpoints};

/// What one page holds when nothing says otherwise. The storefront's own grid
/// asks for this many.
pub const PAGE_SIZE: u64 = 24;

/// The orderings the index offers, as `(name, what it sorts by, which way)`.
///
/// Read off the live `sort_options` rather than invented: a name the index does
/// not know is ignored rather than refused, so a typo would silently return
/// relevance order.
pub const SORTS: [(&str, &str, SortOrder); 6] = [
    ("relevance", "relevance", SortOrder::Descending),
    ("newest", "arrivaldate", SortOrder::Descending),
    ("name", "name", SortOrder::Ascending),
    ("name-desc", "name", SortOrder::Descending),
    ("price", "productsalepricegross", SortOrder::Ascending),
    ("price-desc", "productsalepricegross", SortOrder::Descending),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortOrder {
    Ascending,
    Descending,
}

impl SortOrder {
    fn as_str(self) -> &'static str {
        match self {
            SortOrder::Ascending => "ascending",
            SortOrder::Descending => "descending",
        }
    }
}

/// A sort by the name this crate gives it.
pub fn sort(name: &str) -> Option<(&'static str, SortOrder)> {
    let wanted = name.trim().to_lowercase();
    SORTS
        .iter()
        .find(|(n, _, _)| *n == wanted)
        .map(|(_, by, order)| (*by, *order))
}

/// What is being asked for: a term, or a category.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Subject {
    Term(String),
    /// A category id, or `Brands/Chisel`.
    Group(String),
}

/// One page of results, however it is being narrowed.
#[derive(Clone, Debug, Default)]
pub struct Query {
    /// Zero-based, as this crate counts everywhere.
    pub page: u64,
    pub per_page: Option<u64>,
    /// Facet name and value, as `facets()` on a previous answer names them.
    /// Several with the same name are an OR, which is what the site's own grid
    /// does with a row of checkboxes.
    pub filters: Vec<(String, String)>,
    pub sort: Option<String>,
}

impl Query {
    pub fn new() -> Query {
        Query::default()
    }

    pub fn page(mut self, page: u64) -> Query {
        self.page = page;
        self
    }

    pub fn per_page(mut self, per_page: Option<u64>) -> Query {
        self.per_page = per_page;
        self
    }

    pub fn filter(mut self, name: impl Into<String>, value: impl Into<String>) -> Query {
        self.filters.push((name.into(), value.into()));
        self
    }

    pub fn sorted_by(mut self, sort: Option<String>) -> Query {
        self.sort = sort;
        self
    }

    /// The URL for a keyword search.
    pub fn search(&self, endpoints: &Endpoints, term: &str, client: &str, session: u64) -> String {
        self.url(endpoints, &Subject::Term(term.to_string()), client, session)
    }

    /// The URL for a category or brand listing.
    pub fn browse(&self, endpoints: &Endpoints, group: &str, client: &str, session: u64) -> String {
        self.url(
            endpoints,
            &Subject::Group(group.to_string()),
            client,
            session,
        )
    }

    fn url(&self, endpoints: &Endpoints, subject: &Subject, client: &str, session: u64) -> String {
        let path = match subject {
            Subject::Term(term) => format!("/search/{}", encode(term.trim())),
            // Not encoded as one component: a brand page's id is
            // `Brands/Chisel`, and the slash is a path separator the service
            // expects to see.
            Subject::Group(group) => format!(
                "/browse/group_id/{}",
                group
                    .trim_matches('/')
                    .split('/')
                    .map(encode)
                    .collect::<Vec<_>>()
                    .join("/")
            ),
        };

        let mut params: Vec<(String, String)> = vec![
            (
                "num_results_per_page".into(),
                self.per_page.unwrap_or(PAGE_SIZE).to_string(),
            ),
            // The service counts pages from one. Everything above this counts
            // from zero, so the conversion lives here and nowhere else.
            ("page".into(), (self.page + 1).to_string()),
        ];
        for (name, value) in &self.filters {
            params.push((format!("filters[{name}]"), value.clone()));
        }
        if let Some(name) = &self.sort {
            // An unknown name is dropped rather than passed through: the
            // service ignores one it does not know, which would look like the
            // sort silently not working.
            if let Some((by, order)) = sort(name) {
                params.push(("sort_by".into(), by.to_string()));
                params.push(("sort_order".into(), order.as_str().to_string()));
            }
        }

        format!(
            "{}&{}",
            endpoints.constructor(&path, client, session),
            query_string(&params)
        )
    }
}

/// The URL for what the site would suggest as someone types.
pub fn autocomplete(endpoints: &Endpoints, term: &str, client: &str, session: u64) -> String {
    endpoints.constructor(
        &format!("/autocomplete/{}", encode(term.trim())),
        client,
        session,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoints() -> Endpoints {
        Endpoints::defaults()
    }

    #[test]
    fn a_search_url_counts_pages_from_one_although_this_crate_counts_from_zero() {
        // Off by one here is a whole page of results silently skipped or
        // repeated, and nothing in the answer would say so.
        let url = Query::new().page(0).search(&endpoints(), "robe", "c", 1);
        assert!(url.contains("&page=1"), "{url}");
        let url = Query::new().page(2).search(&endpoints(), "robe", "c", 1);
        assert!(url.contains("&page=3"), "{url}");
    }

    #[test]
    fn a_term_with_a_space_is_encoded_into_the_path() {
        let url = Query::new().search(&endpoints(), "blue shirt", "c", 1);
        assert!(url.contains("/search/blue%20shirt?"), "{url}");
    }

    #[test]
    fn a_brands_group_id_keeps_the_slash_that_makes_it_one() {
        // `Brands/Chisel` is two path segments. Encoding it whole gives
        // `Brands%2FChisel`, which the service answers with an empty listing
        // rather than an error.
        let url = Query::new().browse(&endpoints(), "Brands/Chisel", "c", 1);
        assert!(url.contains("/browse/group_id/Brands/Chisel?"), "{url}");
    }

    #[test]
    fn a_category_id_browses_by_the_same_id_intershop_uses() {
        let url = Query::new().browse(&endpoints(), "51-0303", "c", 1);
        assert!(url.contains("/browse/group_id/51-0303?"), "{url}");
    }

    #[test]
    fn filters_are_bracketed_the_way_the_service_reads_them() {
        let url = Query::new()
            .filter("manufacturername", "Karen Walker")
            .filter("stockstatus", "In stock")
            .search(&endpoints(), "coat", "c", 1);
        assert!(
            url.contains("filters%5Bmanufacturername%5D=Karen%20Walker"),
            "{url}"
        );
        assert!(url.contains("filters%5Bstockstatus%5D=In%20stock"), "{url}");
    }

    #[test]
    fn a_known_sort_is_sent_and_an_unknown_one_is_dropped() {
        // The service ignores a name it does not know, so passing a typo
        // through would look like the sort quietly not working.
        let url = Query::new()
            .sorted_by(Some("price".into()))
            .search(&endpoints(), "robe", "c", 1);
        assert!(url.contains("sort_by=productsalepricegross"), "{url}");
        assert!(url.contains("sort_order=ascending"), "{url}");

        let url =
            Query::new()
                .sorted_by(Some("cheapest".into()))
                .search(&endpoints(), "robe", "c", 1);
        assert!(!url.contains("sort_by"), "{url}");
    }

    #[test]
    fn the_two_directions_of_one_sort_are_told_apart_by_name() {
        assert_eq!(
            sort("price"),
            Some(("productsalepricegross", SortOrder::Ascending))
        );
        assert_eq!(
            sort("price-desc"),
            Some(("productsalepricegross", SortOrder::Descending))
        );
        assert_eq!(
            sort("PRICE"),
            Some(("productsalepricegross", SortOrder::Ascending))
        );
        assert_eq!(sort("nonsense"), None);
    }

    #[test]
    fn every_call_carries_the_credentials_the_service_requires() {
        // It answers 400 without them, which reads as a malformed query.
        let url = Query::new().search(&endpoints(), "robe", "a-client", 3);
        assert!(url.contains("key=key_wafe8wrKCXfPyWKw"), "{url}");
        assert!(url.contains("i=a-client"), "{url}");
        assert!(url.contains("s=3"), "{url}");

        let url = autocomplete(&endpoints(), "rob", "a-client", 3);
        assert!(
            url.starts_with("https://ac.cnstrc.com/autocomplete/rob?"),
            "{url}"
        );
        assert!(url.contains("i=a-client"), "{url}");
    }
}
