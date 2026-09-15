//! The stores.

use std::io::{self, Write};

use cli_kit::{table, Out, View};
use mitre10_api::Store;
use serde::Serialize;

#[derive(Serialize)]
pub struct StoreList<'a> {
    pub stores: &'a [Store],
}

impl View for StoreList<'_> {
    fn text(&self, out: &mut Out) -> io::Result<()> {
        if self.stores.is_empty() {
            return writeln!(out, "No stores found.");
        }
        let mut t = table(&["ID", "Store", "Where", "Phone"]);
        for s in self.stores {
            t.add_row(vec![
                s.code.clone(),
                s.name.clone(),
                super::or_dash(
                    s.address
                        .as_ref()
                        .and_then(|a| a.town.as_deref().or(a.suburb.as_deref())),
                ),
                super::or_dash(s.phone.as_deref()),
            ]);
        }
        writeln!(out, "{t}")?;
        super::write_count(
            out,
            self.stores.len(),
            "store",
            Some("Select one: `mitre10 store set <id>`."),
        )
    }
}

#[derive(Serialize)]
pub struct StoreView<'a> {
    pub store: &'a Store,
}

impl View for StoreView<'_> {
    fn text(&self, out: &mut Out) -> io::Result<()> {
        let s = self.store;
        writeln!(out, "{}", out.heading(&s.name))?;

        // The same headless two-column table the product page uses, and the
        // other tools' store pages: one layout for "the facts about a thing".
        let mut t = table(&["", ""]);
        t.add_row(vec!["ID".to_string(), s.code.clone()]);
        if let Some(address) = s.address.as_ref().and_then(|a| a.formatted.as_deref()) {
            t.add_row(vec!["Address".to_string(), address.to_string()]);
        }
        for (label, value) in [("Phone", s.phone.as_deref()), ("Email", s.email.as_deref())] {
            if let Some(value) = value {
                t.add_row(vec![label.to_string(), value.to_string()]);
            }
        }
        if let Some(hours) = &s.today_hours {
            t.add_row(vec!["Today".to_string(), hours.clone()]);
        }
        t.add_row(vec![
            "Express delivery".to_string(),
            super::yes_no(s.express_delivery),
        ]);
        writeln!(out, "{t}")
    }
}
