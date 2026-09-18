//! Which stores have a product.

use std::io::{self, Write};

use cli_kit::{table, Out, View};
use farmers_api::StoreStock;
use serde::Serialize;

use super::{or_dash, write_count};

#[derive(Serialize)]
pub struct StockList {
    pub sku: String,
    /// Region code and its stores, in the order they were asked for.
    pub regions: Vec<(String, Vec<StoreStock>)>,
    /// Whether only the stores that have it are being shown, so the summary
    /// does not read as "nowhere has it".
    #[serde(skip)]
    pub filtered: bool,
}

impl StockList {
    fn stores(&self) -> usize {
        self.regions.iter().map(|(_, s)| s.len()).sum()
    }

    fn available(&self) -> usize {
        self.regions
            .iter()
            .flat_map(|(_, s)| s)
            .filter(|s| s.available())
            .count()
    }
}

impl View for StockList {
    fn text(&self, out: &mut Out) -> io::Result<()> {
        if self.stores() == 0 {
            return writeln!(
                out,
                "No store answered for {}{}.",
                self.sku,
                match self.filtered {
                    true => " with any in stock",
                    false => "",
                }
            );
        }

        let mut t = table(&["Region", "Store", "Stock", "Address", "Phone"]);
        for (code, stores) in &self.regions {
            for store in stores {
                t.add_row(vec![
                    // The name, not the code: `MWT` means nothing to a reader,
                    // and the code is only ever an input.
                    farmers_api::region_name(code).unwrap_or(code).to_string(),
                    store.store.name.clone(),
                    store.status.clone(),
                    or_dash(store.store.address.as_deref()),
                    or_dash(store.store.phone.as_deref()),
                ]);
            }
        }
        writeln!(out, "{t}")?;

        if self.filtered {
            return write_count(out, self.stores(), "store", Some("All have it."));
        }
        write_count(
            out,
            self.stores(),
            "store",
            Some(&format!("{} have it.", self.available())),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cli_kit::{emit, Format};
    use farmers_api::Store;

    fn render(view: &StockList) -> String {
        let mut out = Out::buffer(Format::Text);
        emit(&mut out, view).expect("writes");
        out.into_string()
    }

    fn stock(name: &str, status: &str) -> StoreStock {
        StoreStock {
            store: Store {
                name: name.into(),
                address: Some("309 Broadway".into()),
                ..Default::default()
            },
            status: status.into(),
        }
    }

    #[test]
    fn a_region_is_shown_by_name_because_the_code_is_only_ever_an_input() {
        let text = render(&StockList {
            sku: "6867065002".into(),
            regions: vec![("MWT".into(), vec![stock("Palmerston North", "In Stock")])],
            filtered: false,
        });
        assert!(text.contains("Manawatū-Whanganui"), "{text}");
        assert!(!text.contains("MWT"), "{text}");
    }

    #[test]
    fn the_summary_counts_the_stores_that_have_it() {
        let text = render(&StockList {
            sku: "6867065002".into(),
            regions: vec![(
                "AUK".into(),
                vec![
                    stock("Newmarket", "In Stock"),
                    stock("Albany", "Not In Stock"),
                ],
            )],
            filtered: false,
        });
        assert!(text.contains("2 stores. 1 have it."), "{text}");
    }

    #[test]
    fn a_filtered_list_does_not_claim_the_others_do_not_exist() {
        // With `--available` the rows are already only the ones that have it,
        // so "3 stores. 3 have it." would be a tautology and "0 have it" a lie.
        let text = render(&StockList {
            sku: "6867065002".into(),
            regions: vec![("AUK".into(), vec![stock("Newmarket", "In Stock")])],
            filtered: true,
        });
        assert!(text.contains("All have it."), "{text}");
    }

    #[test]
    fn nothing_anywhere_says_so_rather_than_drawing_an_empty_table() {
        let text = render(&StockList {
            sku: "6867065002".into(),
            regions: Vec::new(),
            filtered: true,
        });
        assert!(text.contains("with any in stock"), "{text}");
    }
}
