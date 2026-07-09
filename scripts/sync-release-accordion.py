#!/usr/bin/env python3
"""Insert a Landmark-cut release into site/release-notes.html's accordion.

Reads the matching entry out of the Landmark JSON artifact
(docs/releases/releases.json, schema landmark.public-release-notes.v1),
renders it as one more `.msk-acc-entry <details>` block, and inserts it at
the top of the accordion (newest first). The block that was previously
newest loses its `open` attribute and its "... -- present" date closes to
the new release's date.

Usage:
    python3 scripts/sync-release-accordion.py \
        --releases-json docs/releases/releases.json \
        --tag v0.2.0 \
        --site-file site/release-notes.html

Idempotent: re-running for a tag already present in the accordion is a
no-op.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

ACCORDION_OPEN = (
    '<div class="msk-accordion">\n'
    "            <!-- counterspell-927: scripts/sync-release-accordion.py inserts\n"
    "                 each new landmark-cut release here, newest first -->"
)
PRESENT_DATE_RE = re.compile(r'(<span class="msk-acc-date">[^<]*?)&mdash; present(</span>)')
OPEN_ENTRY = '<details class="msk-acc-entry" open>'
CLOSED_ENTRY = '<details class="msk-acc-entry">'
RELEASES_URL = "https://github.com/misty-step/counterspell/releases/tag/{tag}"


def load_entry(releases_json: Path, tag: str) -> dict:
    entries = json.loads(releases_json.read_text())
    bare_tag = tag[1:] if tag.startswith("v") else tag
    for entry in entries:
        if entry.get("tag") in (tag, f"v{bare_tag}") or entry.get("version") in (tag, bare_tag):
            return entry
    raise SystemExit(
        f"no entry for tag {tag!r} in {releases_json} "
        f"(known tags: {[e.get('tag') for e in entries]})"
    )


def bullets_from_sections(entry: dict) -> list[str]:
    bullets: list[str] = []
    for section in entry.get("sections", []):
        for bullet in section.get("bullets", []):
            text = bullet.get("text", "").strip()
            if text:
                bullets.append(text)
    return bullets


def render_entry(tag: str, date: str, bullets: list[str]) -> str:
    items = "\n".join(f"                  <li>{b}</li>" for b in bullets)
    return f"""            <details class="msk-acc-entry" open>
              <summary class="msk-acc-head">
                <span class="msk-acc-ver">{tag}</span>
                <span class="msk-acc-date">{date} &mdash; present</span>
                <span class="msk-acc-chev" aria-hidden="true"></span>
              </summary>
              <div class="msk-acc-body">
                <ul>
{items}
                </ul>
                <p class="ae-status">
                  <svg class="ae-icon ae-ok" data-lucide="circle-check">
                    <use href="#i-circle-check" />
                  </svg>
                  <span class="ae-status-label"
                    >Tagged as
                    <a
                      href="{RELEASES_URL.format(tag=tag)}"
                      >{tag}</a
                    >
                    with signed download archives.</span
                  >
                </p>
              </div>
            </details>
"""


def sync(site_file: Path, tag: str, date: str, bullets: list[str]) -> bool:
    html = site_file.read_text()

    if f'<span class="msk-acc-ver">{tag}</span>' in html:
        print(f"{tag} already present in {site_file}; nothing to do.")
        return False

    if ACCORDION_OPEN not in html:
        raise SystemExit(f"could not find {ACCORDION_OPEN!r} in {site_file}")

    # Close out whichever entry was previously newest before inserting.
    html, present_subs = PRESENT_DATE_RE.subn(rf"\g<1>&mdash; {date}\2", html, count=1)
    if present_subs:
        html = html.replace(OPEN_ENTRY, CLOSED_ENTRY, 1)

    new_block = render_entry(tag, date, bullets)
    html = html.replace(ACCORDION_OPEN, ACCORDION_OPEN + "\n" + new_block, 1)

    site_file.write_text(html)
    return True


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--releases-json", type=Path, required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--site-file", type=Path, required=True)
    args = parser.parse_args()

    entry = load_entry(args.releases_json, args.tag)
    date = str(entry.get("published_at", ""))[:10]
    if not date:
        raise SystemExit(f"entry for {args.tag} has no published_at")
    bullets = bullets_from_sections(entry)
    if not bullets:
        raise SystemExit(f"entry for {args.tag} has no section bullets to render")

    changed = sync(args.site_file, args.tag, date, bullets)
    print(f"{'updated' if changed else 'skipped'} {args.site_file} for {args.tag}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
