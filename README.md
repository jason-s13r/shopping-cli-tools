# shopping-cli-tools

Command line tools for shopping at New Zealand retailers — and the libraries
they are made of.

Eight binaries, all unofficial, all reverse-engineered from what the retailers'
own websites call from a browser:

| Binary | App | Covers |
| --- | --- | --- |
| `gsnz` | [`grocery-nz-cli`](apps/grocery-nz-cli) | New World, PAK'nSAVE and Woolworths NZ side by side — `gsnz compare "2l milk"` |
| `fsnz` | [`foodstuffs-nz-cli`](apps/foodstuffs-nz-cli) | New World and PAK'nSAVE, one Foodstuffs client driving both |
| `wwnz` | [`woolworths-nz-cli`](apps/woolworths-nz-cli) | Woolworths NZ |
| `kmart` | [`kmart-cli`](apps/kmart-cli) | Kmart, Australia and New Zealand off one backend |
| `twlnz` | [`the-warehouse-nz-cli`](apps/the-warehouse-nz-cli) | The Warehouse NZ |
| `bgnz` | [`briscoe-group-nz-cli`](apps/briscoe-group-nz-cli) | Briscoes and Rebel Sport, one Magento backend behind a `store` header |
| `mitre10` | [`mitre10-nz-cli`](apps/mitre10-nz-cli) | Mitre 10 NZ — nationwide per-store stock in one call |
| `farmers` | [`farmers-nz-cli`](apps/farmers-nz-cli) | Farmers NZ — an Intershop storefront a browser has to open the door to |

None of the retailers offer a public API. Everything here is built by reading
the sites' own traffic, and it breaks when they change something.

## Structure

`apps/` ship; `packages/` are the libraries they are built from, and both
release the same way. Code moves to `packages/` when a *second* app needs it,
not before. dispat discovers projects by their directory, so adding one under
`apps/` or `packages/` is the entire registration.

## The libraries

The apps are thin front ends. The twelve crates in [`packages/`](packages) hold
the rest.

Shared:

| Crate | What it holds |
| --- | --- |
| [`net-kit`](packages/net-kit) | the process boundary: browser-fingerprinted HTTP, a persisted cookie jar, the OS credential store, config paths |
| [`cli-kit`](packages/cli-kit) | presentation with no domain: tables, `--json`, prompts, `doctor`, completions |
| [`gsnz-core`](packages/gsnz-core) | the grocery domain — one `Product`, `Cart`, `Order`, `Store`, and the `Retailer` trait |
| [`gsnz-ui`](packages/gsnz-ui) | the grocery renderers, every one a `cli_kit::View` over a `gsnz-core` type |
| [`build-kit`](packages/build-kit) | the build stamp and the `update` command that swaps the binary for a newer release |

Per retailer:

| Crate | What it holds |
| --- | --- |
| [`fsnz-api`](packages/fsnz-api) | the Foodstuffs edge API and the Club Plus login |
| [`wwnz-api`](packages/wwnz-api) | the Woolworths GraphQL API and its Auth0 flow |
| [`kmart-api`](packages/kmart-api) | Kmart's catalogue, stock, cart and login, both countries |
| [`twlnz-api`](packages/twlnz-api) | The Warehouse's Salesforce storefront — mostly HTML, and the one crate that parses it |
| [`bgnz-api`](packages/bgnz-api) | Briscoes and Rebel Sport: Klevu search, Magento GraphQL, the Gigya login |
| [`mitre10-api`](packages/mitre10-api) | Mitre 10: Algolia search and browse, the SAP Commerce OCC API, an OAuth2 PKCE login |
| [`farmers-api`](packages/farmers-api) | Farmers: an Intershop REST API, its HTML pipelines for stock and sign-in, and Constructor.io for search |

Two conventions:

- The API crates speak their vendor's vocabulary and depend on no shared domain
  crate; converting to `gsnz-core` is the app's job.
- The libraries read no environment — `clippy.toml` fails the build over
  `std::env::var`. An app reads its environment once, at the top, and passes
  the values down.

## `wreq`, not `reqwest`

These storefronts sit behind Cloudflare and Akamai, which fingerprint the TLS
handshake and HTTP/2 settings rather than the headers. Every `reqwest` TLS
backend is scored as a bot and answered with a bare 400 or a challenge page.
`net-kit` builds on `wreq`, which presents a real browser's fingerprint.

Cookies live in the OS credential store rather than a plaintext file. Kmart's
and Briscoes' bot checks cannot be passed by an HTTP client at all, so those
logins import browser cookies or drive a real browser for the one step that
needs it. Farmers goes further: Akamai admits only a jar a real browser earned,
so `farmers` starts [camoufox](https://camoufox.com) once per run, loads one
page, and spends the cookies over HTTP from there.

## Install

Each app builds and installs on its own:

```bash
cd apps/grocery-nz-cli
cargo install --path .
```

Published builds are on
[releases](https://github.com/jason-s13r/shopping-cli-tools/releases), tagged
`<app>/vX.Y.Z` — one release per project, never one for the repo. Each carries
`linux-x86_64` and `darwin-arm64` binaries and a `SHA256SUMS`. Installed
binaries replace themselves:

```console
$ gsnz update --check     # is there a newer one, and what changed in it?
$ gsnz update             # download it and swap it in
```

On any other platform `update` says what the release does have and leaves the
binary alone; build from source instead.

## Releases come from commits

A `feat(<project>): ...` or `fix(<project>): ...` on `main` releases that
project; the scope is the directory name. dispat writes the manifest version,
the tag and the GitHub release — do not bump versions by hand.

The root `dispat.yaml` declares the dependency graph so dispat can order the
builds and propagate a library's bump into the apps that depend on it:

```yaml
dependencies:
  foodstuffs-nz-cli: [gsnz-core, gsnz-ui, cli-kit, net-kit, fsnz-api, build-kit]
```

That is the only cross-reference between projects. There is no root
`package.json` and no cargo workspace; deleting a project directory removes it
completely.

## Working here

```bash
dispat run check --since all              # what CI runs, every project
dispat run test  --since all -p cli-kit   # one project
dispat status                             # what a release would do
dispat preview                            # the notes it would write
```

Without `--since all`, `dispat run` only covers projects with commits since
their last tag.

Each project owns its dependencies, build files, lockfiles and a `dispat.yaml`
defining as many of `build`, `test`, `lint`, `fmt`, `fmt-check`, `run`, `check`
and `release-build` as apply. `check` is the CI contract — `fmt-check lint
build test` minus whatever the project does not implement — and it is what
[`.github/workflows/ci.yml`](.github/workflows/ci.yml) runs on every push.

A project shipping binaries implements `release-build` and declares its runners
under `custom.releasePlatforms`; there is no cross-compiling, so each platform
is built on its own runner.

## Disclaimer

Not affiliated with Foodstuffs New Zealand, New World, PAK'nSAVE, Woolworths
New Zealand, The Warehouse, Kmart, Briscoes, Rebel Sport, Mitre 10 or Farmers.
There are no public APIs. These tools call the same undocumented endpoints the
retailers' own frontends call, and can break whenever they change something.
Use at your own risk.

## License

[Unlicense](LICENSE) — public domain.
