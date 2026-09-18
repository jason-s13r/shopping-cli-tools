//! The binary, run as a binary.
//!
//! Aimed at what unit tests cannot reach: that a signed-out command fails in
//! the way a script can act on, that a bad flag is refused before anything is
//! fetched, and that the config file round-trips through the real paths.

use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;

/// `assert_cmd`'s own `Command`, not `std`'s: two of these tests write to
/// stdin, and only this one can.
use assert_cmd::Command;

/// A run with its own config and state, so tests never touch a real one.
fn farmers(home: &tempfile::TempDir) -> Command {
    let mut cmd = Command::cargo_bin("farmers").expect("the binary builds");
    cmd.env("FMNZ_CONFIG_DIR", home.path().join("config"))
        .env("FMNZ_STATE_DIR", home.path().join("state"))
        // Never the system keychain: a test must not prompt, and must not
        // leave anything behind.
        .env("FMNZ_SECRET_BACKEND", "file")
        // Nothing here should reach the network. Pointing both origins at a
        // port that refuses is what makes an accidental live call fail loudly
        // rather than quietly passing -- and against this storefront in
        // particular, a test suite that talked to the real one would degrade
        // its own address's standing.
        .env("FMNZ_SITE_ORIGIN", "http://127.0.0.1:1")
        .env("FMNZ_SEARCH_ORIGIN", "http://127.0.0.1:1")
        .env("NO_COLOR", "1");
    cmd
}

#[test]
fn the_help_names_the_retailer() {
    let home = tempfile::tempdir().expect("a temp dir");
    farmers(&home)
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("Farmers"));
}

#[test]
fn the_version_names_the_library_that_breaks_when_the_site_changes() {
    let home = tempfile::tempdir().expect("a temp dir");
    farmers(&home)
        .arg("--version")
        .assert()
        .success()
        .stdout(contains("farmers-api"));
}

#[test]
fn a_signed_out_logout_is_not_a_failure() {
    // Nothing to undo is not an error, and it must not spend a request
    // finding that out.
    let home = tempfile::tempdir().expect("a temp dir");
    farmers(&home)
        .args(["auth", "logout"])
        .assert()
        .success()
        .stdout(contains("Nothing to forget."));
}

#[test]
fn refresh_with_nothing_on_file_exits_three_rather_than_doing_nothing_quietly() {
    // The state a scheduled run has to be able to tell apart: no session and
    // nothing to make one with. Exiting 0 here would report success for a
    // timer that has silently stopped working.
    let home = tempfile::tempdir().expect("a temp dir");
    farmers(&home)
        .args(["auth", "refresh"])
        .assert()
        .code(3)
        .stderr(contains("not signed in"));
}

#[test]
fn a_password_command_is_a_setting_and_a_blank_one_is_refused() {
    // An empty command would be run and would fail, at the exact moment
    // nobody is watching.
    let home = tempfile::tempdir().expect("a temp dir");
    farmers(&home)
        .args(["config", "set", "auth.password_command", "   "])
        .assert()
        .code(2)
        .stderr(contains("config unset"));

    farmers(&home)
        .args([
            "config",
            "set",
            "auth.password_command",
            "pass show farmers",
        ])
        .assert()
        .success();
    farmers(&home)
        .args(["config", "get", "auth.password_command"])
        .assert()
        .success()
        .stdout(contains("pass show farmers"));
}

#[test]
fn keeping_the_password_can_be_turned_off_and_is_on_by_default() {
    let home = tempfile::tempdir().expect("a temp dir");
    farmers(&home)
        .args(["config", "get", "auth.store_password"])
        .assert()
        .success()
        .stdout(contains("true"));
    farmers(&home)
        .args(["config", "set", "auth.store_password", "no"])
        .assert()
        .success();
    farmers(&home)
        .args(["config", "get", "auth.store_password"])
        .assert()
        .success()
        .stdout(contains("false"));
    farmers(&home)
        .args(["config", "set", "auth.store_password", "maybe"])
        .assert()
        .code(2);
}

#[test]
fn a_password_command_and_stdin_are_refused_together() {
    // They are two answers to one question, and silently preferring one would
    // make a scripted login depend on which.
    let home = tempfile::tempdir().expect("a temp dir");
    farmers(&home)
        .args([
            "auth",
            "login",
            "--email",
            "shopper@example.invalid",
            "--stdin",
            "--password-command",
            "echo hunter2",
        ])
        .assert()
        .code(2);
}

#[test]
fn a_signed_out_status_says_what_to_run_without_reaching_the_network() {
    // The origins above refuse connections, so a run that consulted the site
    // would fail rather than print this.
    let home = tempfile::tempdir().expect("a temp dir");
    farmers(&home)
        .args(["auth", "status"])
        .assert()
        .success()
        .stdout(contains("Signed out").and(contains("farmers auth login")));
}

#[test]
fn auth_login_refuses_a_blank_email_before_posting_anything() {
    // Exit 2, not 3: nothing was rejected, the attempt never happened. The
    // origins refuse connections, so a run that posted would fail differently.
    let home = tempfile::tempdir().expect("a temp dir");
    farmers(&home)
        .args(["auth", "login", "--email", "  ", "--stdin"])
        .write_stdin("not-a-real-password\n")
        .assert()
        .code(2)
        .stderr(contains("an email and a password are both needed"));
}

#[test]
fn auth_login_refuses_an_empty_password_rather_than_sending_one() {
    let home = tempfile::tempdir().expect("a temp dir");
    farmers(&home)
        .args([
            "auth",
            "login",
            "--email",
            "shopper@example.invalid",
            "--stdin",
        ])
        .write_stdin("\n")
        .assert()
        .code(2)
        .stderr(contains("Password"));
}

