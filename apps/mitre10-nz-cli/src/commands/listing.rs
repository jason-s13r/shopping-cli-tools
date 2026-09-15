//! `search`, `browse` and `suggest`, which are one request to the search index.
//!
//! Both listings go to Algolia rather than to OCC, because that is what the
//! website does: every product grid on the site is an Algolia query, so
//! results here are the ones it would show, in the order it would show them.
//! OCC's catalogue endpoints answer one product at a time and know nothing
//! about the facets, so a listing built on them would quietly disagree.

use cli_kit::emit;
use mitre10_api::{Availability, CategoryNode, Query, Sort};

use crate::app::App;
use crate::cli::Listing;
use crate::error::{AppError, AppResult};
use crate::views::{ProductList, Suggestions};

pub async fn search(app: &App, term: String, flags: Listing) -> AppResult<()> {
    run(app, Query::search(term), flags).await
}

pub async fn browse(app: &App, category: String, flags: Listing) -> AppResult<()> {
    // Refused here rather than sent: a landing-page code filters nothing, so
    // the index would answer with the whole catalogue and call it a category.
    if CategoryNode::level_of(&category).is_none() {
        return Err(AppError::usage(format!(
            "{category:?} is not a browsable category code; the browsable ones start RD, RS, RF \
             or RC, and `mitre10 categories` lists them"
        )));
    }
    run(app, Query::category(category), flags).await
}

async fn run(app: &App, query: Query, flags: Listing) -> AppResult<()> {
    let client = app.client()?;

    // A listing with no availability filter answers with products no store can
    // supply, which is not what the website shows.
    let mut availability = Availability::default();
    if let Some(store) = app.store(flags.store.clone()) {
        availability.collect_from = Some(store);
    }
    if let Some(postcode) = app.postcode(flags.postcode.clone()) {
        availability.deliver_to = Some(client.postcode_group(&postcode).await?);
    }

    let query = query
        .with_page(flags.page)
        .with_page_size(flags.limit)
        .with_availability(availability)
        .with_brand(flags.brand)
        .with_colour(flags.colour)
        .with_size(flags.size)
        .with_price(flags.min, flags.max)
        .with_sort(flags.sort.as_deref().map(Sort::parse).unwrap_or_default());

    let listing = client.listing(&query).await?;
    let mut out = app.out();
    emit(&mut out, &ProductList::new(&listing, flags.facets))?;
    Ok(())
}

pub async fn suggest(app: &App, term: &str, limit: u64) -> AppResult<()> {
    let suggestions = app.client()?.suggestions(term, limit).await?;
    let mut out = app.out();
    emit(
        &mut out,
        &Suggestions {
            term,
            suggestions: &suggestions,
        },
    )?;
    Ok(())
}
