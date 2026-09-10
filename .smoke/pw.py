"""Minimal CDP driver (Playwright) for Bea desktop WebView2 smoke testing.

Mimics the agent-browser workflow: snapshot -> refs (@eN) -> interact.
Usage:
  python .smoke/pw.py snap [-i]            # interactive elements w/ refs
  python .smoke/pw.py click @e3            # click ref or selector
  python .smoke/pw.py fill @e2 "text"
  python .smoke/pw.py type @e2 "text"
  python .smoke/pw.py select @e2 "value"
  python .smoke/pw.py check @e2 / uncheck @e2
  python .smoke/pw.py press "Enter"
  python .smoke/pw.py text @e2 | text body
  python .smoke/pw.py attr @e2 href
  python .smoke/pw.py eval "js"
  python .smoke/pw.py shot out.png [--full]
  python .smoke/pw.py url | title
  python .smoke/pw.py wait "text" | wait 2000
  python .smoke/pw.py scroll down 500
  python .smoke/pw.py page                 # visible text of current page
  python .smoke/pw.py file @e2 "C:\\path\\file.mp4"   # set files on <input type=file>
Refs persist across calls in .smoke/refs.json.
"""
import json
import sys
import time
from pathlib import Path

from playwright.sync_api import sync_playwright

CDP = "http://127.0.0.1:9222"
REFS = Path(__file__).parent / "refs.json"
SELECTOR = ("a, button, input, select, textarea, summary, "
            '[role="button"], [role="menuitem"], [role="tab"], [role="checkbox"], '
            '[role="listbox"], [role="option"], [role="switch"], [role="slider"], [role="combobox"], '
            'label:has(input), [onclick], [tabindex]:not([tabindex="-1"])')


def get_page(browser):
    pages = [p for ctx in browser.contexts for p in ctx.pages]
    if not pages:
        raise SystemExit("no pages")
    # Prefer the app page; skip browser-internal pages (edge://downloads-hub
    # etc.) that WebView2 opens for handled downloads.
    def appish(url):
        return ("tauri.localhost" in url or "localhost:1420" in url
                or url.startswith("tauri") or "localhost:1430" in url)
    for p in pages:
        if appish(p.url):
            return p
    for p in pages:
        if not p.url.startswith("edge://") and not p.url.startswith("chrome://"):
            return p
    return pages[0]


def describe(el):
    tag = el.evaluate("e => e.tagName.toLowerCase()")
    role = el.get_attribute("role") or ""
    label = ""
    for attr in ("aria-label",):
        label = el.get_attribute(attr) or ""
        if label:
            break
    if not label:
        label = (el.text_content() or "").strip()
        label = " ".join(label.split())[:90]
    typ = el.get_attribute("type") or ""
    ph = el.get_attribute("placeholder") or ""
    val = el.get_attribute("value") or ""
    bits = [f"@{tag}"]
    if role:
        bits.append(f'role={role}')
    if typ:
        bits.append(f'type={typ}')
    if val:
        bits.append(f'value={val[:40]!r}')
    if ph:
        bits.append(f'placeholder={ph!r}')
    if label:
        bits.append(f'"{label}"')
    return " ".join(bits)


def snapshot(page, interactive_only=True):
    els = page.locator(SELECTOR)
    n = els.count()
    refs = {}
    lines = []
    idx = 0
    for i in range(n):
        el = els.nth(i)
        try:
            if not el.is_visible():
                continue
            box = el.bounding_box()
            if not box or box["width"] <= 0 or box["height"] <= 0:
                continue
        except Exception:
            continue
        idx += 1
        ref = f"@e{idx}"
        refs[ref] = describe(el)
        lines.append(f"{ref} {refs[ref]}")
    REFS.write_text(json.dumps(refs, indent=0))
    print("\n".join(lines) if lines else "(no interactive elements)")


def resolve(page, ref):
    text = json.loads(REFS.read_text())[ref]
    # ref text like '@button role=... "label"' -> find by best match
    import re
    m = re.match(r"@(\w+)", text)
    tag = m.group(1)
    label_m = re.search(r'"(.*)"', text)
    aria = re.search(r'aria=([^\s"]+)', text)
    # simplest robust approach: re-run the same discovery order and pick the nth
    return text


