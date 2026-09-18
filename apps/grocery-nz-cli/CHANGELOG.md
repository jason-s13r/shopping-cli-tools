# Changelog

## grocery-nz-cli/v0.3.2 (2026-09-18)

### Fixes

- stop reading the password on every command
  The client was built with the password already fetched, so every command
  paid a keychain prompt for a renewal that almost never happens. It now
  carries where the password is. auth status and auth refresh still ask
  outright, because that is what they report on.

### Dependencies

- net-kit: 0.1.1 -> 0.1.2
- fsnz-api: 0.2.0 -> 0.2.1
- wwnz-api: 0.1.0 -> 0.1.1


## grocery-nz-cli/v0.3.1 (2026-09-11)

### Dependencies

- build-kit: 0.2.0 -> 0.3.0
- net-kit: 0.1.0 -> 0.1.1


## grocery-nz-cli/v0.3.0 (2026-09-04)

### Features

- show release notes in update --check
  A version number alone is not enough to decide whether to take an update, and
  the release body was being dropped at the wire type.

  `changelog()` returns every release being crossed, newest first, skipping
  previews a stable build was never offered; a downgrade answers with the notes
  of the version asked about. Under --json it is a `changelog` array.

### Fixes

- declare build-kit once
  dispat rewrites one version range per manifest, so the [build-dependencies]
  build-kit stayed at ^0.1.0 when the [dependencies] one moved to ^0.2.0 and the
  release failed to select a version. Latent until build-kit's first bump.

  A path dependency needs a version only to be publishable, so the second
  declaration drops it rather than carrying a number nothing keeps honest.

- rebuild so --version names the real library versions
  grocery-nz-cli/v0.2.0 was released at 4395b4f, before 6d9520e taught the matrix runner to run the version stage ahead of the build. The libraries were compiled from unbumped manifests, and `env!("CARGO_PKG_VERSION")` stamped what they were replacing: the published binary reports gsnz-core, gsnz-ui, cli-kit and fsnz-api as 0.1.0, and all four were 0.2.0 in that release.

  Nothing in this package changed; being built after 6d9520e is the fix.

### Dependencies

- build-kit: 0.1.0 -> 0.2.0


## grocery-nz-cli/v0.2.0 (2026-09-03)

### Features

- bind the account's cart to a store
  `store set` chose which store searches were scoped to and left the cart alone,
  so a freshly signed-in account could search but every `cart add` was refused
  with "Store is not defined" and nothing here could fix it.

  The account's cart carries its own store, and one bare POST sets it:

      POST /v1/edge/cart/store/{storeId}    no body, empty 200

  Found by recording a store change on the site; the earlier capture happened
  not to contain one, which is why an endpoint was assumed not to exist. The
  store cookies on the storefront are the browser's own copy, not the source of
  truth.

  `store set` now writes the local preference and binds the cart, so it means
  the same thing on all three shops, and Foodstuffs claims server_side_store
  like Woolworths already did. Signed out there is no cart to bind and browsing
  still works, so that case is passed over rather than raised.

  Verified against a live account: the cart reported no store, `store set` bound
  it to PAK'nSAVE Whangarei, and an add that had been refused went through.

- set the config from the command line
  Ten settings had no command, and the default shop could be set exactly once --
  as a side effect of the first `store set`, which only filled it in when it was
  empty. Changing it afterwards meant editing the file.

  `gsnz config list|get|set|unset|path` covers all of them, and `gsnz use ww` is
  the shorthand for the one that gets touched most. Two commands for one key is
  worth it here: `use` is the flag you would otherwise pass to every command.

  Writes go through the typed Config, so a value that will not parse is refused
  at the point of making the typo, and what lands in the file is the canonical
  form -- `ww` is stored as `woolworths`, whatever was typed. Only settings that
  differ from their default are written now, so the file stays skimmable for the
  people who do edit it by hand.

  `store set` stays separate. It is not a plain write: it resolves a name
  against the live store list, and on Woolworths it binds the cart server-side.

