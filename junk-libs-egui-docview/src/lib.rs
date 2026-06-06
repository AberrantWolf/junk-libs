//! A reusable, document-format-agnostic egui page-view widget: a zoomable,
//! pageable canvas over which the user draws, selects, moves, resizes, and
//! deletes rectangular regions. It knows nothing about where the pages come from
//! (PDF, images, anything) or what the regions mean — the host supplies both:
//!
//! - a [`PageModel`] (its document, seen through a trait), and
//! - a [`RegionOverlay`] that paints custom content over each region (given an
//!   [`OverlayCtx`] with the per-frame screen geometry and lazy query helpers).
//!
//! All geometry is kept in page points; the page↔screen transform is derived from
//! the displayed image rect each frame, so it's correct at any zoom. The widget
//! owns only its texture, zoom/scroll, and in-progress drag.
//!
//! A batteries-included [`PageModel`] backed by PDFium lives in the sibling
//! `junk-libs-egui-pdfdoc` crate, so consumers needn't write PDF glue themselves.

use egui::{Color32, Pos2, Rect as ERect, Sense, Stroke, Vec2};
use serde::{Deserialize, Serialize};

/// A rectangle in *page coordinates* (e.g. PDF points, origin top-left),
/// independent of zoom/scroll. The widget converts to/from screen space when
/// painting.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Stable identifier for a region, so host state (translations, metadata) can
/// refer to the same region across edits. The host allocates ids in
/// [`PageModel::add_region`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RegionId(pub u64);

/// The host's document, seen through the widget. Geometry is in page points; the
/// widget filters/derives everything else (screen transform, hit-testing) itself.
pub trait PageModel {
    fn page_count(&self) -> usize;
    /// Page size in page points (e.g. PDF points), if the page exists.
    fn page_size(&self, page: usize) -> Option<(f32, f32)>;
    /// The page's rasterized bitmap (RGBA), if rendered.
    fn page_bitmap(&self, page: usize) -> Option<&image::RgbaImage>;
    /// Request that a page's bitmap be (re)rendered at `scale` (pixels per point)
    /// so it stays crisp when zoomed in.
    ///
    /// This is a **fire-and-forget request**, not a synchronous getter. A host may
    /// fulfill it inline (render and update the bitmap before returning) or
    /// asynchronously (kick off a background render and expose the result via
    /// [`page_bitmap`](PageModel::page_bitmap) once ready). The widget calls this
    /// only when the desired `(page, scale)` changes — never every frame — so a
    /// slow or async host is not spammed with duplicate requests. Until a new
    /// bitmap is available, the widget keeps displaying the current one.
    ///
    /// Implementations that can't re-render (e.g. image sources) make this a no-op.
    fn rerender_page(&mut self, page: usize, scale: f32);
    /// The regions on `page`, as `(id, rect)` in page points.
    fn regions_on(&self, page: usize) -> Vec<(RegionId, Rect)>;
    /// Allocate a new region on `page` and return its id.
    fn add_region(&mut self, page: usize, rect: Rect) -> RegionId;
    /// Mutable access to a region's geometry, for move/resize.
    fn region_rect_mut(&mut self, id: RegionId) -> Option<&mut Rect>;
    fn remove_region(&mut self, id: RegionId);
}

/// Host hook to paint custom content over a region (e.g. a translation patch).
/// Return `true` if you drew something opaque over the whole region, so the
/// widget skips its own background fill underneath (the outline/handles still
/// draw on top).
pub trait RegionOverlay {
    fn paint(&mut self, ctx: &OverlayCtx) -> bool;
}

/// Per-region context handed to [`RegionOverlay::paint`]. Carries the per-frame
/// screen geometry the host can't compute itself, plus lazy query helpers
/// (`sample_background` only does work if you call it).
pub struct OverlayCtx<'a> {
    id: RegionId,
    page_rect: Rect,
    screen_rect: ERect,
    painter: &'a egui::Painter,
    egui_ctx: &'a egui::Context,
    page_bitmap: &'a image::RgbaImage,
    bitmap_scale: f32,
}

