# DeDupe2

DeDupe2 finds duplicate photos and videos across your libraries and drives,
moves the extra copies out of the way, and organizes what's left into dated
folders. It runs as a small local web app: you point it at folders, review
what it found, and decide what happens to each file.

**It never deletes anything.** Files it removes from a library go to a
*purgatory* folder you choose, keeping their original folder layout, so you
can inspect them and empty purgatory yourself when you're satisfied.

## Running it

You need a recent Rust toolchain ([rustup.rs](https://rustup.rs)).
[`exiftool`](https://exiftool.org) is optional. If it's installed, DeDupe2
uses it as a last resort for capture dates in formats the built-in readers
can't handle.

```bash
cargo run --release --bin webService
```

Then open <http://127.0.0.1:3000>. To use another port:

```bash
cargo run --release --bin webService -- --port 8080
```

The server only listens on `127.0.0.1` and refuses requests that come from
other websites, so neither another machine nor a page open in your browser
can drive it.

## What each tab does

| Tab | Purpose |
|---|---|
| **Compare Folders** | Compare a library (A) against a candidate tree (B): files only in A, only in B, and in both. Run **Deep scan** to prove the matches, move proven duplicates to purgatory, and copy or move B's originals into A by capture date. |
| **Compare Sets** | Compare one tree against several candidate trees at once: what each candidate adds and what is shared. |
| **Consolidate** | Within one year folder, merge same-date event folders (`07-30` and `07-30 Hawaii`) into one and move proven duplicates to purgatory. |
| **Organize** | Move or copy a tree into a destination laid out by capture date, using a template such as `YYYY/MM-DD <folder description>`. Files that already exist at the destination are left where they are. |
| **Verify** | Check that every file sits in the folder its capture date implies, and re-home the ones that don't. |
| **Similar** | Find visually similar images (resized, recompressed, edited) by perceptual hash, within one tree or between two, and decide per row which copy to keep. |
| **Settings** | Choose where the scan cache lives, save it now, or clear it. |

## How duplicates are decided

Throwing away an original by mistake is the worst thing this app could do,
so a file is moved to purgatory *as a duplicate* only when its full content
is proven identical to a copy that stays:

1. A hash of the first 64 KB of each file's image data (metadata stripped)
   nominates candidates.
2. Each candidate pair is then proven by full content: identical bytes,
   identical complete image data (so a copy whose only difference is edited
   EXIF still counts), or identical decoded pixels.
3. Right before moving, each file is checked again against its keeper. A
   keeper that has gone, a file that changed since the scan, the same file
   reached by two paths (hard link, case variant), and anything that can't
   be fully read all mean the file stays where it is.

Files that can't be read or decoded are never treated as duplicates of each
other. When identical copies differ only in metadata, the one with an
embedded capture date wins, then the larger file, then the shorter name.

The **Similar** tab is the exception by design: similar images are *not*
identical, and you choose which to keep.

Every move is copy, then byte-verify, then remove the source. A failure at
any step leaves the source untouched, and existing files are never
overwritten: name collisions get a ` (1)` suffix.

## Supported formats

- **Stills:** JPEG, PNG, HEIC/HEIF, AVIF, TIFF, DNG
- **RAW:** CR2, CR3, CRW, NEF, NRW, ARW, SR2, SRF, RAF, RW2, RWL, ORF, PEF,
  SRW, X3F, 3FR, IIQ, ERF, FFF, KDC, DCR, DCS, MEF, MOS, MRW, BAY, ARI, RAW
- **Video:** MOV, MP4, M4V, MTS, M2TS, AVI

## Capture dates

Dates come from the file itself: EXIF, Canon CIFF, RAW metadata, XMP, movie
track headers, and `exiftool` when it's installed. If a file has no date of
its own, the nearest dated folder within three levels above it stands in.
Recognised folder names include `2013/07-30 Hawaii`, `Hawaii 2013-07-30`,
`2013_07_30`, `2013.07.30`, `20130730`, `30-07-2013`, `30 July 2013`,
`July 30, 2013`, and the month-only `2013-07` and `July 2013`.

Ambiguous names such as `07-08-2013` (7 August or 8 July?) are left undated
rather than guessed. Month-only dates go into a folder without a day: with
`YYYY/MM-DD <folder description>`, `July 2013 trip` becomes `2013/07 trip`.

## The scan cache

Hashes, dates and perceptual fingerprints are cached per file, so a second
scan of the same drive is fast. An entry is recomputed only when the file's
size or modification time changes. Entries are keyed by volume, so an
external drive keeps its cache wherever it's mounted. A damaged cache file
is ignored and rebuilt.

| Variable | Default (macOS / Linux) |
|---|---|
| `DEDUPE2_CACHE` | `~/Library/Caches/dedupe2/scan-cache.bin` / `~/.cache/dedupe2/scan-cache.bin` |
| `DEDUPE2_SETTINGS` | `~/Library/Application Support/dedupe2/settings.json` / `~/.config/dedupe2/settings.json` |
| `DEDUPE2_PORT` | `3000` (`--port` takes precedence) |

## Development

```bash
cargo test
```

```bash
cargo clippy --all-targets
```

Tests use the sample files in `tests/TestImages`. Layout:

- `src/scanner/`: discovery, hashing, comparison, similarity, and the scan cache
- `src/filemover/`: everything that moves files; `safe_fs.rs` is the only code that copies, renames or removes them
- `src/exif/`: capture-date extraction
- `src/image_reader/`: format detection, image-data extraction, decoding, thumbnails
- `src/web/`: the axum app; one module per tab in `pages/`

One commit only reformats the code. To keep it out of `git blame`:

```bash
git config blame.ignoreRevsFile .git-blame-ignore-revs
```