- make doctor a report that checks things
  Reshaped to match `fsnz doctor`: a header block of what the tool decided, then
  one section per shop of indented label/value lines, then a verdict. The old
  table had a column the status could not name and painted cells that
  box-drawing measured wider than they looked.

  It now also *checks*. One store call per shop says whether the chain works,
  which is a different claim from "configured" and the more useful one, and the
  store list it returns names the selected store for free. Adapters describe
  their own hostnames and token state through Retailer::facts, because what is
  worth reporting differs: Foodstuffs has two hostnames and a mintable token,
  Woolworths has a storefront and an Auth0 tenant.

  Three things found while doing it:

  - `retailer = "nw"` in the config file was rejected while `-b nw` worked, and
    RetailerId serialised as "new-world" while id() said "newworld". It now
    deserialises through FromStr and serialises through id(), so the file takes
    what the flag takes and there is one machine spelling. This changes `--json`:
    "new-world" becomes "newworld". gsnz-ui's stability test caught it, which is
    what that test is for; the tool is a day old and two spellings of a machine
    name would outlive that.
  - doctor exited 0 however badly it went. AppError::Reported carries an exit
    code without a message, since the report already said everything.
  - the suite started making real calls to three supermarkets. Every host is
    pointed at a closed port in the sandbox, which also took the run from 10.5s
    back to 1.1s.

- sign in once per account, not once per shop
  Three shops, two accounts -- and nothing said so. `auth login -b nw` reported
  only New World, so the obvious next move was to run it again for PAK'nSAVE
  with the same Club Plus password, which had already been signed in.

  Every auth command now works in credential units and names what it covered.
  `gsnz auth login` with no `-b` is the whole setup: two prompts, one per
  account. `-b nw,pns` is one credential, not two.

  This is why there is no `-b fs`: a pseudo-retailer would put something in
  RetailerId that is not a shop, and dropping `-b` altogether is fewer
  keystrokes than adding one.

  --email with more than one account in scope is refused rather than tried on
  both.

### Fixes

- complete the variant key, and stop misreporting an unbound cart
  `gsnz -b ww cart add 282848` sent the stock code as the variant key. The site
  does not reject that as unknown -- it answers "these items are no longer
  available at this store", which reads as the product being gone, so every add
  looked like a stock problem. Search prints the stock code and that is what
  people type, so it is completed to `282848-EA` before it is sent, or `-KGM`
  when kilograms were asked for.

  `gsnz -b pns cart add` reported "no PAK'nSAVE store selected: run `store set`"
  when a store *was* selected. Two different problems shared one error: the
  local setting this tool owns, and the store the account's cart is bound to
  server-side. `store set` writes a config file and does not touch the cart, so
  the advice was a dead end. Error::CartUnbound says what it is and points at
  the website, which is the only place that binding can be changed.

  Also: `store set` printed the store *listing* view, whose footer says "1
  store. Select one: gsnz store set ...", so a successful set read as though
  nothing had happened. And an error whose message already contained its
  source's words printed them twice, which read as two problems.

- put the shop-scoped flags where they apply
  Two global flags were accepted everywhere and read almost nowhere.

  --token was Foodstuffs-only, so `gsnz -b ww --token X search` silently used
  something else -- the same trap --store had, which is already an explicit
  refusal. It is gone: GSNZ_NEWWORLD_TOKEN and GSNZ_PAKNSAVE_TOKEN are per shop
  and cannot be ambiguous, and token_command covers the automation case. A token
  is scoped to one banner anyway, and one presented to the other is not refused,
  it answers with that account's empty cart.

  --store now belongs to the four commands that quote a price against a store,
  so `gsnz config list --store 1` is an error rather than a no-op, and it is out
  of the help for every command that cannot use it.

  -b stays global. `gsnz auth status -b ww` after the subcommand is a natural
  thing to type, and a non-global argument only parses before it -- so it still
  appears under `gsnz config --help`, which is the price of that. Hiding it per
  subcommand is not available: clap propagates a global after mut_subcommand
  runs, so mut_arg panics.

- report health as a list, and only show real gaps
  The report was a table whose first column had no header, because the status
  has nothing to call itself. It was also painted, and box-drawing measured the
  escape codes as width, which pushed every border out of line. An aligned list
  has neither problem, and puts a hint under the detail it belongs to.

  The capability matrix is gone from an ordinary run. It exists to surface gaps,
  and there are none left -- so it printed three columns of "yes", which is a
  table that says nothing. It comes back on its own the day a shop cannot do
  something, which is the only day it is worth reading.