impl OverlayCtx<'_> {
    /// The region being painted.
    pub fn id(&self) -> RegionId {
        self.id
    }
    /// The region's geometry in page points.
    pub fn page_rect(&self) -> Rect {
        self.page_rect
    }
    /// Where the region sits on screen this frame (post-zoom/scroll). Paint into
    /// this.
    pub fn screen_rect(&self) -> ERect {
        self.screen_rect
    }
    /// Painter clipped to the canvas, for drawing the overlay.
    pub fn painter(&self) -> &egui::Painter {
        self.painter
    }
    /// The egui context, e.g. to upload a texture with `load_texture`.
    pub fn egui_ctx(&self) -> &egui::Context {
        self.egui_ctx
    }
    /// Current page bitmap scale (pixels per point) — useful to rasterize an
    /// overlay at the page's native resolution.
    pub fn bitmap_scale(&self) -> f32 {
        self.bitmap_scale
    }
    /// Average page color just outside `page_rect`, for a background-matched fill.
    /// Computed on demand from the page bitmap — only runs if you call it.
    pub fn sample_background(&self, page_rect: Rect) -> [u8; 3] {
        let s = self.bitmap_scale;
        let px = junk_libs_raster::PixelRect {
            x: (page_rect.x * s).round() as i64,
            y: (page_rect.y * s).round() as i64,
            w: (page_rect.w * s).round() as i64,
            h: (page_rect.h * s).round() as i64,
        };
        let c = junk_libs_raster::sample_background(self.page_bitmap, px);
        [c[0], c[1], c[2]]
    }
}

/// Screen-space size of a corner resize handle, and the grab tolerance.
const HANDLE: f32 = 7.0;
const MIN_ZOOM: f32 = 0.1;
const MAX_ZOOM: f32 = 8.0;
const ZOOM_STEP: f32 = 1.25;
/// Bounds on the page render scale (pixels per point) for on-demand re-rendering.
const MIN_RENDER_SCALE: f32 = 1.0;
const MAX_RENDER_SCALE: f32 = 6.0;
const REGION_STROKE: Color32 = Color32::from_rgb(0xff, 0xa5, 0x00);
const REGION_FILL: Color32 = Color32::from_rgba_premultiplied(0x30, 0x20, 0x00, 0x20);
const ACCENT: Color32 = Color32::from_rgb(0x4e, 0x9a, 0xff);
const ACCENT_FILL: Color32 = Color32::from_rgba_premultiplied(0x12, 0x25, 0x40, 0x30);
/// Highlight for the region/handle the idle pointer is over.
const HOVER: Color32 = Color32::from_rgb(0xff, 0xcc, 0x66);

/// What the pointer is doing between press and release. Coordinates are in page
/// points.
#[derive(Default)]
enum Drag {
    #[default]
    Idle,
    /// Rubber-banding a new region from `start` to `current`.
    Drawing { start: Pos2, current: Pos2 },
    /// Moving an existing region; `grab` is the pointer offset from its corner.
    Moving { id: RegionId, grab: Vec2 },
    /// Resizing a region; `fixed` is the opposite corner that stays put.
    Resizing { id: RegionId, fixed: Pos2 },
}

/// What the pointer is hovering over when idle — drives the highlight + cursor so
/// the user can tell a click will grab a widget rather than draw a new region. It
/// is the same hit-test [`decide_drag`] uses, so the highlight matches the action.
#[derive(Clone, Copy, PartialEq)]
enum Hover {
    None,
    /// Over a region body (a press would move it).
    Body(RegionId),
    /// Over a corner of the selected region (a press would resize it). The index
    /// is the corner: 0=TL, 1=TR, 2=BL, 3=BR.
    Handle(RegionId, usize),
}

