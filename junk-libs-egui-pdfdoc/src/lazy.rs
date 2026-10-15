//! Lazy, bounded document loading. Page metadata and source bytes are retained;
//! rasters and text are loaded only on demand. One worker runs at a time, with a
//! single replaceable request. The eager `Document` API remains available.

use crate::{Page, Region};
use anyhow::{Context, Result};
use junk_libs_egui_docview::{PageModel, Rect, RegionId};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, mpsc},
};

#[derive(Clone)]
pub enum PageSource {
    Pdf(Arc<Vec<u8>>),
    Image(Arc<image::RgbaImage>),
}
impl PageSource {
    pub fn render(&self, index: usize, scale: f32) -> Result<Page> {
        match self {
            Self::Pdf(bytes) => {
                let page = junk_libs_platen::render_page_from_bytes(bytes, index, scale)?;
                Ok(Page {
                    size: page.size_pts,
                    bitmap: page.image,
                    char_boxes: page
                        .char_boxes
                        .into_iter()
                        .map(|b| {
                            (
                                b.ch,
                                Rect {
                                    x: b.x,
                                    y: b.y,
                                    w: b.w,
                                    h: b.h,
                                },
                            )
                        })
                        .collect(),
                })
            }
            Self::Image(image) => {
                anyhow::ensure!(index == 0, "page out of range");
                Ok(Page {
                    size: (image.width() as f32, image.height() as f32),
                    bitmap: (**image).clone(),
                    char_boxes: Vec::new(),
                })
            }
        }
    }
}
#[derive(Clone, Copy)]
pub struct PageInfo {
    pub size: (f32, f32),
}

/// Both constraints are enforced. An oversized page is displayed at a lower
/// scale and its text can be extracted independently by a region worker.
#[derive(Clone, Copy)]
pub struct CacheLimits {
    pub pages: usize,
    pub bytes: usize,
}
impl Default for CacheLimits {
    fn default() -> Self {
        Self {
            pages: 3,
            bytes: 64 * 1024 * 1024,
        }
    }
}
struct Cached {
    page: Page,
    stamp: u64,
    bytes: usize,
    scale: u32,
}
struct Job {
    request: (usize, u32),
    rx: mpsc::Receiver<Result<Page>>,
}

