"""Earn Farmers' bot-manager cookies in a real browser, and hand them back.

Run by the CLI whenever a gated call needs warmth, never by hand. Reads its
inputs from the environment -- FMNZ_ORIGIN, FMNZ_HEADLESS -- and prints one
JSON document on stdout; progress goes to stderr.

Why a browser at all, when this tool fetched the home page for itself until
now: the bot manager scores *where an `_abck` came from*. Measured against the
live site on one address within one minute -- a jar harvested here was served
the whole category tree through `wreq`, and a jar `wreq` earned from the same
home page was answered `Access Denied` by the byte-identical request. So no
emulation profile fixes this, and nothing but a browser can buy admission.

What it does NOT need to do is solve anything. The cookie this comes back with
is still unvalidated -- its flag reads `~-1~`, the same as the one the old
warm-up earned -- so the JavaScript sensor is not what is being demanded here.
Simply being a browser that loaded the page is enough, which is why this script
loads one page and leaves.

No credentials are passed to it and none are needed: warmth belongs to the
browser, not the person, and signing in happens afterwards over HTTP with the
cookies this hands back -- measured against the live site, the sign-in POST is
accepted on a jar this earned and answers on the credentials' own merits.
"""

import json
import os
import sys
import time

from camoufox.sync_api import Camoufox

# Present the platform this is actually running on. `wreq` picks its
# `User-Agent` platform from the host, so letting camoufox choose its own at
# random would have the jar earned by a Windows Firefox and spent by a macOS
# one -- a mismatch on the same cookie that nothing here would explain.
PLATFORM = {"darwin": "macos", "win32": "windows"}.get(sys.platform, "linux")

ORIGIN = os.environ.get("FMNZ_ORIGIN", "https://www.farmers.co.nz")
# Headless by default; the caller sets FMNZ_HEADLESS="" to show a window. The
# window is the stronger path when a headless run is refused.
HEADLESS = os.environ.get("FMNZ_HEADLESS", "1") not in ("", "0", "false", "no")
TIMEOUT = int(os.environ.get("FMNZ_BROWSER_TIMEOUT", "120")) * 1000

# A gated call, made from inside the page, to prove the warmth is worth
# handing back. Depth 1 is 3KB where the full tree is 2.5MB -- enough to be
# refused if this session is going to be, and cheap enough to always do.
PROBE = ("/INTERSHOP/rest/WFS/Farmers-Shop-Site/-;loc=en_NZ"
         "/categories?view=tree&depth=1")

# The cookies worth carrying back: the bot manager's, and only those.
#
# A browser that has loaded the home page is also holding an anonymous `sid`
# and two or three `__Host-SecureSessionID-t…` cookies, and those belong to
# *its* visit. Handing them over would overwrite the session cookie of
# somebody who is signed in, so the caller drops them anyway -- `is_warmth()`
# in farmers-api's session module is the authority. They are left here too, so
# that what crosses the process boundary is warmth and nothing else.
WARMTH = ("_abck", "bm_sz", "ak_bmsc", "bm_s", "bm_so", "bm_mi", "bm_sv",
          "bm_lso", "AKA_A2")

# What the script is doing, for the benefit of a failure that is not one of
# the ones handled below.
STEP = "starting the browser"


def note(message):
    global STEP
    STEP = message
    print(f"farmers: {message}", file=sys.stderr, flush=True)


def fail(message):
    print(json.dumps({"error": message}))
    sys.exit(1)


def refused(body):
    """The four shapes of refusal, which are one problem.

    Read off the body, never the status: Akamai denies with a 200, and the
    interstitial challenge is a 200 carrying none of the deny markers at all --
    it reads as a successful fetch of an empty page.
    """
    for marker in ("WAF_Deny_Page", "Access Denied",
                   "sec-if-cpt-container", "scf-akamai-logo"):
        if marker in body:
            return marker
    return ""


def keep(name):
    return name in WARMTH


def main():
    note(f"starting a {'headless ' if HEADLESS else ''}browser")
    with Camoufox(headless=HEADLESS, os=PLATFORM, humanize=True) as browser:
        page = browser.new_page()

        note(f"loading {ORIGIN}")
        page.goto(ORIGIN, wait_until="domcontentloaded", timeout=TIMEOUT)

        # The sensor runs on load and the cookies land while it does. Settling
        # here rather than reading straight away is the difference between a
        # jar that works and one that is half-written.
        try:
            page.wait_for_load_state("networkidle", timeout=TIMEOUT)
        except Exception:
            pass
        time.sleep(2)

        marker = refused(page.content())
        if marker:
            fail(f"the home page was refused in the browser too ({marker}); "
                 f"this is the address rather than the client"
                 + ("" if not HEADLESS else ", so try --headful"))

        note("checking the cookies are worth having")
        result = page.evaluate(
            """async (path) => {
                const r = await fetch(path, {headers: {Accept: "application/json"},
                                             credentials: "include"});
                const t = await r.text();
                return {status: r.status, body: t.slice(0, 400)};
            }""",
            PROBE,
        )
        marker = refused(result["body"])
        if marker or result["status"] >= 400:
            fail(f"the browser earned cookies the storefront then refused "
                 f"({marker or result['status']})"
                 + ("" if not HEADLESS else ", so try --headful"))

        jar = {c["name"]: c["value"] for c in page.context.cookies()
               if keep(c["name"])}
        if "_abck" not in jar:
            # Everything else is worthless without it, so this is a failure
            # even though the page loaded and the probe passed.
            fail(f"the browser finished without an _abck "
                 f"(it holds: {sorted(jar) or 'nothing'})")

        note(f"warmed, carrying {len(jar)} cookies")
        print(json.dumps({"cookies": jar}))


if __name__ == "__main__":
    try:
        main()
    except SystemExit:
        raise
    except Exception as crash:
        # A traceback is not an answer. The caller reads one JSON document and
        # otherwise falls back to the last line of stderr, which for a
        # Playwright timeout is a fragment of its own log and says nothing
        # about where it was.
        detail = str(crash).strip().splitlines()
        fail(f"while {STEP}: {detail[0] if detail else type(crash).__name__}")