/// The interactive page-view widget. Holds only transient view state; the
/// document lives in the host's [`PageModel`].
pub struct DocView {
    texture: Option<egui::TextureHandle>,
    /// Which page `texture` holds, so we rebuild it on page changes.
    texture_page: usize,
    /// Render scale (px/pt) the current texture was built at, so we rebuild it
    /// when the page is re-rendered at a different resolution.
    texture_scale: f32,
    /// On-screen magnification (1.0 = page shown at its native point size).
    zoom: f32,
    /// When true, zoom is recomputed each frame to fit the viewport width.
    fit: bool,
    drag: Drag,
    /// Middle-button panning in progress (drag-scrolls the view, never draws).
    panning: bool,
    /// Last frame's scroll offset, mirrored from the `ScrollArea` so we can drive
    /// it directly when zooming (to anchor the zoom the same frame, no settle lag).
    scroll_offset: Vec2,
    /// The `(page, scale)` of the last [`PageModel::rerender_page`] request, so we
    /// fire it only when the desired target changes — not every frame. This makes
    /// the render seam fire-and-forget: an async host receives one request per
    /// (page, scale) and isn't re-asked while it works. `None` page means nothing
    /// has been requested yet (or state was reset).
    requested_page: Option<usize>,
    requested_scale: f32,
}

impl Default for DocView {
    fn default() -> Self {
        Self {
            texture: None,
            texture_page: 0,
            texture_scale: 0.0,
            zoom: 1.0,
            fit: true,
            drag: Drag::Idle,
            panning: false,
            scroll_offset: Vec2::ZERO,
            requested_page: None,
            requested_scale: 0.0,
        }
    }
}

impl DocView {
    /// Invalidate cached state (e.g. when a new document is opened).
    pub fn reset(&mut self) {
        self.texture = None;
        self.texture_scale = 0.0;
        self.drag = Drag::Idle;
        self.panning = false;
        self.scroll_offset = Vec2::ZERO;
        self.fit = true;
        self.zoom = 1.0;
        self.requested_page = None;
        self.requested_scale = 0.0;
    }

