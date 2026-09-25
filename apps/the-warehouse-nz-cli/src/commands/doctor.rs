//! `doctor` -- what is set up, and whether it works.
//!
//! Two halves. The header is what the tool decided before talking to anyone:
//! where its files are, which island and store are selected. Then the site
//! itself, ending in a live call, because "configured" and "working" are
//! different claims and only the second is worth much.

use cli_kit::{emit, human_duration, Out, View};
use serde::Serialize;
use std::io::Write;
use std::time::Duration;

use crate::app::App;
use crate::error::{AppError, AppResult};

pub async fn run(app: &App) -> AppResult<()> {
    let shop = examine(app).await;
    let directory = crate::directory::state(&app.paths).map(|(age, count)| {
        format!(
            "{count} stores, fetched {} ago",
            human_duration(Duration::from_secs(age))
        )
    });
    let report = Doctor {
        // Named, because a report gets pasted into a bug and "0.1.0" alone does
        // not say what of.
        version: format!("twlnz {}", crate::build::short_version()),
        config_file: format!(
            "{} ({})",
            app.config_file.display(),
            if app.config_file.exists() {
                "present"
            } else {
                "not written yet"
            }
        ),
        state_dir: app.paths.state_dir.display().to_string(),
        secrets: net_kit::Backend::detect().describe().to_string(),
        directory,
        shop,
    };
    let healthy = report.shop.healthy;
    emit(&mut app.out(), &report)?;
    // The report already said what failed and why, so this only carries the
    // code -- `doctor` in a script should not need its output parsed.
    if healthy {
        Ok(())
    } else {
        Err(AppError::Reported(1))
    }
}

async fn examine(app: &App) -> Shop {
    // Both named off the cached store directory, so a report costs no extra
    // request and still reads as something a person recognises: an id and a
    // region code are exactly the two things nobody remembers.
    let store = app.config.store_id.as_deref().map(|id| {
        match crate::directory::cached(&app.paths)
            .into_iter()
            .flatten()
            .find(|s| s.id == id)
        {
            Some(store) => format!("{} ({id})", store.name),
            None => id.to_string(),
        }
    });
    let region = app
        .config
        .region
        .as_deref()
        .map(crate::commands::region::label);

    let client = match app.client() {
        Ok(client) => client,
        Err(e) => {
            return Shop {
                origin: app.endpoints().origin,
                emulation: format!("{:?}", app.emulation),
                pacing: pacing(&app.pace),
                browser: crate::browser::available(app.env.browser_python.as_deref()),
                island: app.island.map(|i| i.to_string()),
                region,
                store,
                login: None,
                listing: Err(e.to_string()),
                healthy: false,
            }
        }
    };

    let session = client.session();
    let login = session.account().then(|| Login {
        account: twlnz_api::StoredSession::load(&app.secrets())
            .ok()
            .flatten()
            .and_then(|s| s.email),
        expires_in: session
            .expires_at()
            .map(|exp| exp.saturating_sub(net_kit::jwt::now_secs())),
    });

    // One cheap listing that needs no account and no store, so it says whether
    // the chain works even for someone who has never signed in. A listing
    // rather than a store lookup on purpose: it exercises the HTML scrape,
    // which is the half most likely to break.
    let listing = client
        .page(
            &twlnz_api::Query::Category("specials".into()),
            0,
            4,
            None,
            &[],
        )
        .await;
    let listing = match &listing {
        Ok(page) if page.products.is_empty() => Err(
            "a listing came back with no products in it, which usually means the markup moved"
                .to_string(),
        ),
        Ok(page) => Ok(format!("{} products parsed", page.products.len())),
        Err(e) => Err(e.to_string()),
    };

    Shop {
        origin: app.endpoints().origin,
        emulation: format!("{:?}", app.emulation),
        pacing: pacing(&app.pace),
        browser: crate::browser::available(app.env.browser_python.as_deref()),
        island: app.island.map(|i| i.to_string()),
        region,
        store,
        login,
        healthy: listing.is_ok(),
        listing,
    }
}

#[derive(Serialize)]
struct Doctor {
    version: String,
    config_file: String,
    state_dir: String,
    secrets: String,
    /// The cached store directory. Worth reporting because a stale one is the
    /// only thing that would make `stores <name>` miss a shop that exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    directory: Option<String>,
    shop: Shop,
}

