# mitre10-nz-cli

`mitre10` — Mitre 10 New Zealand from the command line. Search and browse the
catalogue, check which stores have something, price a list of products, and
build a basket.

> Unofficial. Reverse-engineered from the site's own traffic; these endpoints
> can change without notice.

```
mitre10 search "fence paint"
mitre10 browse RF7336 --brand Nouveau --max 50
mitre10 product 174969
mitre10 stock 174969 --available
mitre10 stores --postcode 0110
```

## Most of it needs no account

Search, browse, product pages, stock, stores and the **whole basket** work
signed out. Only the wishlist and past orders need credentials.

```
mitre10 auth login
```

An email and a password, once. There is no captcha and no browser to drive —
unlike the Briscoes tool next door. What is stored is a refresh token, never
the password.

> If signing in ever answers "the sign-in endpoint refused a client with no
> storefront session", that is exit `6`, not a wrong password — no credential
> fixes it. Everything above needs no account regardless.

## Set a store first

Almost everything reads better with one, and `cart add` requires it: the
storefront wants a store on every line, delivery or not, because that is where
the stock is drawn from.

```
mitre10 stores --postcode 0110
mitre10 store set 66
```

Store ids are **numeric**. The `X57` spelling on the site is the internal SAP
code and no endpoint accepts it.

A basket holds whole units. Asking for more than a store has is not an error:
the line is capped and the reason printed above the table, so a quantity that
did not take never passes silently.

## Browsing

`mitre10 categories` prints the tree. A category code says its own depth, so a bare
code is all `browse` needs:

| Prefix | What it is |
| --- | --- |
| `RD` | department |
| `RS` | section |
| `RF` | fineline |
| `RC` | class |

`N1`–`N9` are the top menu landing pages. They are not catalogue categories, so
`browse` refuses them rather than answering with the whole catalogue — the
`Browsable` column says which is which.

Listings take `--brand`, `--colour`, `--size`, `--min`, `--max`, `--store`,
`--postcode` and `--facets`. `--facets` prints what the index offered, which is
the quickest way to find the spelling a filter wants.

Only `--sort relevance` is confirmed to exist; any other name is passed through
as a replica index and will fail with a 404 if the storefront has none.

## Stock

```
mitre10 stock 174969
```

One call, every store in the country. The configured store sorts first. `--near`
filters by store name and `--available` drops the ones that have none.

The words are the storefront's own and carry no count. `mitre10 product <code>
--store 66` does give a number, but only for that store.

## Prices in bulk

```
mitre10 prices 174969 269938 269940
cut -f1 codes.tsv | mitre10 prices
```

Eighteen at a time, which is what the storefront's batch endpoint takes. A code
it does not recognise is dropped rather than refused, so unknown ones are named
at the end.

## Output

`--json` on any command prints the same data as a document rather than a table.
Both come from one renderer, so they cannot disagree.

Exit codes: `2` misuse, `3` needs a sign-in, `5` no such store, `6` signing in is
unavailable, `7` rate limited.

## Settings

```
mitre10 config list
mitre10 config set postcode 0110
```

| Setting | What it is |
| --- | --- |
| `store` | the store commands use when `--store` is not given |
| `postcode` | the postcode a delivery listing is priced for |
| `output.color` | `auto`, `always` or `never` |

`mitre10 doctor` reports where the files are and whether the storefront, the search
index and the credentials actually answer.

## Environment

`M10_CONFIG_DIR`, `M10_STATE_DIR`, `M10_SECRET_BACKEND` (`file` or `keyring`),
`M10_DEBUG`, `M10_EMULATION`, and `M10_API_ORIGIN` / `M10_SITE_ORIGIN` /
`M10_SEARCH_ORIGIN` for pointing a test suite at a mock. `NO_COLOR` is honoured
whatever the config says.

## Install

```
cargo build --release          # target/release/mitre10
mitre10 update                     # replace it with a newer release
mitre10 completions zsh
```

The protocol lives in [`mitre10-api`](../../packages/mitre10-api), the rendering in
[`cli-kit`](../../packages/cli-kit). What is here is the part that is about
this program: reading the environment once, resolving flags against config, and
turning a failure into an exit code.