    /// Render the page and handle interaction. `extra_controls` is invoked inside
    /// the control bar so the host can add its own buttons (it should add its own
    /// separator if it wants one). Returns `true` if a region was added, moved,
    /// resized, or deleted this frame (so the caller can mark its state dirty).
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        model: &mut impl PageModel,
        current_page: &mut usize,
        selected: &mut Option<RegionId>,
        overlay: &mut impl RegionOverlay,
        extra_controls: impl FnOnce(&mut egui::Ui),
    ) -> bool {
        let total_pages = model.page_count();
        let mut edited = false;

        // Accumulates this frame's zoom change (from buttons and/or Ctrl+scroll) as
        // a single ratio, applied below to keep a fixed point (cursor or viewport
        // center) pinned while zooming.
        let mut zoom_factor = 1.0;

        // --- control bar: zoom + page navigation ---------------------------
        ui.horizontal(|ui| {
            if ui.button("−").on_hover_text("Zoom out").clicked() {
                let new = (self.zoom / ZOOM_STEP).max(MIN_ZOOM);
                zoom_factor *= new / self.zoom;
                self.zoom = new;
                self.fit = false;
            }
            ui.label(format!("{:.0}%", self.zoom * 100.0));
            if ui.button("+").on_hover_text("Zoom in").clicked() {
                let new = (self.zoom * ZOOM_STEP).min(MAX_ZOOM);
                zoom_factor *= new / self.zoom;
                self.zoom = new;
                self.fit = false;
            }
            if ui.selectable_label(self.fit, "Fit width").clicked() {
                self.fit = true;
            }

            // Host-supplied controls (e.g. a translation-overlay toggle).
            extra_controls(ui);

            if total_pages > 1 {
                ui.separator();
                if ui
                    .add_enabled(*current_page > 0, egui::Button::new("◀"))
                    .clicked()
                {
                    *current_page -= 1;
                }
                ui.label(format!("Page {} / {}", *current_page + 1, total_pages));
                if ui
                    .add_enabled(*current_page + 1 < total_pages, egui::Button::new("▶"))
                    .clicked()
                {
                    *current_page += 1;
                }
            }
        });
        ui.separator();

        let Some(page_size_pts) = model.page_size(*current_page) else {
            self.texture = None;
            ui.centered_and_justified(|ui| {
                ui.label("Open a PDF or image to begin.");
            });
            return edited;
        };
        let page_size = egui::vec2(page_size_pts.0, page_size_pts.1);

        // Resolve the on-screen zoom (fit-to-width / Ctrl+scroll) before rendering.
        if self.fit && page_size.x > 0.0 {
            self.zoom = (ui.available_width() / page_size.x).clamp(MIN_ZOOM, MAX_ZOOM);
        }
        // Ctrl+scroll (and trackpad pinch) zooms. egui routes the ctrl-modified
        // wheel into `zoom_delta` (a multiplicative factor) and keeps it out of the
        // scroll delta, so the ScrollArea below won't also pan — read it directly.
        let zoom_delta = ui.input(|i| i.zoom_delta());
        if (zoom_delta - 1.0).abs() > f32::EPSILON {
            let old = self.zoom;
            self.zoom = (self.zoom * zoom_delta).clamp(MIN_ZOOM, MAX_ZOOM);
            self.fit = false;
            zoom_factor *= self.zoom / old;
        }

        // Request a render of the current page at a scale matching the on-screen
        // size (zoom × device pixels-per-point), keeping it crisp at high zoom.
        // Quantized so scrolling the zoom doesn't change the target every frame.
        // Edge-triggered on the requested `(page, scale)` — not the current
        // bitmap's scale — so the request fires once per target and a slow/async
        // host isn't re-asked while it works (a sync host still lands the new
        // bitmap the same frame, so behavior there is unchanged). A no-op for
        // image sources.
        let ppp = ui.ctx().pixels_per_point();
        let target_scale = render_scale_for(self.zoom, ppp);
        if self.requested_page != Some(*current_page)
            || (self.requested_scale - target_scale).abs() > 0.1
        {
            model.rerender_page(*current_page, target_scale);
            self.requested_page = Some(*current_page);
            self.requested_scale = target_scale;
        }

        // (Re)build the texture when the page changed or its raster scale changed.
        let bitmap_scale = page_scale(&*model, *current_page);
        if self.texture.is_none()
            || self.texture_page != *current_page
            || (self.texture_scale - bitmap_scale).abs() > 0.01
        {
            if let Some(bmp) = model.page_bitmap(*current_page) {
                let size = [bmp.width() as usize, bmp.height() as usize];
                let img = egui::ColorImage::from_rgba_unmultiplied(size, bmp.as_raw());
                self.texture =
                    Some(ui.ctx().load_texture("page", img, egui::TextureOptions::LINEAR));
                self.texture_page = *current_page;
                self.texture_scale = bitmap_scale;
            }
        }
        let Some(texture) = self.texture.as_ref() else {
            return edited;
        };
        let tex_id = texture.id();

        let display = page_size * self.zoom;

        // Zoom keeps a fixed anchor pinned. Rather than nudging the offset *after*
        // layout (which lands a frame late and visibly drifts into place), we drive
        // the `ScrollArea` offset directly so the page is in its final position the
        // same frame. The anchor is the cursor when it's over the view (Ctrl+scroll),
        // else the view's center (zoom buttons / keyboard). With `O` the old offset
        // and `f` the zoom ratio, `O' = f·O + (f−1)·(anchor − viewport.min)` keeps
        // the page-point under the anchor fixed.
        let viewport = ui.available_rect_before_wrap();
        let forced_offset = ((zoom_factor - 1.0).abs() > f32::EPSILON).then(|| {
            let anchor = ui
                .input(|i| i.pointer.hover_pos())
                .filter(|p| viewport.contains(*p))
                .unwrap_or_else(|| viewport.center());
            let target =
                self.scroll_offset * zoom_factor + (anchor - viewport.min) * (zoom_factor - 1.0);
            // Clamp to the real scroll range. A dimension where the page *fits* has
            // an empty range, so its offset pins to 0 (centered). Without this egui
            // would position the content at the raw offset and only clamp afterward
            // — that per-frame shift-then-snap is the jitter.
            let max_off = (display - viewport.size()).max(Vec2::ZERO);
            target.max(Vec2::ZERO).min(max_off)
        });

        let mut area = egui::ScrollArea::both().id_salt("page_scroll").auto_shrink([false; 2]);
        if let Some(off) = forced_offset {
            area = area.scroll_offset(off);
        }
        let out = area.show(ui, |ui| {
            // Lay the content out at least viewport-sized so a page smaller than the
            // view can be centered in it (egui otherwise top-left-justifies); when
            // the page is larger, the extra is the scrollable region. Use the
            // pre-scrollbar viewport size so a fitting page doesn't flicker a bar.
            let content_size = display.max(viewport.size());
            let (content_rect, _) = ui.allocate_exact_size(content_size, Sense::hover());
            let pad = ((content_size - display) * 0.5).max(Vec2::ZERO);
            let rect = ERect::from_min_size(content_rect.min + pad, display);
            ui.painter().image(
                tex_id,
                rect,
                ERect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
            let resp = ui.interact(rect, ui.id().with("page_canvas"), Sense::click_and_drag());

            let scale = if page_size.x > 0.0 {
                rect.width() / page_size.x
            } else {
                1.0
            };
            let to_screen = |p: Pos2| rect.min + p.to_vec2() * scale;
            let to_page = |s: Pos2| ((s - rect.min) / scale).to_pos2();

            // Regions on this page (id + geometry), for hit-testing this frame.
            let regions = model.regions_on(*current_page);

            // --- interaction ---------------------------------------------------
            // Middle-button drag pans the view (grab-scroll), and is handled
            // before region logic so it never starts drawing a region.
            let middle_down = ui.input(|i| i.pointer.middle_down());
            if middle_down && (self.panning || resp.contains_pointer()) {
                self.panning = true;
                // Immediate (un-animated) so the page tracks the cursor 1:1.
                ui.scroll_with_delta_animation(
                    ui.input(|i| i.pointer.delta()),
                    egui::style::ScrollAnimation::none(),
                );
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            } else {
                self.panning = false;
            }

            // Region draw/move/resize is the *primary* button only — the `_by`
            // variants keep middle/secondary buttons out of it.
            // Start from the *press origin*, not interact_pointer_pos: the latter
            // is only sampled once the drag threshold is crossed, which offsets
            // the start by a few pixels from where the user actually clicked.
            if resp.drag_started_by(egui::PointerButton::Primary) {
                if let Some(press) = ui.input(|i| i.pointer.press_origin()) {
                    self.drag = decide_drag(&regions, *selected, to_page(press), scale);
                    match self.drag {
                        Drag::Moving { id, .. } | Drag::Resizing { id, .. } => {
                            *selected = Some(id);
                        }
                        _ => {}
                    }
                }
            }
            if resp.dragged_by(egui::PointerButton::Primary) {
                if let Some(ptr) = resp.interact_pointer_pos() {
                    let pp = to_page(ptr);
                    match &mut self.drag {
                        Drag::Drawing { current, .. } => *current = pp,
                        Drag::Moving { id, grab } => {
                            if let Some(r) = model.region_rect_mut(*id) {
                                r.x = pp.x - grab.x;
                                r.y = pp.y - grab.y;
                                edited = true;
                            }
                        }
                        Drag::Resizing { id, fixed } => {
                            if let Some(r) = model.region_rect_mut(*id) {
                                *r = norm_rect(*fixed, pp);
                                edited = true;
                            }
                        }
                        Drag::Idle => {}
                    }
                }
            }
            if resp.drag_stopped_by(egui::PointerButton::Primary) {
                if let Drag::Drawing { start, current } = self.drag {
                    let r = norm_rect(start, current);
                    if r.w > 2.0 && r.h > 2.0 {
                        *selected = Some(model.add_region(*current_page, r));
                        edited = true;
                    }
                }
                self.drag = Drag::Idle;
            }
            if resp.clicked() {
                if let Some(ptr) = resp.interact_pointer_pos() {
                    *selected = topmost_at(&regions, to_page(ptr));
                }
            }

            // What the idle pointer is over — for the hover highlight + cursor, so
            // the user can tell when a press will grab a region/handle instead of
            // drawing. Skipped mid-drag/pan (the action is already decided).
            let hover = if matches!(self.drag, Drag::Idle) && !self.panning {
                resp.hover_pos()
                    .map(|p| hover_target(&regions, *selected, to_page(p), scale))
                    .unwrap_or(Hover::None)
            } else {
                Hover::None
            };
            if !self.panning {
                match hover {
                    Hover::Handle(_, i) => ui.ctx().set_cursor_icon(if i == 0 || i == 3 {
                        egui::CursorIcon::ResizeNwSe
                    } else {
                        egui::CursorIcon::ResizeNeSw
                    }),
                    Hover::Body(_) => ui.ctx().set_cursor_icon(egui::CursorIcon::Grab),
                    Hover::None if resp.hovered() => {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair)
                    }
                    Hover::None => {}
                }
            }
            if let Some(id) = *selected {
                // Don't treat Delete/Backspace as "delete region" while a text
                // widget (e.g. a host editor) holds keyboard focus — there those
                // keys edit the text.
                let editing = ui.memory(|m| m.focused().is_some());
                let del = !editing
                    && ui.input(|i| {
                        i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)
                    });
                if del {
                    model.remove_region(id);
                    *selected = None;
                    edited = true;
                }
            }

            // --- paint ---------------------------------------------------------
            let painter = ui.painter_at(rect.expand(HANDLE));
            let egui_ctx = ui.ctx().clone();
            // Re-fetch so adds/removes/moves from this frame's interaction show.
            let regions = model.regions_on(*current_page);
            let page_bitmap = model.page_bitmap(*current_page);
            for (id, region_rect) in &regions {
                let id = *id;
                let scr = ERect::from_min_size(
                    to_screen(egui::pos2(region_rect.x, region_rect.y)),
                    egui::vec2(region_rect.w * scale, region_rect.h * scale),
                );
                let is_sel = *selected == Some(id);

                // Let the host paint its overlay (e.g. a translation patch) with the
                // per-frame screen geometry; if it drew, skip our background fill.
                let drew_overlay = if let Some(bmp) = page_bitmap {
                    let octx = OverlayCtx {
                        id,
                        page_rect: *region_rect,
                        screen_rect: scr,
                        painter: &painter,
                        egui_ctx: &egui_ctx,
                        page_bitmap: bmp,
                        bitmap_scale,
                    };
                    overlay.paint(&octx)
                } else {
                    false
                };

                // A region the idle pointer is over (a press would grab it) gets a
                // brighter, heavier outline so accidental grabs are obvious.
                let is_hover_body = matches!(hover, Hover::Body(hid) if hid == id);
                let (stroke_color, fill, stroke_w) = if is_sel {
                    (ACCENT, ACCENT_FILL, 2.0)
                } else if is_hover_body {
                    (HOVER, REGION_FILL, 2.0)
                } else {
                    (REGION_STROKE, REGION_FILL, 1.5)
                };
                if !drew_overlay {
                    painter.rect_filled(scr, 2.0, fill);
                }
                painter.rect_stroke(
                    scr,
                    2.0,
                    Stroke::new(stroke_w, stroke_color),
                    egui::StrokeKind::Inside,
                );
                if is_sel {
                    let corners =
                        [scr.left_top(), scr.right_top(), scr.left_bottom(), scr.right_bottom()];
                    for (i, corner) in corners.iter().enumerate() {
                        // Enlarge + recolor the handle under the pointer.
                        let on = matches!(hover, Hover::Handle(hid, hi) if hid == id && hi == i);
                        let h = ERect::from_center_size(*corner, Vec2::splat(if on { HANDLE * 1.6 } else { HANDLE }));
                        painter.rect_filled(h, 1.0, if on { HOVER } else { ACCENT });
                    }
                }
            }
            if let Drag::Drawing { start, current } = self.drag {
                let scr = ERect::from_two_pos(to_screen(start), to_screen(current));
                painter.rect_stroke(scr, 0.0, Stroke::new(1.5, ACCENT), egui::StrokeKind::Inside);
            }
        });
        // Mirror the (possibly clamped) offset so next frame's zoom math starts
        // from where the view actually is — including manual scrolls and pans.
        self.scroll_offset = out.state.offset;

        edited
    }
}