pub struct LazyDocument {
    pub pages: Vec<PageInfo>,
    pub regions: Vec<Region>,
    source: PageSource,
    labels: Vec<String>,
    next_id: u64,
    cache: HashMap<usize, Cached>,
    limits: CacheLimits,
    clock: u64,
    job: Option<Job>,
    desired: Option<(usize, u32)>,
}
impl LazyDocument {
    pub fn open(path: &Path) -> Result<Self> {
        let bytes = Arc::new(std::fs::read(path)?);
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
        {
            Self::from_pdf_bytes(bytes, CacheLimits::default())
        } else {
            let bitmap = image::load_from_memory(&bytes)?.into_rgba8();
            let size = (bitmap.width() as f32, bitmap.height() as f32);
            Ok(Self::new(
                PageSource::Image(Arc::new(bitmap)),
                vec![PageInfo { size }],
                Vec::new(),
                CacheLimits::default(),
            ))
        }
    }
    pub fn from_pdf_bytes(bytes: Arc<Vec<u8>>, limits: CacheLimits) -> Result<Self> {
        anyhow::ensure!(
            limits.pages > 0 && limits.bytes >= 1024 * 1024,
            "page cache needs at least one page and 1 MiB"
        );
        let pages = junk_libs_platen::page_sizes_from_bytes(&bytes)?
            .into_iter()
            .map(|size| PageInfo { size })
            .collect::<Vec<_>>();
        anyhow::ensure!(!pages.is_empty(), "document has no pages");
        let labels = junk_libs_platen::metadata::read_from_bytes(&bytes)
            .map(|m| m.page_labels)
            .unwrap_or_default();
        Ok(Self::new(PageSource::Pdf(bytes), pages, labels, limits))
    }
    fn new(
        source: PageSource,
        pages: Vec<PageInfo>,
        labels: Vec<String>,
        limits: CacheLimits,
    ) -> Self {
        Self {
            source,
            pages,
            labels,
            limits,
            regions: Vec::new(),
            next_id: 0,
            cache: HashMap::new(),
            clock: 0,
            job: None,
            desired: None,
        }
    }
    /// A worker snapshot shares encoded/source pixels but retains no viewer
    /// rasters or in-flight jobs. Export may stream pages without growing cache.
    pub fn snapshot(&self) -> Self {
        let mut snapshot = Self::new(
            self.source(),
            self.pages.clone(),
            self.labels.clone(),
            self.limits,
        );
        snapshot.regions = self.regions.clone();
        snapshot.next_id = self.next_id;
        snapshot
    }
    pub fn source(&self) -> PageSource {
        self.source.clone()
    }
    pub fn resident_pages(&self) -> usize {
        self.cache.len()
    }
    pub fn resident_bytes(&self) -> usize {
        self.cache.values().map(|c| c.bytes).sum()
    }
    pub fn is_rendering(&self) -> bool {
        self.job.is_some()
    }
    pub fn bitmap(&self, index: usize) -> Option<&image::RgbaImage> {
        match &self.source {
            PageSource::Image(image) if index == 0 => Some(image),
            _ => self.cache.get(&index).map(|c| &c.page.bitmap),
        }
    }
    pub fn region_mut(&mut self, id: RegionId) -> Option<&mut Region> {
        self.regions.iter_mut().find(|r| r.id == id)
    }
    pub fn add_region(&mut self, page: usize, rect: Rect) -> RegionId {
        let id = RegionId(self.next_id);
        self.next_id += 1;
        self.regions.push(Region { id, page, rect });
        id
    }
    pub fn remove_region(&mut self, id: RegionId) {
        self.regions.retain(|r| r.id != id);
    }
    pub fn next_region_id(&self) -> u64 {
        self.next_id
    }
    pub fn restore_regions(&mut self, regions: Vec<Region>, next: u64) {
        self.regions = regions;
        self.next_id = next;
    }
    pub fn region_text(&self, id: RegionId) -> String {
        self.region_text_with_direction(id, junk_libs_platen::reading_order::Direction::Auto)
    }
    pub fn region_text_with_direction(
        &self,
        id: RegionId,
        direction: junk_libs_platen::reading_order::Direction,
    ) -> String {
        let Some(region) = self.regions.iter().find(|r| r.id == id) else {
            return String::new();
        };
        self.cache
            .get(&region.page)
            .map(|c| crate::text_in_rect_with_direction(&c.page.char_boxes, region.rect, direction))
            .unwrap_or_default()
    }
    pub fn region_input(&self, id: RegionId) -> Result<RegionInput> {
        let region = self
            .regions
            .iter()
            .find(|r| r.id == id)
            .context("region not found")?
            .clone();
        Ok(RegionInput {
            source: self.source(),
            region,
        })
    }
    /// A transient page for export. It never expands the interactive cache.
    pub fn render_page(&self, index: usize, scale: f32) -> Result<Page> {
        self.source.render(index, scale)
    }
    pub fn poll(&mut self) -> Result<()> {
        let Some(job) = &self.job else { return Ok(()) };
        let outcome = match job.rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return Ok(()),
            Err(_) => Err(anyhow::anyhow!("page render worker stopped")),
        };
        let request = job.request;
        self.job = None;
        if self.desired == Some(request) {
            self.desired = None;
            let page = outcome?;
            let bytes = page.bitmap.as_raw().len()
                + page.char_boxes.capacity() * std::mem::size_of::<(char, Rect)>();
            anyhow::ensure!(
                bytes <= self.limits.bytes,
                "page text/bitmap exceeds cache budget"
            );
            self.cache.remove(&request.0);
            while !self.cache.is_empty()
                && (self.cache.len() >= self.limits.pages
                    || self.resident_bytes() + bytes > self.limits.bytes)
            {
                let oldest = *self
                    .cache
                    .iter()
                    .min_by_key(|(_, c)| c.stamp)
                    .expect("nonempty")
                    .0;
                self.cache.remove(&oldest);
            }
            self.clock += 1;
            self.cache.insert(
                request.0,
                Cached {
                    page,
                    stamp: self.clock,
                    bytes,
                    scale: request.1,
                },
            );
        }
        self.start();
        Ok(())
    }
    fn start(&mut self) {
        let Some(request) = self.desired else { return };
        if self.job.is_some() {
            return;
        }
        let source = self.source();
        let size = self.pages[request.0].size;
        // Reserve half the budget for geometry and a previous page while a render
        // is in flight. Platen also caps the maximum raster dimension.
        let max_scale = ((self.limits.bytes as f32 * 0.5) / (4.0 * size.0 * size.1)).sqrt();
        let scale = (request.1 as f32 / 100.0).min(max_scale).max(0.01);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(source.render(request.0, scale));
        });
        self.job = Some(Job { request, rx });
    }
}
impl PageModel for LazyDocument {
    fn page_count(&self) -> usize {
        self.pages.len()
    }
    fn page_label(&self, index: usize) -> Option<&str> {
        self.labels.get(index).map(String::as_str)
    }
    fn page_size(&self, index: usize) -> Option<(f32, f32)> {
        self.pages.get(index).map(|p| p.size)
    }
    fn page_bitmap(&self, index: usize) -> Option<&image::RgbaImage> {
        self.bitmap(index)
    }
    fn rerender_page(&mut self, index: usize, scale: f32) {
        if !scale.is_finite()
            || scale <= 0.0
            || index >= self.pages.len()
            || matches!(self.source, PageSource::Image(_))
        {
            return;
        }
        let key = junk_libs_platen::quantize_render_scale(scale);
        self.clock += 1;
        if let Some(c) = self.cache.get_mut(&index) {
            c.stamp = self.clock;
            if c.scale == key {
                self.desired = None;
                return;
            }
        }
        self.desired = Some((index, key));
        self.start();
    }
    fn regions_on(&self, page: usize) -> Vec<(RegionId, Rect)> {
        self.regions
            .iter()
            .filter(|r| r.page == page)
            .map(|r| (r.id, r.rect))
            .collect()
    }
    fn add_region(&mut self, page: usize, rect: Rect) -> RegionId {
        Self::add_region(self, page, rect)
    }
    fn region_rect_mut(&mut self, id: RegionId) -> Option<&mut Rect> {
        self.region_mut(id).map(|r| &mut r.rect)
    }
    fn remove_region(&mut self, id: RegionId) {
        Self::remove_region(self, id);
    }
}

