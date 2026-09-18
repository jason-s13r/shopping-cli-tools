//! `update` -- replacing this binary with a newer release.

use cli_kit::{emit, Out, View};
use serde::Serialize;
use std::io::Write;

use crate::app::App;
use crate::error::{AppError, AppResult};

pub async fn run(app: &App, version: Option<String>, check: bool, pre: bool) -> AppResult<()> {
    let stamp = crate::build::stamp();
    let current = build_kit::update::parse_version(stamp.version)?;
    let mut source = build_kit::update::Source::new(stamp.repo, "farmers-nz-cli", stamp.version)
        .with_token(app.env.github_token.clone());
    if let Some(api) = &app.env.update_api {
        source = source.with_api_base(api.clone());
    }

    let http = net_kit::http::build(net_kit::ClientSpec::new(
        net_kit::wreq_util::Profile::Chrome137,
        net_kit::wreq::redirect::Policy::limited(10),
    ))
    .map_err(|e| AppError::usage(format!("building the HTTP client: {e}")))?;

    let releases = build_kit::update::releases(&http, &source).await?;
    let wanted = match &version {
        Some(text) => {
            let want = build_kit::update::parse_version(text)?;
            build_kit::update::find(&releases, &want).ok_or_else(|| {
                AppError::usage(format!("there is no farmers-nz-cli release {want}"))
            })?
        }
        None => match build_kit::update::pick(&releases, &current, pre) {
            Some(release) => release,
            None => {
                let mut out = app.out();
                let preview = build_kit::update::newest_preview(&releases, &current);
                emit(
                    &mut out,
                    &Outcome {
                        current: stamp.version.to_string(),
                        available: None,
                        installed: false,
                        changelog: Vec::new(),
                        note: preview.map(|r| {
                            format!(
                                "{} is available as a pre-release: farmers update --pre-release",
                                r.version
                            )
                        }),
                    },
                )?;
                return Ok(());
            }
        },
    };

    if check {
        emit(
            &mut app.out(),
            &Outcome {
                current: stamp.version.to_string(),
                available: Some(wanted.version.to_string()),
                installed: false,
                note: None,
                changelog: build_kit::update::changelog(&releases, &current, wanted)
                    .into_iter()
                    .map(Entry::from)
                    .collect(),
            },
        )?;
        return Ok(());
    }

    let target = build_kit::exe_path().ok_or_else(|| {
        AppError::usage("cannot tell where this binary is, so there is nothing to replace")
    })?;
    let asset = wanted.asset_for_host().ok_or_else(|| {
        AppError::usage(format!(
            "release {} has no build for this platform ({})",
            wanted.version,
            build_kit::update::host_platforms().join(" or ")
        ))
    })?;
    let json = app.out().is_json();
    let path = build_kit::update::install(&http, &source, wanted, asset, &target, &|step: &str| {
        // Progress goes to stderr, and not at all under --json: a script
        // reading the document should not have to filter it out.
        if !json {
            eprintln!("farmers: {step}");
        }
    })
    .await?;
    // Recorded so `--version` can say where this binary came from, and so a
    // later `update` knows it manages this file.
    build_kit::Install {
        version: wanted.version.to_string(),
        tag: wanted.tag.clone(),
        url: wanted.url.clone(),
        asset: asset.name.clone(),
        path,
        installed_at: build_kit::now(),
    }
    .save(&app.paths)?;

    emit(
        &mut app.out(),
        &Outcome {
            current: stamp.version.to_string(),
            available: Some(wanted.version.to_string()),
            installed: true,
            note: None,
            changelog: Vec::new(),
        },
    )?;
    Ok(())
}

#[derive(Serialize)]
struct Outcome {
    current: String,
    available: Option<String>,
    installed: bool,
    note: Option<String>,
    /// What `--check` found, newest first. Empty everywhere else: the notes
    /// answer "should I take this?", which is only a question before the fact.
    changelog: Vec<Entry>,
}

/// One release in that changelog.
#[derive(Serialize)]
struct Entry {
    version: String,
    url: String,
    notes: String,
}

impl From<&build_kit::update::Release> for Entry {
    fn from(release: &build_kit::update::Release) -> Entry {
        Entry {
            version: release.version.to_string(),
            url: release.url.clone(),
            notes: release.notes.clone(),
        }
    }
}

impl View for Outcome {
    fn text(&self, out: &mut Out) -> std::io::Result<()> {
        match (&self.available, self.installed) {
            (Some(v), true) => writeln!(out, "Updated {} to {v}.", self.current)?,
            (Some(v), false) => writeln!(out, "{v} is available; running {}.", self.current)?,
            (None, _) => writeln!(out, "{} is the newest release.", self.current)?,
        }
        if let Some(note) = &self.note {
            writeln!(out, "{}", out.dim(note))?;
        }
        for entry in &self.changelog {
            writeln!(out)?;
            writeln!(out, "{}", out.heading(&entry.version))?;
            if entry.notes.is_empty() {
                writeln!(out, "  {}", out.dim("(no release notes)"))?;
                continue;
            }
            // Indented under the heading; blank lines stay bare rather than
            // becoming two spaces of trailing whitespace.
            for line in entry.notes.lines() {
                if line.trim().is_empty() {
                    writeln!(out)?;
                } else {
                    writeln!(out, "  {line}")?;
                }
            }
        }
        Ok(())
    }
}
