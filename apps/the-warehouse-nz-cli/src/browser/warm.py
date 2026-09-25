"""Pass The Warehouse's Cloudflare challenge in a real browser, and hand back
the cookies that prove it.

Run by the CLI when a request is challenged, never by hand. Reads its inputs
from the environment -- TWLNZ_ORIGIN, TWLNZ_HEADLESS -- and prints one JSON
document on stdout; progress goes to stderr.

Cloudflare guards the sign-in and account pages with a JavaScript challenge
("Just a moment..."), which no HTTP client can run. A browser runs it and is
given `cf_clearance`. Measured 2026-09-25: that cookie is then accepted from
the CLI's own Safari-profile `wreq` client, so the browser does this one page
and leaves.

No credentials are passed to it and none are needed: signing in happens
afterwards over HTTP, with the cookies this hands back.
"""

import json
import os
import sys
import time

from camoufox.sync_api import Camoufox

# The platform this is running on, which is what `wreq` puts in its user agent.
PLATFORM = {"darwin": "macos", "win32": "windows"}.get(sys.platform, "linux")

ORIGIN = os.environ.get("TWLNZ_ORIGIN", "https://www.thewarehouse.co.nz")
HEADLESS = os.environ.get("TWLNZ_HEADLESS", "1") not in ("", "0", "false", "no")
TIMEOUT = int(os.environ.get("TWLNZ_BROWSER_TIMEOUT", "60"))

# The page to clear. The home page is not challenged, so it earns nothing.
GATED = "/login"

# The title Cloudflare's challenge page carries while it works.
CHALLENGE_TITLE = "Just a moment"

STEP = "starting the browser"


def note(message):
    global STEP
    STEP = message
    print(f"twlnz: {message}", file=sys.stderr, flush=True)


def fail(message):
    print(json.dumps({"error": message}))
    sys.exit(1)


def clearance(name):
    """Cloudflare's cookies only. The browser also holds its own `dwsid` and
    shopper tokens, which would overwrite a signed-in person's session."""
    return name == "cf_clearance" or name.startswith("__cf")


def main():
    note(f"starting a {'headless ' if HEADLESS else ''}browser")
    with Camoufox(headless=HEADLESS, os=PLATFORM, humanize=True) as browser:
        page = browser.new_page()

        note(f"loading {ORIGIN}{GATED}")
        page.goto(ORIGIN + GATED, wait_until="domcontentloaded",
                  timeout=TIMEOUT * 1000)

        # The challenge replaces itself with the real page when it passes,
        # usually in about five seconds.
        deadline = time.monotonic() + TIMEOUT
        while CHALLENGE_TITLE in page.title():
            if time.monotonic() > deadline:
                fail(f"the challenge did not clear in {TIMEOUT}s"
                     + ("" if not HEADLESS else ", so try --headful"))
            time.sleep(0.5)
        note("the challenge cleared")

        jar = {c["name"]: c["value"] for c in page.context.cookies()
               if clearance(c["name"])}
        if "cf_clearance" not in jar:
            fail(f"the page loaded without a cf_clearance "
                 f"(it holds: {sorted(jar) or 'nothing'})")

        note(f"carrying {len(jar)} cookies")
        print(json.dumps({"cookies": jar}))


if __name__ == "__main__":
    try:
        main()
    except SystemExit:
        raise
    except Exception as crash:
        detail = str(crash).strip().splitlines()
        fail(f"while {STEP}: {detail[0] if detail else type(crash).__name__}")