/// Current render scale (pixels per point) of a page's bitmap.
fn page_scale<M: PageModel + ?Sized>(model: &M, page: usize) -> f32 {
    match (model.page_bitmap(page), model.page_size(page)) {
        (Some(b), Some((w, _))) if w > 0.0 => b.width() as f32 / w,
        _ => 1.0,
    }
}

/// Target render scale (px/pt) for the given on-screen zoom and device
/// pixels-per-point, clamped and quantized to 0.5 steps so zoom scrolling doesn't
/// re-render every frame.
fn render_scale_for(zoom: f32, pixels_per_point: f32) -> f32 {
    let raw = (zoom * pixels_per_point).clamp(MIN_RENDER_SCALE, MAX_RENDER_SCALE);
    (raw * 2.0).round() / 2.0
}

/// Normalize two page-space corners into a non-negative-size [`Rect`].
fn norm_rect(a: Pos2, b: Pos2) -> Rect {
    Rect {
        x: a.x.min(b.x),
        y: a.y.min(b.y),
        w: (a.x - b.x).abs(),
        h: (a.y - b.y).abs(),
    }
}

fn point_in(p: Pos2, r: &Rect) -> bool {
    p.x >= r.x && p.x <= r.x + r.w && p.y >= r.y && p.y <= r.y + r.h
}