- a refused sign-in is not an expired session
  A wrong Woolworths password reported "the Woolworths session has expired" and
  suggested `gsnz auth refresh` -- a command that cannot help, for a session
  that never existed. The upstream error is LoginRefused, whose Fault is
  AuthFault::Rejected, which the adapter mapped to SessionExpired along with
  every other rejection.

  The same upstream error means different things depending on what was being
  attempted, so each adapter now reads failures from its login through a
  separate mapping. Error::LoginRefused carries the reason, exits 3 like the
  other auth failures, and offers no hint, because there is nothing useful to
  suggest beyond the password.

  Two more from the same transcript:

  - `auth login` and `auth refresh` abandoned every remaining account at the
    first failure, and threw away the statuses of the ones that had already
    worked. They now collect failures, print what happened to every account,
    and return the first so the exit code is still right.
  - `auth refresh` treated "never signed in" as a failure. It is a note now:
    refreshing everything is maintenance, and one shop being signed out is not
    a reason for it to fail.

  Both login chains are now verified against the real services.

### Dependencies

- gsnz-core: 0.1.0 -> 0.2.0
- gsnz-ui: 0.1.0 -> 0.2.0
- cli-kit: 0.1.0 -> 0.2.0
- fsnz-api: 0.1.0 -> 0.2.0


## grocery-nz-cli/v0.1.0 (2026-09-03)

### Features

- list the library versions in --version
  The seven packages release on their own tags, so "gsnz 0.1.0" alone does not
  say which fsnz-api is compiled in -- and that is the part that breaks when a
  supermarket changes its API. One per line, aligned under the label column:
  seven of them comma-run together is a wall, and the version is the part being
  looked for.

  `-V` stays one line, which is what a script greps and what a bug report
  pastes; the libraries, the binary path and the install record go on
  `--version`.

  Overrides::get memoises the environment read, because clap builds the version
  string before App exists and the install record lives under a state directory
  the environment can move.

- Retailer::login, auth, doctor and update
  One subject for both because it is one change: the trait method and its only
  callers. A required method cannot land in gsnz-core ahead of the adapters that
  implement it without leaving a commit that does not compile, and a default
  returning Unsupported would be a lie -- both retailers do support signing in.

  login takes a code callback rather than a second command: the token a Club
  Plus challenge hands back only means anything inside the exchange that
  produced it, so splitting the flow across two runs would mean persisting a
  half-finished login.

  doctor prints the capability matrix before anything runs into it. update takes
  the file to replace from build-kit rather than reading current_exe deep
  inside, and stays silent on stderr under --json.

  Also: release-build.sh, the release flow and platforms, and a README.

  Not covered by any test: `auth login` against real Club Plus and real Auth0.
  Both chains are only exercised against mocks, and that is a manual step.

- cart, orders and compare
  compare fans out with tokio tasks and reports a shop that could not answer on
  stderr rather than failing: a lapsed Woolworths session must not hide the two
  Foodstuffs prices. Rows matched by description rather than product code are
  marked, and --strict drops them.

  `-b` now takes a list so compare can span a subset; every other command
  insists on exactly one and says so.

  cart add reads the current line before writing, because both APIs take an
  absolute quantity per line and neither has an increment -- and a line priced
  by the kilogram counts the addition in kilograms, whatever the flag says.

- the app skeleton, both adapters and the first commands
  Consumes all seven packages for the first time. Two Retailer implementations
  -- Foodstuffs parameterised by banner and instantiated twice, Woolworths once
  -- with their conversions, a lazy Registry, and search/specials/browse/
  departments/stores/store/completions on top.

  src/env.rs is the only std::env::var in the tree; the libraries take values,
  and a clippy.toml in each enforces it.

  Woolworths refuses a per-command --store rather than ignoring it: prices are
  quoted against the store the cart is bound to server-side, so honouring the
  flag is impossible and ignoring it would be a wrong-price bug.

### Fixes

- declare the build-dependency as an inline entry
  dispat rewrites the version of an inline dependency entry on release. It
  rewrote all seven in [dependencies] and did not see the eighth, an eighth
  declaration of build-kit written as a [build-dependencies.build-kit]
  sub-table -- so that one stayed at ^0.0.0 while build-kit became 0.1.0, and
  syncLock failed with "failed to select a version".

  The ranges are at ^0.1.0 here too: the seven libraries released, this did
  not, so its manifest was the one thing left pointing at versions that no
  longer exist.

- drop the doubled colon in the auth prompts
  `cli_kit::prompt` appends ": " itself, so "Email: " rendered as "Email: : ".
  Its no-terminal error reads "{label} is required", which is what the
  regression test asserts on -- and which only reads correctly with a bare
  label either.