#[derive(Serialize)]
struct Shop {
    origin: String,
    /// The browser every request presents as. Reported because it is the one
    /// setting here that stops working on its own: Cloudflare scores the
    /// fingerprint, and a profile it has turned against 403s every line below
    /// this one at once. Knowing which one was tried is the first question a
    /// bug report about that has to answer.
    emulation: String,
    /// The gap between requests, so a report of throttling says what rate
    /// provoked it.
    pacing: String,
    /// Whether there is a browser to pass Cloudflare's challenge with, which
    /// signing in needs.
    browser: bool,
    /// What a listing contains.
    island: Option<String>,
    /// Which shops get asked. A different thing from the island, and named
    /// apart here for the same reason the commands are.
    region: Option<String>,
    store: Option<String>,
    login: Option<Login>,
    listing: Result<String, String>,
    /// Reachability only. Nobody having signed in is not a fault: most of this
    /// tool works signed out.
    healthy: bool,
}

#[derive(Serialize)]
struct Login {
    account: Option<String>,
    expires_in: Option<u64>,
}

/// The label column, wide enough for the longest label used.
const LABEL: usize = 14;

impl View for Doctor {
    fn text(&self, out: &mut Out) -> std::io::Result<()> {
        writeln!(out, "{}", self.version)?;
        line(out, "config file", &self.config_file)?;
        line(out, "state dir", &self.state_dir)?;
        line(out, "secrets", &self.secrets)?;
        line(
            out,
            "store list",
            &match &self.directory {
                Some(detail) => detail.clone(),
                None => out.dim("not fetched yet").to_string(),
            },
        )?;

        writeln!(out)?;
        writeln!(out, "{}", out.heading("The Warehouse"))?;
        indented(out, "origin", &self.shop.origin)?;
        indented(out, "presents as", &self.shop.emulation)?;
        indented(out, "pacing", &self.shop.pacing)?;
        indented(
            out,
            "browser",
            &match self.shop.browser {
                true => format!("{}, camoufox", out.good("ok")),
                false => format!("{}, {}", out.bad("none"), crate::browser::MISSING_BROWSER),
            },
        )?;
        indented(
            out,
            "island",
            &match &self.shop.island {
                Some(island) => island.clone(),
                // Not a warning: the site has a default, this only pins it.
                None => out.dim("not set"),
            },
        )?;
        indented(
            out,
            "region",
            &match &self.shop.region {
                Some(region) => region.clone(),
                None => out.dim("not set"),
            },
        )?;
        indented(
            out,
            "store",
            &match &self.shop.store {
                Some(store) => store.clone(),
                None => out.dim("none selected"),
            },
        )?;
        indented(out, "login", &describe_login(out, self.shop.login.as_ref()))?;
        match &self.shop.listing {
            Ok(detail) => indented(out, "listings", &format!("{}, {detail}", out.good("ok")))?,
            Err(e) => indented(out, "listings", &format!("{}, {e}", out.bad("no")))?,
        }

        writeln!(out)?;
        writeln!(
            out,
            "{}",
            if self.shop.healthy {
                out.good("healthy")
            } else {
                out.bad("not healthy")
            }
        )
    }
}

fn line(out: &mut Out, label: &str, value: &str) -> std::io::Result<()> {
    writeln!(out, "{label:<LABEL$} {value}")
}

fn indented(out: &mut Out, label: &str, value: &str) -> std::io::Result<()> {
    writeln!(out, "  {label:<width$} {value}", width = LABEL - 2)
}

fn describe_login(out: &Out, login: Option<&Login>) -> String {
    let Some(login) = login else {
        return out.dim("signed out").to_string();
    };
    let who = login.account.as_deref().unwrap_or("signed in");
    match login.expires_in {
        Some(0) => format!("{who}, {}", out.warn("expired")),
        Some(secs) => format!(
            "{who}, expires in {}",
            human_duration(Duration::from_secs(secs))
        ),
        None => who.to_string(),
    }
}

fn pacing(pace: &net_kit::Pace) -> String {
    match pace.interval().as_secs_f64() {
        0.0 => "off".to_string(),
        s => format!("{s}s between requests"),
    }
}
