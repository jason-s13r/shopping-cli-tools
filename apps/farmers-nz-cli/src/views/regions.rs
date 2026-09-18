//! The regions `stock` can be narrowed to.

use std::io::{self, Write};

use cli_kit::{table, Out, View};
use serde::Serialize;

use super::write_count;

#[derive(Serialize)]
pub struct RegionList {
    pub regions: Vec<Region>,
    /// Which one a bare `stock` would use, when one is configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected: Option<String>,
}

#[derive(Serialize)]
pub struct Region {
    pub code: String,
    pub name: String,
}

impl RegionList {
    /// Every region the store finder knows, in the order it offers them.
    pub fn all(selected: Option<String>) -> RegionList {
        RegionList {
            regions: farmers_api::REGIONS
                .iter()
                .map(|(code, name)| Region {
                    code: (*code).to_string(),
                    name: (*name).to_string(),
                })
                .collect(),
            selected,
        }
    }
}

impl View for RegionList {
    fn text(&self, out: &mut Out) -> io::Result<()> {
        let mut t = table(&["Code", "Region"]);
        for region in &self.regions {
            let current = self.selected.as_deref() == Some(region.code.as_str());
            t.add_row(vec![
                match current {
                    true => format!("{} *", region.code),
                    false => region.code.clone(),
                },
                region.name.clone(),
            ]);
        }
        writeln!(out, "{t}")?;
        // Said here rather than left to be discovered: the list is short and
        // three of New Zealand's regions are missing from it.
        writeln!(
            out,
            "{}",
            out.dim("Tasman, Marlborough and the West Coast have no Farmers store.")
        )?;
        write_count(
            out,
            self.regions.len(),
            "region",
            self.selected
                .as_ref()
                .map(|_| "* is the configured one.")
                .or(Some("Set one with `farmers config set region <name>`.")),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cli_kit::{emit, Format};

    fn render(view: &RegionList) -> String {
        let mut out = Out::buffer(Format::Text);
        emit(&mut out, view).expect("writes");
        out.into_string()
    }

    #[test]
    fn every_region_the_store_finder_knows_is_listed() {
        let text = render(&RegionList::all(None));
        assert!(text.contains("13 regions."), "{text}");
        assert!(text.contains("Manawatū-Whanganui"), "{text}");
    }

    #[test]
    fn the_missing_regions_are_explained_rather_than_left_to_be_noticed() {
        // Three of New Zealand's regions are absent, which reads as a bug in
        // this tool unless it says why.
        let text = render(&RegionList::all(None));
        assert!(text.contains("no Farmers store"), "{text}");
        assert!(text.contains("config set region"), "{text}");
    }

    #[test]
    fn the_configured_region_is_marked_and_the_hint_changes() {
        let text = render(&RegionList::all(Some("CAN".into())));
        assert!(text.contains("CAN *"), "{text}");
        assert!(text.contains("* is the configured one."), "{text}");
        assert!(!text.contains("Set one with"), "{text}");
    }
}
