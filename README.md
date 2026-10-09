# junk-libs

Shared Rust libraries for [Junk Collector](https://github.com/AberrantWolf/junk-collector), [Retro Junk](https://github.com/AberrantWolf/retro-junk), [Phono Junk](https://github.com/AberrantWolf/phono-junk), [Trash Amp](https://github.com/AberrantWolf/trash-amp), and the document tools.

Use individual crates for reusable parsing, hashing, audio, rendering, and UI primitives. Collection policy, provider access, authorization, and application workflows stay in consumers.

## 📦 Crates

| Crate | Purpose |
|---|---|
| `junk-libs-core` | Streaming CRC32/MD5/SHA-1 hashing, checksum types, reader traits, and filename grouping. |
| `junk-libs-disc` | CUE, CHD, ISO 9660, and redumper parsing; CD sectors and integer PCM readers. |
| `junk-libs-disc-id` | MusicBrainz, FreeDB/CDDB, and AccurateRip disc identifiers from validated TOCs. |
| `junk-libs-accuraterip` | AccurateRip v1/v2 checksums, dBAR parsing, matching, and bounded sample-offset search. |
| `junk-libs-dat` | Streaming Logiqx XML and ClrMamePro DAT parsing. |
| `junk-libs-game-formats` | Reader-based game-dump inspection, normalization, and hashing. |
| `junk-libs-audio` | Opaque byte sources, PCM frame coordinates, decoding, resampling, DSP, and derivative validation. |
| `junk-libs-playback` | Consumer-owned queue keys, deterministic transitions, and generation-fenced seeking. |
| `junk-libs-platen` | PDF rendering and text extraction; pure Rust by default, optional PDFium backend. |
| `junk-libs-pdfium` | PDFium rendering and text extraction; downloads its matching native library at build time. |
| `junk-libs-raster` | GUI-independent raster helpers. |
| `junk-libs-egui-docview` | Zoomable, pageable egui canvas with editable rectangular regions. |
| `junk-libs-egui-pdfdoc` | PDF/image document model for the egui document viewer. |

### Audio features

`junk-libs-audio` has no default features. Enable only what your application needs:

- `decode-wav`: PCM-WAV decoding and exact frame seeking.
- `decode-compressed`: Symphonia codec support, including FLAC; exact seek and truncation checks have an independent FLAC test vector.
- `disc-readers`: adapters for already-open CUE and redumper readers.
- `resample`: bounded streaming sinc resampling. With `decode-compressed`, adds cancellable sequence conversion that retains resampler phase across compatible adjacent sources.
- `derivative-validation`: FLAC integer-PCM comparison and Ogg Opus structure inspection.

Gain and hard clipping are available without optional features. Codec availability does not imply every format is independently qualified; disc identifiers and integrity hashes are distinct from catalog verification.

## 🔧 Build and check

Use a current stable Rust toolchain with edition 2024 support.

```sh
cargo build --workspace --locked
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo fmt --all -- --check
```

Some real-dump and external-tool tests are explicitly ignored unless their fixtures are provisioned.

## Use in another workspace

Depend on individual crates from Git; pin a `rev` or an existing `tag` for reproducible builds:

```toml
[workspace.dependencies]
junk-libs-core = { git = "https://github.com/AberrantWolf/junk-libs" }
junk-libs-audio = { git = "https://github.com/AberrantWolf/junk-libs", features = ["decode-compressed", "resample"] }
```

For local development, override the Git dependencies with a sibling checkout:

```toml
[patch."https://github.com/AberrantWolf/junk-libs"]
junk-libs-core = { path = "../junk-libs/junk-libs-core" }
junk-libs-audio = { path = "../junk-libs/junk-libs-audio" }
```

Add overrides for other crates you edit. Active path overrides require that checkout to exist.

## License

MIT — see [LICENSE](LICENSE).
