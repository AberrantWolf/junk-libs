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
| `junk-libs-text` | Unicode shaping, bidi layout, explicit font fallback and rasterization. |
| `junk-libs-egui-docview` | Zoomable, pageable egui canvas with editable rectangular regions. |
| `junk-libs-egui-pdfdoc` | Eager and bounded lazy PDF/image models for the document viewer. |

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

### DAT parser security update (2026-10-25)

`junk-libs-dat` uses quick-xml 0.41, fixing RUSTSEC-2026-0194/0195. XML text
fragments, predefined/numeric entity events and CDATA are joined before trimming,
with a 64 KiB accumulated-value bound. Attributes retain duplicate checking and
use the nondeprecated normalized-value API. User-defined DTD entities are not
expanded; unknown/malformed references fail parsing. Existing input/token/depth
and entry limits remain in force. Neutral DAT records and consumer authority are
unchanged. Run `cargo test -p junk-libs-dat --all-features --locked` and its
all-targets Clippy check for focused parser acceptance; consumers must also qualify
their catalog import workflows before adopting the new Git revision.

The 2026-10-25 security review additionally enforces the XML read cap per parser
event before buffering, rather than resetting on literal delimiter bytes.
Oversized quoted attributes, comments and CDATA cannot bypass that cap with
embedded `<` or `>` characters. The accumulated decoded-value cap remains separate.

### CHD decoder security update (2026-10-25)

Shared crates require `chd >=0.3.5, <0.4`, bringing in `ruzstd >=0.9.1` for
RUSTSEC-2026-0344. No decoder fork or replacement format mechanism is introduced.
The explicitly provisioned chdman 0.285 oracle exercises `cdzl`, `cdzs` and `zstd`
compression on the same generated mixed data/audio source, checks every unpadded
raw-track digest and rejects truncated CHDs. Run it with
`cargo test -p junk-libs-game-formats --features chd --test chd_oracle --locked -- --ignored`.
These generated vectors qualify codec/byte preservation, not drive or catalog
verification, and do not constitute a full decompression-bomb reproduction.

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

`junk-libs-core::document` supplies bounded PDF-signature/UTF-8 classification and
bounded plain-text preview reads for opaque document attachments. It does not
parse or validate PDF structure, decompress streams or render pages. Consumers
own upload/preview policy and authorization; original bytes remain unchanged.
