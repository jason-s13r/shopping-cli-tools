//! Listings: search results, browse results, suggestions and a price list.

use std::io::{self, Write};

use cli_kit::{table, Out, View};
use farmers_api::{Facet, Listing, Product, Suggestion};
use serde::Serialize;

use super::{or_dash, price_label, write_count};

/// A page of search or browse results.
#[derive(Serialize)]
pub struct ListingView {
    pub listing: Listing,
    /// Prices, when `--prices` asked for them. A separate field rather than a
    /// richer `Hit`, because the index genuinely has no price in it and a
    /// type that implied otherwise would print a dash for every row of the
    /// ordinary, cheap path.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub priced: Vec<Option<Product>>,
    /// Whether to print the facets alongside.
    #[serde(skip)]
    pub show_facets: bool,
    /// What to suggest running next, spelled for this binary.
    #[serde(skip)]
    pub next: Option<String>,
}

impl ListingView {
    fn price_of(&self, i: usize) -> Option<&Product> {
        self.priced.get(i).and_then(Option::as_ref)
    }
}

impl View for ListingView {
    fn text(&self, out: &mut Out) -> io::Result<()> {
        if let Some(redirect) = &self.listing.redirect {
            // Said before the results, because it explains why they are not
            // what was asked for.
            writeln!(
                out,
                "{}",
                out.dim(&format!("The site redirects this term to {redirect}."))
            )?;
        }
        if self.listing.hits.is_empty() {
            return writeln!(out, "Nothing found.");
        }

        let priced = !self.priced.is_empty();
        let mut headers = vec!["Code", "Product", "Brand", "Stock"];
        if priced {
            headers.insert(3, "Price");
        }
        let mut t = table(&headers);
        for (i, hit) in self.listing.hits.iter().enumerate() {
            let mut row = vec![
                hit.sku.clone(),
                hit.name.clone(),
                or_dash(hit.brand.as_deref()),
                or_dash(hit.stock_status.as_deref()),
            ];
            if priced {
                row.insert(
                    3,
                    self.price_of(i)
                        .map(price_label)
                        .unwrap_or_else(|| "—".into()),
                );
            }
            t.add_row(row);
        }
        writeln!(out, "{t}")?;

        // The page, not the total, and both are said: a listing that shows 24
        // of 1,200 with no total reads as the whole answer.
        writeln!(
            out,
            "{}",
            out.dim(&format!(
                "Showing {} of {} on page {}.",
                self.listing.hits.len(),
                self.listing.total,
                self.listing.page
            ))
        )?;
        if self.show_facets {
            write_facets(out, &self.listing.facets)?;
        }
        if let Some(next) = &self.next {
            writeln!(out, "{}", out.dim(next))?;
        }
        Ok(())
    }
}

fn write_facets(out: &mut Out, facets: &[Facet]) -> io::Result<()> {
    if facets.is_empty() {
        return Ok(());
    }
    writeln!(out)?;
    let mut t = table(&["Filter", "Values"]);
    for facet in facets {
        // Truncated on purpose: `size-displayname` alone runs to dozens, and
        // a table cell holding all of them is unreadable and pushes every
        // other column to nothing.
        let shown: Vec<String> = facet
            .options
            .iter()
            .take(8)
            .map(|o| format!("{} ({})", o.display_name, o.count))
            .collect();
        let more = facet.options.len().saturating_sub(shown.len());
        let mut values = shown.join(", ");
        if more > 0 {
            values.push_str(&format!(", +{more} more"));
        }
        t.add_row(vec![facet.name.clone(), values]);
    }
    writeln!(out, "{t}")?;
    writeln!(
        out,
        "{}",
        out.dim("Narrow with --filter name=value, using the names in the first column.")
    )
}

/// What the site would offer as someone types.
#[derive(Serialize)]
pub struct Suggestions {
    pub term: String,
    pub suggestions: Vec<Suggestion>,
}

impl View for Suggestions {
    fn text(&self, out: &mut Out) -> io::Result<()> {
        if self.suggestions.is_empty() {
            return writeln!(out, "Nothing suggested for {:?}.", self.term);
        }
        let mut t = table(&["Section", "Suggestion"]);
        for s in &self.suggestions {
            t.add_row(vec![s.section.clone(), s.value.clone()]);
        }
        writeln!(out, "{t}")?;
        write_count(out, self.suggestions.len(), "suggestion", None)
    }
}

/// Several products priced at once.
#[derive(Serialize)]
pub struct PriceList {
    /// The code as it was asked for, with what came back. The code is carried
    /// even on a failure, because "which one of these forty failed" is the
    /// whole question a batch is asked.
    pub prices: Vec<(String, Result<Product, String>)>,
}

