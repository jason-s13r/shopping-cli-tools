//! A page of search or browse results.

use std::io::{self, Write};

use cli_kit::{table, Out, View};
use mitre10_api::Listing;
use serde::Serialize;

#[derive(Serialize)]
pub struct ProductList<'a> {
    pub listing: &'a Listing,
    /// Whether to print the facets the index offered.
    #[serde(skip)]
    pub facets: bool,
}

impl<'a> ProductList<'a> {
    pub fn new(listing: &'a Listing, facets: bool) -> ProductList<'a> {
        ProductList { listing, facets }
    }
}

impl View for ProductList<'_> {
    fn text(&self, out: &mut Out) -> io::Result<()> {
        let l = self.listing;
        if l.products.is_empty() {
            return writeln!(out, "Nothing found.");
        }
        let mut t = table(&["Code", "Product", "Brand", "Price", "Size"]);
        for p in &l.products {
            t.add_row(vec![
                p.code.clone(),
                p.name.clone(),
                super::or_dash(p.brand.as_deref()),
                super::price_label(p.price, p.was_price),
                super::or_dash(p.size.as_deref()),
            ]);
        }
        writeln!(out, "{t}")?;

        if self.facets && !l.facets.is_empty() {
            writeln!(out)?;
            for facet in &l.facets {
                let options: Vec<String> = facet
                    .options
                    .iter()
                    .take(8)
                    .map(|o| format!("{} ({})", o.value, o.count))
                    .collect();
                if !options.is_empty() {
                    writeln!(out, "{}: {}", out.heading(&facet.name), options.join(", "))?;
                }
            }
            writeln!(out)?;
        }

        // The window, not just the count: a listing is one page of many and
        // saying "24 products" of 173 would be a lie by omission.
        let shown = l.products.len() as u64;
        let first = l.offset() + 1;
        writeln!(
            out,
            "{first}–{} of {}.{}",
            l.offset() + shown,
            l.total,
            if l.has_more() {
                format!(" Next: --page {}", l.page + 1)
            } else {
                String::new()
            }
        )
    }
}

/// What the site would suggest for a partial term.
#[derive(Serialize)]
pub struct Suggestions<'a> {
    pub term: &'a str,
    pub suggestions: &'a [String],
}

impl View for Suggestions<'_> {
    fn text(&self, out: &mut Out) -> io::Result<()> {
        if self.suggestions.is_empty() {
            return writeln!(out, "Nothing suggested for {:?}.", self.term);
        }
        for suggestion in self.suggestions {
            writeln!(out, "{suggestion}")?;
        }
        Ok(())
    }
}
