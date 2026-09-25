//! Every Warehouse store, cached.
//!
//! The finder is per region and there is no call that lists them all, so
//! answering "which store is 116?" or "where is there a Whangarei?" means
//! asking sixteen times. That is too much to spend on every lookup and far too
//! little to spend once: shops open and close on the order of once a year, so
//! the whole directory is fetched concurrently and kept for a week.
//!
//! The cache is a convenience, never a source of truth for stock — it holds
//! addresses and opening hours, both of which are stale-tolerant. Anything
//! about availability is asked live.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use twlnz_api::{Client, Store};

use crate::error::AppResult;

/// How long a fetched directory is good for. Long, because the thing it
/// describes barely changes, and a wrong answer here is an address rather than
/// a price.
const MAX_AGE_SECS: u64 = 7 * 24 * 60 * 60;

#[derive(Serialize, Deserialize)]
struct Cached {
    fetched_at: u64,
    stores: Vec<Store>,
}

fn file(paths: &net_kit::Paths) -> std::path::PathBuf {
    paths.state_file("stores.json")
}

/// Every store, from the cache or from the site.
///
/// `refresh` forces a re-fetch, for when a shop has opened since.
pub async fn all(
    client: &Arc<Client>,
    paths: &net_kit::Paths,
    refresh: bool,
) -> AppResult<Vec<Store>> {
    if !refresh {
        if let Some(cached) = fresh(paths) {
            return Ok(cached);
        }
    }
    let stores = fetch(client).await?;
    net_kit::config::save_json_cache(
        &file(paths),
        &Cached {
            fetched_at: net_kit::jwt::now_secs(),
            stores: stores.clone(),
        },
    );
    Ok(stores)
}

/// The cached directory whatever its age, for a caller that would rather have a
/// stale name than make a request. `store show` is the case: a shop's name is
/// not something worth a round trip.
pub fn cached(paths: &net_kit::Paths) -> Option<Vec<Store>> {
    let cached: Cached = net_kit::config::load_json_cache(&file(paths))?;
    Some(cached.stores)
}

/// The cached directory, if it is still young enough to use.
fn fresh(paths: &net_kit::Paths) -> Option<Vec<Store>> {
    let cached: Cached = net_kit::config::load_json_cache(&file(paths))?;
    let age = net_kit::jwt::now_secs().saturating_sub(cached.fetched_at);
    (age < MAX_AGE_SECS && !cached.stores.is_empty()).then_some(cached.stores)
}

/// How old the cached directory is, and how many stores it holds, for
/// `doctor`. `None` when nothing has been fetched yet.
pub fn state(paths: &net_kit::Paths) -> Option<(u64, usize)> {
    let cached: Cached = net_kit::config::load_json_cache(&file(paths))?;
    Some((
        net_kit::jwt::now_secs().saturating_sub(cached.fetched_at),
        cached.stores.len(),
    ))
}

/// How many region lookups are in flight at once.
///
/// Not sixteen. The requests do not depend on each other, so firing them all
/// would be the fastest thing to write and the rudest thing to do -- sixteen
/// at once is a burst nobody's traffic looks like, and it is the shape that
/// gets a client rate-limited whatever the total volume was. Four is roughly
/// what a browser opens to one host, it costs four round trips instead of one,
/// and this runs about once a week. The client's pacing still spaces their
/// starts, so the whole directory takes about sixteen request intervals.
const IN_FLIGHT: usize = 4;

/// All sixteen regions, a few at a time.
///
/// Ordered by region afterwards so the directory does not reshuffle between
/// runs.
async fn fetch(client: &Arc<Client>) -> AppResult<Vec<Store>> {
    let mut found: Vec<(usize, Vec<Store>)> = Vec::new();
    let mut failures = 0;

    for batch in twlnz_api::REGIONS.chunks(IN_FLIGHT) {
        let offset = found.len() + failures;
        let mut tasks = tokio::task::JoinSet::new();
        for (index, (code, _)) in batch.iter().enumerate() {
            let client = Arc::clone(client);
            tasks.spawn(async move { (offset + index, client.stores(code).await) });
        }
        while let Some(joined) = tasks.join_next().await {
            match joined {
                // One region failing is not a reason to have no directory: the
                // rest still answer most questions, and the miss shows up as a
                // store that cannot be found rather than a broken command.
                Ok((index, Ok(stores))) => found.push((index, stores)),
                Ok((_, Err(_))) | Err(_) => failures += 1,
            }
        }
    }

    if found.is_empty() {
        return Err(twlnz_api::Error::Shape(
            "no region answered, so there is no store directory to search".into(),
        )
        .into());
    }
    if failures > 0 {
        eprintln!(
            "twlnz: {failures} of {} regions did not answer; the store list may be short",
            twlnz_api::REGIONS.len()
        );
    }

    found.sort_by_key(|(index, _)| *index);
    Ok(found.into_iter().flat_map(|(_, stores)| stores).collect())
}