impl View for PriceList {
    fn text(&self, out: &mut Out) -> io::Result<()> {
        let mut t = table(&["Code", "Product", "Price", "Stock"]);
        for (code, result) in &self.prices {
            match result {
                Ok(p) => t.add_row(vec![
                    code.clone(),
                    or_dash(p.name.as_deref()),
                    price_label(p),
                    p.available_stock
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "—".into()),
                ]),
                Err(e) => t.add_row(vec![code.clone(), e.clone(), "—".into(), "—".into()]),
            };
        }
        writeln!(out, "{t}")?;
        let failed = self.prices.iter().filter(|(_, r)| r.is_err()).count();
        write_count(out, self.prices.len(), "code", None)?;
        if failed > 0 {
            writeln!(out, "{}", out.warn(&format!("{failed} could not be read.")))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cli_kit::{emit, Format};
    use farmers_api::{FacetOption, Hit, Money};

    fn render<V: View>(view: &V) -> String {
        let mut out = Out::buffer(Format::Text);
        emit(&mut out, view).expect("writes");
        out.into_string()
    }

    fn hit(sku: &str, name: &str) -> Hit {
        Hit {
            sku: sku.into(),
            name: name.into(),
            brand: Some("Chisel".into()),
            stock_status: Some("In stock".into()),
            ..Default::default()
        }
    }

    fn listing() -> Listing {
        Listing {
            hits: vec![hit("6867065", "Chisel Fleece Robe")],
            total: 20,
            page: 0,
            ..Default::default()
        }
    }

    #[test]
    fn a_listing_says_how_many_it_is_showing_of_how_many_there_are() {
        // 24 rows with no total reads as the whole answer, and the next page
        // is then invisible.
        let text = render(&ListingView {
            listing: listing(),
            priced: Vec::new(),
            show_facets: false,
            next: None,
        });
        assert!(text.contains("Showing 1 of 20 on page 0."), "{text}");
        assert!(!text.contains("Price"), "no price column unless asked");
    }

    #[test]
    fn a_redirect_is_explained_before_the_results_rather_than_after() {
        // The results are a category's, not the term's; saying so afterwards
        // would be too late to be read as the reason.
        let text = render(&ListingView {
            listing: Listing {
                redirect: Some("/toys/lego-construction".into()),
                ..listing()
            },
            priced: Vec::new(),
            show_facets: false,
            next: None,
        });
        let redirect_at = text.find("redirects this term").expect("said");
        let table_at = text.find("6867065").expect("results");
        assert!(redirect_at < table_at, "{text}");
    }

    #[test]
    fn prices_appear_only_when_they_were_asked_for() {
        let priced = ListingView {
            listing: listing(),
            priced: vec![Some(Product {
                sale_price: Some(Money::new(69.99, "NZD")),
                list_price: Some(Money::new(89.99, "NZD")),
                ..Default::default()
            })],
            show_facets: false,
            next: None,
        };
        let text = render(&priced);
        assert!(text.contains("Price"), "{text}");
        assert!(text.contains("was $89.99"), "{text}");
    }

    #[test]
    fn a_long_facet_is_truncated_rather_than_wrecking_the_table() {
        // `size-displayname` runs to dozens of values, and a cell holding all
        // of them squeezes every other column to nothing.
        let options: Vec<FacetOption> = (0..20)
            .map(|i| FacetOption {
                value: i.to_string(),
                display_name: i.to_string(),
                count: 1,
            })
            .collect();
        let text = render(&ListingView {
            listing: Listing {
                facets: vec![Facet {
                    name: "size-displayname".into(),
                    display_name: "Size".into(),
                    options,
                }],
                ..listing()
            },
            priced: Vec::new(),
            show_facets: true,
            next: None,
        });
        assert!(text.contains("+12 more"), "{text}");
        assert!(text.contains("--filter name=value"), "{text}");
    }

    #[test]
    fn an_empty_listing_says_so_rather_than_drawing_an_empty_table() {
        let text = render(&ListingView {
            listing: Listing::default(),
            priced: Vec::new(),
            show_facets: false,
            next: None,
        });
        assert_eq!(text, "Nothing found.\n");
    }

    #[test]
    fn a_batch_keeps_the_code_beside_the_failure_it_caused() {
        // "which of these forty failed" is the only question a batch is asked.
        let text = render(&PriceList {
            prices: vec![
                (
                    "6867065002".into(),
                    Ok(Product {
                        name: Some("A robe".into()),
                        sale_price: Some(Money::new(69.99, "NZD")),
                        ..Default::default()
                    }),
                ),
                ("9999999".into(), Err("no product called 9999999".into())),
            ],
        });
        assert!(text.contains("9999999"), "{text}");
        assert!(text.contains("1 could not be read."), "{text}");
    }
}
