//! Downloads the prebuilt PDFium shared library (bblanchon/pdfium-binaries)
//! into `OUT_DIR` and exports its directory as `PDFIUM_LIB_DIR`, which
//! `instance()` uses to bind PDFium at runtime.
//!
//! The release must match the `pdfium_NNNN` feature of `pdfium-render` in
//! Cargo.toml; a mismatch fails at bind time with undefined symbols.

use std::path::{Path, PathBuf};
use std::{env, fs};

/// Must match the `pdfium_NNNN` feature in Cargo.toml.
const PDFIUM_VERSION: &str = "7763";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let target = env::var("TARGET").unwrap();

    let (platform, arch, lib_name) = match target.as_str() {
        t if t.contains("apple") => (
            "mac",
            if t.contains("aarch64") {
                "arm64"
            } else {
                "x64"
            },
            "libpdfium.dylib",
        ),
        t if t.contains("linux") => (
            "linux",
            if t.contains("aarch64") {
                "arm64"
            } else {
                "x64"
            },
            "libpdfium.so",
        ),
        t if t.contains("windows") => (
            "win",
            if t.contains("aarch64") {
                "arm64"
            } else if t.contains("i686") {
                "x86"
            } else {
                "x64"
            },
            "pdfium.dll",
        ),
        _ => {
            println!("cargo:warning=Unsupported target for PDFium vendoring: {target}");
            return;
        }
    };

    let pdfium_dir = out_dir.join("pdfium");
    // Windows ships the DLL in bin/, everything else in lib/.
    let lib_dir = pdfium_dir.join(if platform == "win" { "bin" } else { "lib" });
    let lib_path = lib_dir.join(lib_name);

    // Tell our own source where to find the library at runtime, regardless of cwd.
    println!("cargo:rustc-env=PDFIUM_LIB_DIR={}", lib_dir.display());

    if lib_path.exists() {
        return; // already vendored for this OUT_DIR
    }

    let url = format!(
        "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium/{PDFIUM_VERSION}/pdfium-{platform}-{arch}.tgz"
    );
    println!("cargo:warning=Downloading PDFium {PDFIUM_VERSION} from {url}");

    fs::create_dir_all(&pdfium_dir).expect("create pdfium dir");
    let tgz = out_dir.join(format!("pdfium-{PDFIUM_VERSION}-{platform}-{arch}.tgz"));
    download(&url, &tgz);
    extract(&tgz, &pdfium_dir);
    let _ = fs::remove_file(&tgz);

    assert!(
        lib_path.exists(),
        "PDFium vendoring failed: {} not found after extraction",
        lib_path.display()
    );
    println!("cargo:warning=PDFium vendored to {}", lib_path.display());
}

fn download(url: &str, dest: &Path) {
    let mut reader = ureq::get(url)
        .call()
        .unwrap_or_else(|e| panic!("download {url}: {e}"))
        .into_body()
        .into_reader();
    let mut file = fs::File::create(dest).expect("create temp tgz");
    std::io::copy(&mut reader, &mut file).expect("write tgz");
}

fn extract(tarball: &Path, dest: &Path) {
    use flate2::read::GzDecoder;
    use tar::Archive;

    let file = fs::File::open(tarball).expect("open tgz");
    let mut archive = Archive::new(GzDecoder::new(file));
    for entry in archive.entries().expect("read tgz entries") {
        let mut entry = entry.expect("tgz entry");
        let kind = entry.header().entry_type();
        // Skip links: the tar crate can fail on them and we don't need them.
        if kind.is_symlink() || kind.is_hard_link() {
            continue;
        }
        let path = entry.path().expect("entry path").to_path_buf();
        let out = dest.join(&path);
        if kind.is_dir() {
            fs::create_dir_all(&out).expect("mkdir");
        } else if kind.is_file() {
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent).expect("mkdir parent");
            }
            entry
                .unpack(&out)
                .unwrap_or_else(|e| panic!("unpack {}: {e}", path.display()));
        }
    }
}
