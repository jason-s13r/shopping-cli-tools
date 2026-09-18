# farmers-api

Farmers New Zealand as one client: catalogue, search, browse, per-store stock
and the account surface.

> Reverse-engineered from the site's own traffic. There is no public API and no
> documentation; these endpoints can change without notice.

## Three backends

The storefront is Intershop ICM. Search is not.

| Surface | Answers | Gated |
| --- | --- | --- |
| Intershop REST | products, prices, categories, variations — JSON | **yes** |
| Intershop `ViewX-` pipelines | per-store stock, sign-in, the account header — HTML | **yes** |
| Constructor.io | search, autocomplete, browse — JSON | no |

```
https://www.farmers.co.nz/INTERSHOP/rest/WFS/Farmers-Shop-Site/-;loc=en_NZ
https://www.farmers.co.nz/INTERSHOP/web/WFS/Farmers-Shop-Site/en_NZ/-/NZD
https://ac.cnstrc.com
```

Note the two Intershop bases disagree about where the locale goes, and that the
REST one carries it as a *matrix* parameter — `-;loc=en_NZ`, not `?loc=en_NZ`.
Both are written as the site writes them.

## The REST API cannot search

`/products?searchTerm=lego` is accepted and silently ignored. It answers
`total: 10000` and returns the catalogue in its natural order, so it looks like
a working endpoint returning bad results. Nothing here sends it.

Constructor.io is the real search backend, with a public search-only key the
storefront ships in its own bundle. Its results carry brand, stock status and
facets but **no price**, so anything showing money joins back to
`Client::product` by SKU — which is what `Client::price_hits` does.

Some terms are not searched at all: `lego` is redirected to
`/toys/lego-construction` by a merchandising rule. `Client::find` follows those;
`Client::search` hands the redirect back untouched.

## A product code is not a leaf

`6867065` is a master — a price *range*, no stock of its own, and it cannot be
bought. `6867065002` is the size that can be. The search index returns the
first and a basket takes the second, so `Hit::variants` is what to add and
`Client::variants` is what lists them.

## Bot protection

Akamai Bot Manager fronts everything under `/INTERSHOP/`, including the
pipelines. Two things follow.

**Warm first, and a browser has to do it.** Admission is an `_abck` cookie, and
as of 2026-09-18 the bot manager scores *where it came from*: one this crate
fetches for itself is refused whatever profile it wears, and one a real browser
earned is accepted — from this crate, on any profile. Measured on one address
inside one minute, a camoufox jar was served the whole category tree through
`wreq` while a `wreq`-earned jar got `Access Denied` from the byte-identical
request.

`Warmer` is the hook that fills the gap: a callback returning a cookie jar,
called on the first gated call and again whenever one is denied. Driving a
browser is the caller's business — `farmers-nz-cli` runs camoufox — because a
browser is a subprocess and a hundred megabytes, and this crate takes values.

What has *not* changed is the sensor: the browser's `_abck` reads `~-1~` too,
so what is demanded is a page load by something genuinely a browser, not a
proof-of-work. Such a jar is good for minutes and a couple of dozen requests,
then lapses without warning.

**Read the body, never the status.** A refusal has three shapes:

| | |
| --- | --- |
| `200` + `WAF_Deny_Page` | the REST API |
| `403 Access Denied` | a `ViewX-` pipeline |
| `429 {"cpr_chlge":"true"}` | a demand to solve a JavaScript proof-of-work |

There is a **fourth shape, and it is the dangerous one**: the Akamai
interstitial challenge, a `200` whose body is the sensor widget
(`sec-if-cpt-container`, `scf-akamai-logo`) and which carries none of the deny
markers. It passes for a successful fetch of an empty page — measured
2026-09-18, `/orders` answered this and the order history was reported as "no
orders". `is_challenge_page` catches it, and it is checked before the deny page
on every transport.

All four arrive as `Error::Denied` or `Error::Challenged`. None is about
credentials, and the `429` is not about rate.

**One endpoint is guarded by method.** `ViewExpressShop-AddProduct` — add to
cart, the endpoint retailers guard hardest — answers `403 Access Denied` to a
POST from this client and serves the *same add* asked for as a GET. Measured
2026-09-18 on one jar inside one minute, while POSTs to `ViewCart-Dispatch` and
`ViewWishlist-AddItems` were served and a browser's POST to `AddProduct` was
served. Intershop dispatches on the parameters rather than the verb, so
`Client::cart_add` sends a GET and gets back the same mini-cart fragment. It is
the only mutating call that does.

