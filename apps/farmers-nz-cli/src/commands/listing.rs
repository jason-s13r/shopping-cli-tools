//! `search`, `browse` and `suggest` -- everything that finds products.
//!
//! All three go to Constructor.io, not to the storefront. The Intershop REST
//! API accepts a `searchTerm` and silently ignores it, so building search on
//! the "obvious" endpoint would look like it worked and return the catalogue in
//! its natural order.

use cli_kit::emit;
use farmers_api::Query;

use crate::app::App;
use crate::cli::Listing as Flags;
use crate::error::{AppError, AppResult};
use crate::views::{ListingView, Suggestions};

pub async fn search(app: &App, term: &str, flags: Flags) -> AppResult<()> {
    let query = query(app, &flags)?;
    // `find` rather than `search`: a term a merchandising rule claims comes
    // back with no results and a redirect, and reporting "nothing found" for
    // `lego` would be wrong in the most visible way available.
    let listing = app.client()?.find(term, &query).await?;
    show(
        app,
        listing,
        &flags,
        Some(format!(
            "Next page: `farmers search {term:?} --page {}`.",
            flags.page + 1
        )),
    )
    .await
}

pub async fn browse(app: &App, category: &str, flags: Flags) -> AppResult<()> {
    let query = query(app, &flags)?;
    let listing = app.client()?.browse(category, &query).await?;
    show(
        app,
        listing,
        &flags,
        Some(format!(
            "Next page: `farmers browse {category} --page {}`.",
            flags.page + 1
        )),
    )
    .await
}

async fn show(
    app: &App,
    listing: farmers_api::Listing,
    flags: &Flags,
    next: Option<String>,
) -> AppResult<()> {
    // One request per product, so it is asked for rather than assumed -- and
    // against this host that matters more than the time it costs.
    let priced = match flags.prices {
        false => Vec::new(),
        true => app
            .client()?
            .price_hits(&listing.hits)
            .await
            .into_iter()
            .map(|(_, product)| product)
            .collect(),
    };

    let mut out = app.out();
    emit(
        &mut out,
        &ListingView {
            // Only offer a next page when there is one.
            next: next.filter(|_| {
                let shown = (flags.page + 1) * listing.hits.len().max(1) as u64;
                (shown as i64) < listing.total
            }),
            listing,
            priced,
            show_facets: flags.facets,
        },
    )?;
    Ok(())
}

pub async fn suggest(app: &App, term: &str, limit: usize) -> AppResult<()> {
    let mut suggestions = app.client()?.suggest(term).await?;
    suggestions.truncate(limit);
    let mut out = app.out();
    emit(
        &mut out,
        &Suggestions {
            term: term.to_string(),
            suggestions,
        },
    )?;
    Ok(())
}

/// The flags, as the search backend takes them.
fn query(app: &App, flags: &Flags) -> AppResult<Query> {
    if let Some(name) = &flags.sort {
        // Refused here rather than dropped silently by the library: the
        // service ignores a sort it does not know, so a typo would look like
        // the flag quietly not working.
        if farmers_api::search::sort(name).is_none() {
            let names: Vec<&str> = farmers_api::SORTS.iter().map(|(n, _, _)| *n).collect();
            return Err(AppError::usage(format!(
                "{name:?} is not a sort; use one of {}",
                names.join(", ")
            )));
        }
    }

    let mut query = Query::new()
        .page(flags.page)
        .per_page(app.page_size(flags.limit))
        .sorted_by(flags.sort.clone());

    // The facet names the index actually uses, which are not the words anyone
    // would guess. `--filter` reaches the rest.
    if let Some(brand) = &flags.brand {
        query = query.filter("manufacturername", brand);
    }
    if let Some(colour) = &flags.colour {
        query = query.filter("colourfamilydisplayname", colour);
    }
    if let Some(size) = &flags.size {
        query = query.filter("size-displayname", size);
    }
    if flags.in_stock {
        query = query.filter("stockstatus", "In stock");
    }
    for raw in &flags.filter {
        let (name, value) = raw.split_once('=').ok_or_else(|| {
            AppError::usage(format!(
                "{raw:?} is not a filter; they are written name=value, and `--facets` \
                 lists the names a search offered"
            ))
        })?;
        query = query.filter(name.trim(), value.trim());
    }
    Ok(query)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The flags a bare `search` produces.
    fn flags() -> Flags {
        Flags::default()
    }

    fn app() -> App {
        // `App::new` only reads config and env, so this needs no network and
        // no fixtures.
        let cli = <crate::cli::Cli as clap::Parser>::parse_from(["farmers", "regions"]);
        App::new(&cli).expect("an app")
    }

    #[test]
    fn a_misspelled_sort_is_refused_rather_than_silently_ignored() {
        // The service drops a sort it does not know and answers in relevance
        // order, so passing it through would look like the flag not working.
        let e = query(
            &app(),
            &Flags {
                sort: Some("cheapest".into()),
                ..flags()
            },
        )
        .expect_err("not a sort");
        assert!(e.to_string().contains("price-desc"), "{e}");
    }

    #[test]
    fn every_documented_sort_is_accepted() {
        for (name, _, _) in farmers_api::SORTS {
            assert!(
                query(
                    &app(),
                    &Flags {
                        sort: Some(name.to_string()),
                        ..flags()
                    }
                )
                .is_ok(),
                "{name}"
            );
        }
    }

    #[test]
    fn a_filter_without_an_equals_sign_says_how_to_write_one() {
        let e = query(
            &app(),
            &Flags {
                filter: vec!["brand Chisel".into()],
                ..flags()
            },
        )
        .expect_err("malformed");
        assert!(e.to_string().contains("name=value"), "{e}");
    }

    #[test]
    fn the_named_flags_become_the_facet_names_the_index_actually_uses() {
        // `colour` is `colourfamilydisplayname` in the index, which nobody
        // would guess and which is exactly why the flag exists.
        let q = query(
            &app(),
            &Flags {
                brand: Some("Chisel".into()),
                colour: Some("Grey".into()),
                in_stock: true,
                ..flags()
            },
        )
        .expect("builds");
        let names: Vec<&str> = q.filters.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"manufacturername"), "{names:?}");
        assert!(names.contains(&"colourfamilydisplayname"), "{names:?}");
        assert!(names.contains(&"stockstatus"), "{names:?}");
    }
}
