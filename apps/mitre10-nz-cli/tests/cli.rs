//! The binary, run as a binary.
//!
//! Aimed at what unit tests cannot reach: that a signed-out command fails in
//! the way a script can act on, that a bad flag is refused before anything is
//! fetched, and that the config file round-trips through the real paths.

use std::process::Command;

use assert_cmd::prelude::*;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;

/// A run with its own config and state, so tests never touch a real one.
fn mitre10(home: &tempfile::TempDir) -> Command {
    let mut cmd = Command::cargo_bin("mitre10").expect("the binary builds");
    cmd.env("M10_CONFIG_DIR", home.path().join("config"))
        .env("M10_STATE_DIR", home.path().join("state"))
        // Never the system keychain: a test must not prompt, and must not
        // leave anything behind.
        .env("M10_SECRET_BACKEND", "file")
        // Nothing here should reach the network. Pointing every origin at a
        // port that refuses is what makes an accidental live call fail loudly
        // rather than quietly passing.
        .env("M10_API_ORIGIN", "http://127.0.0.1:1")
        .env("M10_SEARCH_ORIGIN", "http://127.0.0.1:1")
        .env("NO_COLOR", "1");
    cmd
}

#[test]
fn the_help_names_the_retailer() {
    let home = tempfile::tempdir().expect("a temp dir");
    mitre10(&home)
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("Mitre 10"));
}

#[test]
fn the_version_names_the_library_that_breaks_when_the_site_changes() {
    let home = tempfile::tempdir().expect("a temp dir");
    mitre10(&home)
        .arg("--version")
        .assert()
        .success()
        .stdout(contains("mitre10-api"));
}

#[test]
fn a_signed_out_account_command_exits_three_and_says_what_to_run() {
    // Exit 3 is the whole point: a script driving this can tell "sign in"
    // from "that product does not exist" without reading the message.
    let home = tempfile::tempdir().expect("a temp dir");
    mitre10(&home)
        .args(["wishlist", "list"])
        .assert()
        .code(3)
        .stderr(contains("not signed in").and(contains("mitre10 auth login")));
}

#[test]
fn a_landing_page_code_is_refused_before_a_request_is_made() {
    // `N2` filters nothing, so the index would answer with the whole
    // catalogue and call it a category. The origins above refuse connections,
    // so reaching the network at all would surface as a different failure.
    let home = tempfile::tempdir().expect("a temp dir");
    mitre10(&home)
        .args(["browse", "N2"])
        .assert()
        .code(2)
        .stderr(contains("not a browsable category code"));
}

#[test]
fn an_unknown_setting_is_refused_with_the_settings_that_exist() {
    let home = tempfile::tempdir().expect("a temp dir");
    mitre10(&home)
        .args(["config", "set", "colour", "always"])
        .assert()
        .code(2)
        .stderr(contains("output.color"));
}

#[test]
fn a_store_code_must_be_numeric() {
    // `X57` is the SAP code and appears on the site, but no endpoint takes
    // it -- catching it here beats failing on every later command.
    let home = tempfile::tempdir().expect("a temp dir");
    mitre10(&home)
        .args(["config", "set", "store", "X57"])
        .assert()
        .code(2)
        .stderr(contains("numeric"));
}

#[test]
fn a_setting_survives_into_the_next_run() {
    let home = tempfile::tempdir().expect("a temp dir");
    mitre10(&home)
        .args(["config", "set", "store", "66"])
        .assert()
        .success();
    mitre10(&home)
        .args(["config", "get", "store"])
        .assert()
        .success()
        .stdout(contains("66"));
}

#[test]
fn a_cart_command_with_no_basket_says_how_to_start_one() {
    let home = tempfile::tempdir().expect("a temp dir");
    mitre10(&home)
        .args(["cart", "list"])
        .assert()
        .code(2)
        .stderr(contains("mitre10 cart new"));
}

#[test]
fn adding_to_a_cart_with_no_store_set_says_so_rather_than_failing_at_the_storefront() {
    // The storefront wants a store on every line. Without one this would reach
    // the network and come back as a 400 that says nothing useful.
    let home = tempfile::tempdir().expect("a temp dir");
    mitre10(&home)
        .args(["cart", "add", "174969"])
        .assert()
        .code(2)
        .stderr(contains("mitre10 store set"));
}

#[test]
fn prices_with_no_codes_and_no_stdin_says_what_it_wanted() {
    let home = tempfile::tempdir().expect("a temp dir");
    mitre10(&home)
        .arg("prices")
        .stdin(std::process::Stdio::null())
        .assert()
        .code(2)
        .stderr(contains("no product codes"));
}

#[test]
fn an_unusable_emulation_profile_is_refused_once_rather_than_per_request() {
    let home = tempfile::tempdir().expect("a temp dir");
    mitre10(&home)
        .env("M10_EMULATION", "netscape")
        .args(["search", "paint"])
        .assert()
        .code(2)
        .stderr(contains("not a browser profile"));
}

#[test]
fn completions_writes_a_script_and_nothing_else() {
    // `source <(mitre10 completions zsh)` breaks on any stray line.
    let home = tempfile::tempdir().expect("a temp dir");
    mitre10(&home)
        .args(["completions", "zsh"])
        .assert()
        .success()
        .stdout(contains("#compdef mitre10"));
}

#[test]
fn an_unknown_shell_is_refused_with_the_shells_that_work() {
    let home = tempfile::tempdir().expect("a temp dir");
    mitre10(&home)
        .args(["completions", "ksh"])
        .assert()
        .code(2)
        .stderr(contains("bash"));
}