def find_ref(page, ref):
    """Locate an element: @ref from last snapshot, or a plain CSS selector."""
    if not ref.startswith("@"):
        return page.locator(ref).first
    desc = json.loads(REFS.read_text())[ref]
    import re
    tag = re.match(r"@(\w+)", desc).group(1)
    label = re.search(r'"(.*)"', desc)
    aria = re.search(r"aria=([^\s\"]+)", desc)
    typ = re.search(r"type=([^\s\"]+)", desc)
    ph = re.search(r"placeholder=([^ ]+)", desc)
    val = re.search(r"value=('[^']*'|\"[^\"]*\")", desc)
    # find candidates
    sel = SELECTOR
    els = page.locator(sel)
    idx = 0
    target = None
    n = els.count()
    for i in range(n):
        el = els.nth(i)
        try:
            if not el.is_visible():
                continue
            box = el.bounding_box()
            if not box or box["width"] <= 0 or box["height"] <= 0:
                continue
        except Exception:
            continue
        idx += 1
        if f"@e{idx}" == ref:
            target = el
            break
    if target is None:
        raise SystemExit(f"ref {ref} stale — re-snapshot")
    return target


def main():
    args = sys.argv[1:]
    if not args:
        raise SystemExit(__doc__)
    cmd = args[0]
    with sync_playwright() as pw:
        browser = pw.chromium.connect_over_cdp(CDP, timeout=10000)
        page = get_page(browser)
        page.set_default_timeout(8000)
        # The app uses window.confirm for destructive guards (VTT replace,
        # meeting delete). Smoke tests accept them so the guarded paths run.
        page.on("dialog", lambda d: (d.type == "confirm" and d.accept()) or (d.type == "beforeunload" and d.accept()) or (d.type == "alert" and d.accept()))
        try:
            if cmd == "snap":
                snapshot(page)
            elif cmd == "click":
                el = find_ref(page, args[1])
                el.click()
                time.sleep(0.4)
                print("clicked")
            elif cmd == "fill":
                el = find_ref(page, args[1])
                el.fill(args[2])
                print("filled")
            elif cmd == "type":
                el = find_ref(page, args[1])
                el.type(args[2], delay=15)
                print("typed")
            elif cmd == "select":
                el = find_ref(page, args[1])
                el.select_option(args[2])
                print("selected")
            elif cmd in ("check", "uncheck"):
                el = find_ref(page, args[1])
                (el.check if cmd == "check" else el.uncheck)()
                print(cmd)
            elif cmd == "press":
                page.keyboard.press(args[1])
                print("pressed")
            elif cmd == "text":
                el = find_ref(page, args[1])
                print(el.text_content())
            elif cmd == "attr":
                el = find_ref(page, args[1])
                print(el.get_attribute(args[2]))
            elif cmd == "eval":
                print(page.evaluate(args[1]))
            elif cmd == "shot":
                page.screenshot(path=args[1], full_page="--full" in args)
                print("saved", args[1])
            elif cmd == "url":
                print(page.url)
            elif cmd == "title":
                print(page.title())
            elif cmd == "wait":
                if args[1].isdigit():
                    time.sleep(int(args[1]) / 1000)
                    print("waited")
                else:
                    page.wait_for_selector(f"text={args[1]}", timeout=15000)
                    print("found:", args[1])
            elif cmd == "scroll":
                page.mouse.wheel(0, int(args[2]) if len(args) > 2 else 400)
                print("scrolled")
            elif cmd == "page":
                print(page.evaluate("() => document.body.innerText"))
            elif cmd == "file":
                el = find_ref(page, args[1])
                el.set_input_files(args[2])
                print("file set")
            elif cmd == "invoke":
                import json as _json
                payload = _json.loads(args[2]) if len(args) > 2 and args[2] else {}
                print(page.evaluate(
                    """async ([c, a]) => { try { return await window.__TAURI_INTERNALS__.invoke(c, a); } catch (e) { return 'ERR: ' + String(e); } }""",
                    [args[1], payload],
                ))
            elif cmd == "pages":
                for p in [p for c in browser.contexts for p in c.pages]:
                    print(p.url)
            else:
                raise SystemExit(f"unknown cmd {cmd}")
        finally:
            browser.close()


if __name__ == "__main__":
    main()