#[test]
fn a_misspelled_sort_is_refused_before_a_request_is_made() {
    // The search service ignores a sort it does not know and answers in
    // relevance order, so passing it through would look like the flag quietly
    // not working.
    let home = tempfile::tempdir().expect("a temp dir");
    farmers(&home)
        .args(["search", "robe", "--sort", "cheapest"])
        .assert()
        .code(2)
        .stderr(contains("price-desc"));
}

#[test]
fn a_malformed_filter_says_how_to_write_one() {
    let home = tempfile::tempdir().expect("a temp dir");
    farmers(&home)
        .args(["search", "robe", "--filter", "brand Chisel"])
        .assert()
        .code(2)
        .stderr(contains("name=value"));
}

#[test]
fn regions_are_listed_without_touching_the_network() {
    // New Zealand's regions do not change, so this is a table and not a call.
    let home = tempfile::tempdir().expect("a temp dir");
    farmers(&home)
        .arg("regions")
        .assert()
        .success()
        .stdout(contains("13 regions").and(contains("no Farmers store")));
}

#[test]
fn a_region_with_no_farmers_store_is_refused_with_a_way_to_find_out() {
    let home = tempfile::tempdir().expect("a temp dir");
    farmers(&home)
        .args(["config", "set", "region", "Tasman"])
        .assert()
        .code(2)
        .stderr(contains("farmers regions"));
}

#[test]
fn a_region_is_filed_as_the_code_whichever_way_it_was_typed() {
    // The endpoint takes `AUK`. Storing "Auckland" verbatim would fail much
    // later, as "no such region", with nothing pointing back to the config.
    let home = tempfile::tempdir().expect("a temp dir");
    farmers(&home)
        .args(["config", "set", "region", "Auckland"])
        .assert()
        .success();
    farmers(&home)
        .args(["config", "get", "region"])
        .assert()
        .success()
        .stdout(contains("AUK"));
    // And it is then marked in the list.
    farmers(&home)
        .arg("regions")
        .assert()
        .success()
        .stdout(contains("AUK *"));
}

#[test]
fn an_unknown_setting_is_refused_with_the_settings_that_exist() {
    let home = tempfile::tempdir().expect("a temp dir");
    farmers(&home)
        .args(["config", "set", "colour", "always"])
        .assert()
        .code(2)
        .stderr(contains("output.color"));
}

#[test]
fn an_unusable_emulation_name_is_refused_once_rather_than_per_request() {
    let home = tempfile::tempdir().expect("a temp dir");
    farmers(&home)
        .env("FMNZ_EMULATION", "netscape")
        .arg("regions")
        .assert()
        .code(2)
        .stderr(contains("is not a browser profile"));
}

#[test]
fn a_storefront_that_cannot_be_reached_is_not_reported_as_a_sign_in_problem() {
    // Exit 1, not 3: a connection refused has nothing to do with credentials,
    // and a script retrying with a password would loop.
    let home = tempfile::tempdir().expect("a temp dir");
    farmers(&home)
        .args(["product", "6867065002"])
        .assert()
        .code(1);
}

#[test]
fn prices_given_nothing_says_so_rather_than_failing() {
    // `farmers prices < empty-file` asking for nothing and getting nothing is
    // a reasonable thing to have happened.
    let home = tempfile::tempdir().expect("a temp dir");
    farmers(&home)
        .arg("prices")
        .write_stdin("")
        .assert()
        .success()
        .stdout(contains("No product codes given."));
}

#[test]
fn a_completion_script_is_the_only_thing_on_the_stream() {
    // `source <(farmers completions zsh)` breaks on anything else.
    let home = tempfile::tempdir().expect("a temp dir");
    farmers(&home)
        .args(["completions", "zsh"])
        .assert()
        .success()
        .stdout(contains("_farmers"));
}

#[test]
fn json_output_parses_as_json() {
    let home = tempfile::tempdir().expect("a temp dir");
    let output = farmers(&home)
        .args(["--json", "regions"])
        .assert()
        .success()
        .get_output()
        .clone();
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout is a JSON document");
    assert_eq!(value["regions"][0]["code"], "NTL");
}

#[test]
fn an_account_only_command_exits_three_and_says_what_to_run() {
    // Exit 3 is the point: a script can tell "sign in" from "that product does
    // not exist" without reading the message. And it must not reach the
    // network to find out, which the refused origins above prove.
    let home = tempfile::tempdir().expect("a temp dir");
    for args in [
        vec!["orders"],
        vec!["wishlist", "list"],
        vec!["wishlist", "add", "6867065002"],
    ] {
        farmers(&home)
            .args(&args)
            .assert()
            .code(3)
            .stderr(contains("not signed in").and(contains("farmers auth login")));
    }
}

#[test]
fn a_cart_quantity_below_one_is_refused_before_a_request_is_made() {
    let home = tempfile::tempdir().expect("a temp dir");
    farmers(&home)
        .args(["cart", "add", "6867065002", "--quantity", "0"])
        .assert()
        .code(2)
        .stderr(contains("at least 1"));
}

#[test]
fn the_cart_is_not_an_account_command() {
    // A basket hangs off the session cookie and works signed out. Exit 1 here
    // is the refused origin, not a sign-in wall -- 3 would mean this had been
    // wired up as account-only by mistake.
    let home = tempfile::tempdir().expect("a temp dir");
    farmers(&home).args(["cart", "list"]).assert().code(1);
}

#[test]
fn the_cart_help_says_lines_are_addressed_by_their_listed_number() {
    let home = tempfile::tempdir().expect("a temp dir");
    farmers(&home)
        .args(["cart", "remove", "--help"])
        .assert()
        .success()
        .stdout(contains("cart list"));
}
