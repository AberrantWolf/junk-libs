# junk-libs

Shared Rust library infrastructure for the [retro-junk](https://github.com/AberrantWolf/retro-junk) and [phono-junk](https://github.com/AberrantWolf/phono-junk) tools.

Domain-agnostic building blocks only: CD image parsing, streaming hashers, checksum descriptors, and common I/O traits. No retro-game or audio-specific semantics — those live in the consuming projects.

## Crates

- **`junk-libs-core`** — Generic types. `AnalysisError` (thiserror), `MultiHasher` (streaming CRC32/SHA1/MD5), `ChecksumAlgorithm` / `ExpectedChecksum`, multi-disc filename grouping utilities, `ReadSeek` trait alias, byte/ASCII helpers.
- **`junk-libs-disc`** — CD-ROM / optical disc parsing. CUE sheet parser (standard + CDRWin compatibility), CHD reader, ISO 9660 filesystem, CD sector constants, format detection.

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
