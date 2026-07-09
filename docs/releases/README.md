# Release notes pipeline

Counterspell's releases are cut by [Landmark](https://github.com/misty-step/landmark)
(`.github/workflows/landmark-release.yml`), not hand-tagged. The pre-stable
(0.x) line is pinned via `.releaserc.json` (repo-owned semantic-release
config, `branches: ["main"]`, pre-stable commit-analyzer rules) per the
operator ruling to stay below 1.0.0 until promotion is an explicit act
(`git tag v1.0.0`, see Landmark's README "Promotion to stable").

## What happens on a release

1. `CI` finishes successfully on `main`.
2. `Landmark Release` runs `landmark` in `mode: full`: analyzes commits since
   the last tag, decides the next `0.y.z` version, updates `CHANGELOG.md`,
   bumps the root `Cargo.toml` version (and regenerates `Cargo.lock`) via the
   `@semantic-release/exec` step in `.releaserc.json`, tags, and publishes a
   GitHub Release. It also synthesizes user-facing notes and writes
   `docs/releases/{version}.html` and `docs/releases/releases.json`
   (`landmark.public-release-notes.v1` schema).
3. The tag push (`v*`) separately triggers `.github/workflows/release.yml`,
   which builds and uploads the signed macOS binaries to the same GitHub
   Release.
4. `scripts/sync-release-accordion.py` reads the new entry out of
   `docs/releases/releases.json` and inserts it into
   `site/release-notes.html`'s accordion, newest first, closing the previous
   entry's "... -- present" date range. This is committed and pushed with
   `[skip ci]` (a plain push to `main`, not a tag, so the skip is safe here —
   see the note in `landmark-release.yml` about why the release commit itself
   does not carry `[skip ci]`).

## Re-running or fixing the accordion by hand

The script is the documented path, not a one-off:

```bash
python3 scripts/sync-release-accordion.py \
  --releases-json docs/releases/releases.json \
  --tag v0.3.0 \
  --site-file site/release-notes.html
```

It is idempotent (a tag already present in the accordion is a no-op) and
only touches the accordion markup — house style (plain language, no
invented benefits, no real client/project names) still needs a human pass
on the generated bullets before they ship, same as any synthesized copy.

If synthesis degrades or is skipped (`synthesis-required` is `false`), the
JSON entry may be missing and the accordion sync step in
`landmark-release.yml` will fail loudly rather than publish nothing — the
release itself still lands; only the site sync needs a manual re-run once
notes exist.
