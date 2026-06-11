# junk-libs

Shared Rust library infrastructure for the [retro-junk](https://github.com/AberrantWolf/retro-junk), [phono-junk](https://github.com/AberrantWolf/phono-junk), [print-junk](https://github.com/AberrantWolf/print-junk), and expat-junk tools.

Reusable building blocks that carry no app-specific semantics: CD image parsing, streaming hashers, checksum descriptors, common I/O traits, and PDF rendering. Domain meaning (retro-game, audio, document translation) lives in the consuming projects.

## Crates

- **`junk-libs-core`** — Generic types. `AnalysisError` (thiserror), `MultiHasher` (streaming CRC32/SHA1/MD5), `ChecksumAlgorithm` / `ExpectedChecksum`, multi-disc filename grouping utilities, `ReadSeek` trait alias, byte/ASCII helpers.
- **`junk-libs-disc`** — CD-ROM / optical disc parsing. CUE sheet parser (standard + CDRWin compatibility), CHD reader, ISO 9660 filesystem, CD sector constants, format detection.
- **`junk-libs-pdfium`** — GUI-agnostic PDFium render core: rasterize PDF pages to RGBA and extract the text layer as per-character boxes. Its `build.rs` vendors the matching PDFium binary automatically (downloads from bblanchon/pdfium-binaries into `OUT_DIR`), so consumers need no manual setup; bind once per process via `instance()`. Shared by print-junk and expat-junk.
- **`junk-libs-platen`** — Engine-agnostic successor to `junk-libs-pdfium`: same render API (RGBA page bitmaps, per-character text boxes) with the engine behind a feature flag — `backend-hayro` (default, pure Rust, nothing to download or bundle, parallel rendering) or `backend-pdfium` (legacy, kept for A/B comparison during migration). See its SPEC.md.

## Build

```bash
cargo build
cargo test
```

## Consuming this crate

From another Cargo workspace, add via git dependency:

```toml
[workspace.dependencies]
junk-libs-core = { git = "https://github.com/AberrantWolf/junk-libs" }
junk-libs-disc = { git = "https://github.com/AberrantWolf/junk-libs" }
```

For faster local iteration when developing against junk-libs, override with a path dep via Cargo's `[patch]` section in the consuming workspace's root `Cargo.toml`:

```toml
[patch."https://github.com/AberrantWolf/junk-libs"]
junk-libs-core = { path = "../junk-libs/junk-libs-core" }
junk-libs-disc = { path = "../junk-libs/junk-libs-disc" }
```

This requires `junk-libs` to be cloned as a sibling directory. Cargo errors if the path doesn't exist, so either clone both repos side-by-side or leave the patch lines commented out.

## License

MIT — see [LICENSE](LICENSE).