/// Topmost region (last drawn) containing `p`, if any.
fn topmost_at(regions: &[(RegionId, Rect)], p: Pos2) -> Option<RegionId> {
    regions
        .iter()
        .rev()
        .find(|(_, r)| point_in(p, r))
        .map(|(id, _)| *id)
}

/// The four corners of a region, in `Hover::Handle` index order (TL, TR, BL, BR).
fn region_corners(r: &Rect) -> [Pos2; 4] {
    [
        egui::pos2(r.x, r.y),
        egui::pos2(r.x + r.w, r.y),
        egui::pos2(r.x, r.y + r.h),
        egui::pos2(r.x + r.w, r.y + r.h),
    ]
}

/// What page-point `pp` is over: a corner handle of the selected region, a region
/// body, or nothing. `regions` is already the current page's regions. This is the
/// single hit-test behind both the hover highlight and [`decide_drag`], so the
/// highlight always matches what a press would do.
fn hover_target(
    regions: &[(RegionId, Rect)],
    selected: Option<RegionId>,
    pp: Pos2,
    scale: f32,
) -> Hover {
    let grab = HANDLE / scale.max(f32::EPSILON); // grab tolerance in page points

    // Resize handle: a corner of the currently selected region takes priority.
    if let Some(id) = selected
        && let Some((_, r)) = regions.iter().find(|(rid, _)| *rid == id)
    {
        for (i, c) in region_corners(r).iter().enumerate() {
            if (pp.x - c.x).abs() <= grab && (pp.y - c.y).abs() <= grab {
                return Hover::Handle(id, i);
            }
        }
    }

    // Body: the topmost region under the pointer.
    if let Some((rid, _)) = regions.iter().rev().find(|(_, r)| point_in(pp, r)) {
        return Hover::Body(*rid);
    }

    Hover::None
}

/// Decide what a press at page-point `pp` begins: resizing a corner of the
/// selected region, moving a region under the pointer, or drawing a new one.
/// Mirrors [`hover_target`]'s hit-test exactly.
fn decide_drag(
    regions: &[(RegionId, Rect)],
    selected: Option<RegionId>,
    pp: Pos2,
    scale: f32,
) -> Drag {
    match hover_target(regions, selected, pp, scale) {
        Hover::Handle(id, i) => {
            // The drag's anchor is the corner opposite the grabbed one.
            const OPPOSITE: [usize; 4] = [3, 2, 1, 0];
            let (_, r) = regions
                .iter()
                .find(|(rid, _)| *rid == id)
                .expect("hover_target returned a live region");
            Drag::Resizing { id, fixed: region_corners(r)[OPPOSITE[i]] }
        }
        Hover::Body(id) => {
            let (_, r) = regions.iter().find(|(rid, _)| *rid == id).expect("live region");
            Drag::Moving { id, grab: egui::vec2(pp.x - r.x, pp.y - r.y) }
        }
        Hover::None => Drag::Drawing { start: pp, current: pp },
    }
}