#[derive(Clone)]
pub struct RegionInput {
    source: PageSource,
    pub region: Region,
}
impl RegionInput {
    /// Fixed-scale crop and source text, resolved on the translation worker. This
    /// works for off-screen/evicted pages and does not depend on viewer zoom.
    pub fn resolve(&self, scale: f32) -> Result<(image::RgbaImage, String)> {
        self.resolve_with_direction(scale, junk_libs_platen::reading_order::Direction::Auto)
    }
    pub fn resolve_with_direction(
        &self,
        scale: f32,
        direction: junk_libs_platen::reading_order::Direction,
    ) -> Result<(image::RgbaImage, String)> {
        let page = self.source.render(self.region.page, scale)?;
        let crop = crate::crop_rect(&page.bitmap, page.size, self.region.rect)?;
        Ok((
            crop,
            crate::text_in_rect_with_direction(&page.char_boxes, self.region.rect, direction),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{Document, Stream, dictionary};
    fn pdf(count: usize) -> Arc<Vec<u8>> {
        let mut doc = Document::with_version("1.5");
        let tree = doc.new_object_id();
        let font =
            doc.add_object(dictionary! {"Type"=>"Font","Subtype"=>"Type1","BaseFont"=>"Helvetica"});
        // Content order deliberately differs from visual reading order.
        let stream = doc.add_object(Stream::new(
            Default::default(),
            b"BT /F 10 Tf 1 0 0 1 10 40 Tm (Second) Tj 1 0 0 1 10 70 Tm (First) Tj ET".to_vec(),
        ));
        let kids: Vec<lopdf::Object> = (0..count)
            .map(|_| {
                doc.add_object(dictionary! {"Type"=>"Page","Parent"=>tree,"Contents"=>stream})
                    .into()
            })
            .collect();
        doc.objects.insert(tree,dictionary!{"Type"=>"Pages","Kids"=>kids,"Count"=>count as i64,"MediaBox"=>vec![0.into(),0.into(),100.into(),100.into()],"Resources"=>dictionary!{"Font"=>dictionary!{"F"=>font}}}.into());
        let root = doc.add_object(dictionary! {"Type"=>"Catalog","Pages"=>tree});
        doc.trailer.set("Root", root);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        Arc::new(bytes)
    }
    fn drain(doc: &mut LazyDocument) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while doc.is_rendering() {
            doc.poll().unwrap();
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    #[test]
    fn opens_hundreds_of_pages_without_rasters_and_bounds_navigation_cache() {
        let mut doc = LazyDocument::from_pdf_bytes(
            pdf(300),
            CacheLimits {
                pages: 2,
                bytes: 1024 * 1024,
            },
        )
        .unwrap();
        assert_eq!(doc.pages.len(), 300);
        assert_eq!(doc.resident_pages(), 0);
        for page in 0..8 {
            doc.rerender_page(page, 4.0);
            drain(&mut doc);
            assert!(doc.resident_pages() <= 2);
            assert!(doc.resident_bytes() <= 1024 * 1024);
            assert!(doc.bitmap(page).is_some());
        }
        assert!(doc.bitmap(0).is_none());
    }
    #[test]
    fn region_input_reads_evicted_pages_at_fixed_resolution_without_filling_cache() {
        let mut doc = LazyDocument::from_pdf_bytes(pdf(5), CacheLimits::default()).unwrap();
        let id = doc.add_region(
            4,
            Rect {
                x: 0.0,
                y: 0.0,
                w: 100.0,
                h: 100.0,
            },
        );
        let input = doc.region_input(id).unwrap();
        let (crop, text) = input.resolve(2.0).unwrap();
        assert_eq!(crop.dimensions(), (200, 200));
        assert_eq!(text, "First\n\nSecond");
        assert_eq!(doc.resident_pages(), 0);
        doc.rerender_page(4, 4.0);
        drain(&mut doc);
        let (other, text2) = doc.region_input(id).unwrap().resolve(2.0).unwrap();
        assert_eq!(crop, other);
        assert_eq!(text, text2);
    }
    #[test]
    fn worker_snapshot_shares_source_and_keeps_independent_geometry_and_cache() {
        let mut doc = LazyDocument::from_pdf_bytes(pdf(3), CacheLimits::default()).unwrap();
        let id = doc.add_region(
            0,
            Rect {
                x: 0.0,
                y: 0.0,
                w: 100.0,
                h: 100.0,
            },
        );
        doc.rerender_page(0, 2.0);
        drain(&mut doc);
        let snapshot = doc.snapshot();
        doc.remove_region(id);
        assert_eq!(snapshot.regions.len(), 1);
        assert_eq!(snapshot.resident_pages(), 0);
        assert!(!snapshot.is_rendering());
        assert_eq!(snapshot.render_page(1, 1.0).unwrap().bitmap.width(), 100);
        assert_eq!(doc.resident_pages(), 1);
    }
    #[test]
    fn coalesces_navigation_and_zoom_requests() {
        let mut doc = LazyDocument::from_pdf_bytes(pdf(4), CacheLimits::default()).unwrap();
        doc.rerender_page(0, 1.0);
        doc.rerender_page(1, 2.0);
        doc.rerender_page(3, 3.0);
        assert_eq!(doc.job.as_ref().unwrap().request, (0, 100));
        assert_eq!(doc.desired, Some((3, 300)));
        drain(&mut doc);
        assert_eq!(doc.resident_pages(), 1);
        assert!(doc.bitmap(3).is_some());
        assert!(doc.bitmap(0).is_none());
    }
}
