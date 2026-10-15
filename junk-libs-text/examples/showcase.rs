//! cargo run -p junk-libs-text --example showcase -- output.png [extra-font.ttf ...]
use junk_libs_text::{TextRenderer, TextStyle};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let output = args.next().ok_or("output PNG path required")?;
    let extra = args.map(std::fs::read).collect::<Result<Vec<_>, _>>()?;
    let mut renderer = TextRenderer::with_fonts(extra);
    let samples = [
        "English — Office AV fi 123",
        "Русский — Привет мир",
        "العربية — مرحباً بالعالم 123",
        "עברית — שלום עולם 123",
        "हिन्दी — नमस्ते दुनिया",
        "ไทย — สวัสดีชาวโลก",
        "Tiếng Việt — Xin chào thế giới",
        "日本語 — 世界へのご挨拶",
        "한국어 — 안녕하세요 세계",
    ];
    let mut image = image::RgbaImage::from_pixel(
        960,
        100 * samples.len() as u32,
        image::Rgba([248, 250, 252, 255]),
    );
    for (i, text) in samples.iter().enumerate() {
        let raster = renderer.render(
            text,
            920,
            90,
            [248, 250, 252, 255],
            [24, 32, 48, 255],
            TextStyle {
                size_px: Some(40.0),
                padding_px: 8.0,
                ..Default::default()
            },
        );
        println!("{text}: {:?}", raster.layout);
        image::imageops::overlay(&mut image, &raster.image, 20, i as i64 * 100);
    }
    image.save(output)?;
    Ok(())
}
