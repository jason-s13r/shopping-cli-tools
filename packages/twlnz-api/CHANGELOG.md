# Changelog

## twlnz-api/v0.2.1 (2026-09-18)

### Fixes

- name the password rather than fetch it
  Building a client read the stored password every time, for a renewal
  that almost never runs -- a keychain prompt per command. A source with
  nothing behind it now reports the same dead end as no source at all,
  once something tries to spend it.

### Dependencies

- net-kit: 0.1.1 -> 0.1.2


## twlnz-api/v0.2.0 (2026-09-11)

### Features

- add session verify and a browser-profile escape hatch
  `Client::verify` asks the account page once and reads where the request
  landed -- a redirect to the sign-in page is the whole answer -- with the
  landing URL now taken from net-kit's `landed_text` rather than a response
  header the site does not send.

  Cloudflare now refuses the whole Firefox family, so the default profile
  is Safari26_4; `profile()` and `client_spec_for()` let callers pick
  another without a release.

### Dependencies

- net-kit: 0.1.0 -> 0.1.1


## twlnz-api/v0.1.1 (2026-09-04)

### Fixes

- stop carrying captured secrets in the test fixtures
  The store fixture embedded The Warehouse's Google Maps key in three
  mapEmbedURL values, which secret scanning flagged. Nothing parses that
  field, nor the 33KB of rendered HTML beside it, so the fixture is cut to
  the envelope and three invented records -- one of them closed and without
  click and collect, so both states of each optional field are covered.

  Also drops a captured csrf_token from the stock modal fixture, and stops
  an assertion printing a signed add-to-cart url on failure, which CodeQL
  read as logging a secret.


## twlnz-api/v0.1.0 (2026-09-04)

### Features

- read and change the wishlist
  The wishlist has no JSON controller to ask. Reading it is the rendered
  `/wishlist` page, parsed as markup, and as an ordinary navigation rather
  than the `fetch` shape -- the exact opposite of the minicart, which is a
  read that must be an XHR.

  A row is a `<gep-product-card uuid pid addtocarturl>` custom element, so
  the ids are the element's own attributes. It carries no
  `data-gtm-product`, so there is no brand, EAN or category, and no
  per-channel stock marker -- only the cart's phrasing, kept as a string
  rather than fabricated into an `Availability`.

  Each row is minted with its own add-to-cart token, so this is the one
  place the two-step collapses: a saved item reaches the basket without a
  product page.

  The two write controllers need no `verify` token and still disagree
  about the method: `Wishlist-RemoveProduct` is a GET with a query string,
  `Wishlist-UpdateProductQuantity` a POST with a form body. Posting to the
  first is a 500 and a page of apology that names nothing. Both answer
  `{"success":true}` and no model, so a caller showing the result must
  re-read the page, and `success:false` is the only refusal there is.

  The list is paged behind a `Wishlist-MoreList` whose parameters were in
  no capture, so the heading's count is carried beside the rows: a caller
  that trusted the rows would report a shorter list than the person has.

- scrape The Warehouse storefront
  A Salesforce Commerce Cloud site with no public API, so this reads the
  HTML the storefront serves and the JSON its own page scripts fetch.

  Five constraints the site imposes, all measured live rather than read
  off a capture:

  - Firefox151 or Safari26_4 emulation. Firefox149 and Chrome149 are both
    answered with a Cloudflare managed challenge on the home page.
  - Writes are two steps: the page mints a signed `verify` token, the
    action spends it. A token cannot be reused or constructed.
  - Background requests come in two shapes -- `fetch` with same-origin
    Sec-Fetch headers, and legacy XMLHttpRequest with cors. Sending the
    wrong triad is a 403, not a redirect.
  - One cart model arrives under five names -- `cart`, `cartModel`,
    `basketModel`, `basket`, or unwrapped -- with its subtotal under
    `totals`, its line totals in two shapes, and two ids per line, of
    which removal takes `preOrderUUID`/`UUID` and refuses `uuid`. Reading
    a subset of that is quiet and wrong: the write lands and the basket
    reads as empty.
  - A product id is a variation master, not a leaf. Price and stock hang
    off the variant.

  Scraped payloads parse into a loose map: analytics blobs change field
  types between tiles, and one number where a string was expected would
  otherwise fail the whole response.
