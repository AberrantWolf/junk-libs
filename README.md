# junk-libs

Shared Rust libraries for [Junk Collector](https://github.com/AberrantWolf/junk-collector), [Retro Junk](https://github.com/AberrantWolf/retro-junk), [Phono Junk](https://github.com/AberrantWolf/phono-junk), [Trash Amp](https://github.com/AberrantWolf/trash-amp) and related tools.

The crates provide parsing, hashing, audio, rendering and UI building blocks. Application policy — collections, providers, authorization, workflows — belongs in the consumers.

## Crates

| Crate | Purpose |
|---|---|
| `junk-libs-core` | Streaming CRC32/MD5/SHA-1 hashing, checksum types, reader traits, filename grouping, PDF/text document classification. |
| `junk-libs-disc` | CUE, CHD, ISO 9660 and redumper parsing; CD sector and PCM readers. See [PLAYBACK.md](junk-libs-disc/PLAYBACK.md). |
| `junk-libs-disc-id` | MusicBrainz, FreeDB/CDDB and AccurateRip disc IDs from a TOC. |
| `junk-libs-accuraterip` | AccurateRip v1/v2 checksums, dBAR parsing, matching and sample-offset search. |
| `junk-libs-dat` | Streaming Logiqx XML and ClrMamePro DAT parsing. |
| `junk-libs-game-formats` | Game-dump inspection, normalization and hashing. |
| `junk-libs-music` | FLAC/MP3 fingerprints, MusicBrainz models, AcoustID responses, album comparison. See [README](junk-libs-music/README.md). |
| `junk-libs-audio` | Byte sources, PCM frame coordinates, decoding, resampling, DSP. See [README](junk-libs-audio/README.md). |
| `junk-libs-playback` | Playback queue transitions and generation-fenced seeking. |
| `junk-libs-platen` | PDF rendering and text extraction; pure Rust by default, optional PDFium backend. See [README](junk-libs-platen/README.md). |
| `junk-libs-pdfium` | PDFium bindings; downloads the matching native library at build time. |
| `junk-libs-raster` | GUI-independent raster helpers. |
| `junk-libs-text` | Unicode shaping, bidi layout, font fallback and rasterization. See [README](junk-libs-text/README.md). |
| `junk-libs-egui-docview` | Zoomable, pageable egui canvas with editable rectangular regions. |
| `junk-libs-egui-pdfdoc` | Eager and lazy PDF/image models for the document viewer. See [README](junk-libs-egui-pdfdoc/README.md). |

Optional features are listed in each crate's `Cargo.toml` and README.

## Using the crates

Depend on individual crates from Git:

```toml
[workspace.dependencies]
junk-libs-core = { git = "https://github.com/AberrantWolf/junk-libs" }
junk-libs-audio = { git = "https://github.com/AberrantWolf/junk-libs", features = ["decode-compressed", "resample"] }
```

To develop against a local checkout, patch the Git source:

```toml
[patch."https://github.com/AberrantWolf/junk-libs"]
junk-libs-core = { path = "../junk-libs/junk-libs-core" }
junk-libs-audio = { path = "../junk-libs/junk-libs-audio" }
```

## Building

Requires stable Rust with edition 2024.

```sh
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

Tests that need real dumps or external tools are `#[ignore]`d unless their fixtures are provided.

## License

MIT — see [LICENSE](LICENSE).