**`EMULATION` is no longer the lever.** It was: an earlier sweep found current
Firefox served and Chrome, Edge, Safari and Firefox 133 refused, every round,
twenty probes with the order reversed. That is still the reason `wreq` is used
rather than `reqwest` — every `reqwest` TLS backend is scored as a bot outright
— and still the reason curl is useless as a probe here: in the same minute
`wreq` on `firefox139` was served the whole category tree, curl with a
byte-identical `User-Agent` got a 5KB `WAF_Deny_Page`. **A curl refusal means
nothing about the site or the address.**

But a browser-earned jar is now spent successfully from `chrome139` and
`safari18_5` as well, so the profile is not what admission turns on. It stays
current Firefox because that is what the browser buying the jar presents as.
Reach for `FMNZ_EMULATION` second: a refusal is far more likely to be a lapsed
jar.

Requests are still kept few, because there is no reason to be noisy: a denial
is retried once after a fresh warm-up and then given up on, and nothing fans
out — `Client::stock` walks the thirteen regions in sequence.

## Signing in

An ordinary form POST with **no captcha**: fetch `/login`, scrape the
`SynchronizerToken`, post it with `ShopLoginForm_Login`,
`ShopLoginForm_Password` and `login=Login`. The 302 to `/account` *is* the
success signal, so the redirect must not be followed.

**No browser drives the sign-in itself** — only the warm-up needs one. Measured
2026-09-18, the form is accepted on a browser-earned jar and answers on the
credentials' own merits, which is what separates this from the Kmart flow where
the bot check guards the password submit.

Being signed in is a `__Host-SecureSessionID-p…` cookie. The kind letter is the
whole tell — every visitor carries two or three `-t…` cookies of the same
shape, so matching the prefix would report a browser that has never signed in
as signed in.

There is no grant to renew from: the session is a cookie, and the only way to
get another is to run the form again. `Client::renew` is that, from a `Reauth`
holding an email and a `net_kit::password::Source` — so unattended use costs a
stored password, unlike the token-based clients in this repo.

`Client::verify` is the question `renew` asks first, and it has three answers
rather than two. Good, lapsed, or **refused** — and a refusal is propagated
rather than reported as a lapse, because the login form would be refused in
exactly the same way. Reading it as a lapse would spend a password to learn
nothing and blame a credential that is fine.

## The basket

Read from the `ViewMiniCart-Status` fragment, not the cart page: 3KB against
160KB, no form token, and it carries the line item ids that every change is
addressed by. Those ids exist in exactly one place in the markup — inside a
JavaScript call in an `onclick` attribute — and they are **not** the SKU, which
matters because the same product can sit on two lines.

Three of the four calls need no token and answer with the new basket directly:

| | |
| --- | --- |
| `cart` | `ViewMiniCart-Status` |
| `cart_add` | `ViewExpressShop-AddProduct`, with `addToCartBehavior=expresscart` — without it the pipeline redirects instead of answering |
| `cart_remove` | `ViewMiniCart-RemoveItemFromMinicart?RemovePLI=…` |
| `cart_update` | `ViewCart-Dispatch` — **the one two-step**, needing a `SynchronizerToken` off `/cart` |

A basket is anonymous: it hangs off the session cookie, works signed out, and
survives signing in afterwards.

## What is not verified

The account the capture was made with had **no orders and no saved lists**, so
`extract::orders` and `extract::wishlists` are honest about their limits:

- The **empty** states are verified against real markup, and `says_empty` tells
  "you have none" apart from "this code could not read the page" — an empty
  vector alone means either, and confidently reporting an empty order history
  because a template moved is the wrong answer to give.
- The **populated** row selectors are inferred from the same template's class
  names and have never run against real rows. Treat them as unverified.

`wishlist_add` has one verified trap worth knowing: signed out it answers
**200 with a link to the sign-in page** rather than an error, so a lapsed
session reads as a successful save unless that is looked for. It is.

## Testing

`tests/client.rs` runs the whole thing against `wiremock`. The live site is
deliberately not used: it serves only a narrow set of browser fingerprints,
so a suite that talked to it would fail for reasons unrelated to the code *and*
would make the next
session worse.
