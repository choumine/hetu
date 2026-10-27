#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! 合图 · 多图拼接工具（Rust 版）
//!
//! 把多张图片按 横排 / 竖排 / 网格 拼成一张大图，每张图按原始像素放置，不做任何缩放。

use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::time::Instant;

use eframe::egui;
use egui::{Color32, CornerRadius, FontId, RichText, Sense, Stroke, Vec2};
use image::{imageops::FilterType, Rgba, RgbaImage};

// ---------------------------------------------------------------------------
// 配色
// ---------------------------------------------------------------------------

const COL_BG: Color32 = Color32::from_rgb(0xF3, 0xF4, 0xF7);
const COL_PANEL: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);
const COL_LINE: Color32 = Color32::from_rgb(0xE7, 0xE9, 0xEF);
const COL_TEXT: Color32 = Color32::from_rgb(0x14, 0x18, 0x21);
const COL_TEXT2: Color32 = Color32::from_rgb(0x4B, 0x55, 0x67);
const COL_MUTED: Color32 = Color32::from_rgb(0x8B, 0x93, 0xA5);
const COL_BRAND: Color32 = Color32::from_rgb(0x25, 0x63, 0xEB);
const COL_BRAND_SOFT: Color32 = Color32::from_rgb(0xEE, 0xF4, 0xFF);
const COL_DANGER: Color32 = Color32::from_rgb(0xDC, 0x26, 0x26);

const PREVIEW_MAX: f32 = 1300.0;
const THUMB_SIDE: u32 = 160;
const MAX_OUTPUT_PIXELS: u64 = 180_000_000;

// ---------------------------------------------------------------------------
// 数据模型
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Horizontal,
    Vertical,
    Grid,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HAlign {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VAlign {
    Top,
    Middle,
    Bottom,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Png,
    Jpg,
    Webp,
}

impl Format {
    fn ext(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpg => "jpg",
            Format::Webp => "webp",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Format::Png => "PNG",
            Format::Jpg => "JPG",
            Format::Webp => "WebP",
        }
    }
}

#[derive(Clone)]
struct Opts {
    mode: Mode,
    cols: usize,
    gap: u32,
    margin: u32,
    halign: HAlign,
    valign: VAlign,
    transparent: bool,
    bg: Color32,
    format: Format,
    quality: u8,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            mode: Mode::Horizontal,
            cols: 2,
            gap: 12,
            margin: 0,
            halign: HAlign::Center,
            valign: VAlign::Middle,
            transparent: false,
            bg: Color32::WHITE,
            format: Format::Png,
            quality: 92,
        }
    }
}

struct Item {
    path: PathBuf,
    name: String,
    width: u32,
    height: u32,
    image: Arc<RgbaImage>,
    thumb: Option<egui::TextureHandle>,
}

struct ExportResult {
    path: PathBuf,
    width: u32,
    height: u32,
    ms: u128,
}

enum ExportState {
    Idle,
    Running(mpsc::Receiver<Result<ExportResult, String>>),
}

enum DialogResult {
    Open(Vec<PathBuf>),
    Save(PathBuf),
}

// ---------------------------------------------------------------------------
// 布局与合成
// ---------------------------------------------------------------------------

/// 返回每张图左上角坐标，以及画布总尺寸。
fn layout(sizes: &[(u32, u32)], o: &Opts) -> (Vec<(u32, u32)>, u32, u32) {
    let n = sizes.len();
    if n == 0 {
        return (Vec::new(), 0, 0);
    }

    let cols = match o.mode {
        Mode::Horizontal => n,
        Mode::Vertical => 1,
        Mode::Grid => o.cols.clamp(1, n),
    };
    let rows = n.div_ceil(cols);

    let mut col_w = vec![0u32; cols];
    let mut row_h = vec![0u32; rows];
    for (i, (w, h)) in sizes.iter().enumerate() {
        let (r, c) = (i / cols, i % cols);
        col_w[c] = col_w[c].max(*w);
        row_h[r] = row_h[r].max(*h);
    }

    let total_w = o.margin * 2
        + col_w.iter().sum::<u32>()
        + o.gap * (cols.saturating_sub(1) as u32);
    let total_h = o.margin * 2
        + row_h.iter().sum::<u32>()
        + o.gap * (rows.saturating_sub(1) as u32);

    let mut col_x = Vec::with_capacity(cols);
    let mut acc = o.margin;
    for c in 0..cols {
        col_x.push(acc);
        acc += col_w[c] + o.gap;
    }
    let mut row_y = Vec::with_capacity(rows);
    let mut acc = o.margin;
    for r in 0..rows {
        row_y.push(acc);
        acc += row_h[r] + o.gap;
    }

    let mut pos = Vec::with_capacity(n);
    for (i, (w, h)) in sizes.iter().enumerate() {
        let (r, c) = (i / cols, i % cols);
        let mut x = col_x[c];
        let mut y = row_y[r];
        match o.halign {
            HAlign::Center => x += (col_w[c] - w) / 2,
            HAlign::Right => x += col_w[c] - w,
            HAlign::Left => {}
        }
        match o.valign {
            VAlign::Middle => y += (row_h[r] - h) / 2,
            VAlign::Bottom => y += row_h[r] - h,
            VAlign::Top => {}
        }
        pos.push((x, y));
    }

    (pos, total_w, total_h)
}

fn bg_rgba(o: &Opts) -> Rgba<u8> {
    if o.transparent {
        Rgba([0, 0, 0, 0])
    } else {
        Rgba([o.bg.r(), o.bg.g(), o.bg.b(), 255])
    }
}

/// 合成整张图。`max_side` 为 Some 时按比例缩小（预览用），None 表示原始分辨率。
fn compose(items: &[Item], o: &Opts, max_side: Option<f32>) -> (RgbaImage, u32, u32) {
    let sizes: Vec<(u32, u32)> = items.iter().map(|i| (i.width, i.height)).collect();
    let (pos, total_w, total_h) = layout(&sizes, o);

    let scale = match max_side {
        Some(limit) => (limit / total_w.max(total_h).max(1) as f32).min(1.0),
        None => 1.0,
    };

    let out_w = ((total_w as f32 * scale).round() as u32).max(1);
    let out_h = ((total_h as f32 * scale).round() as u32).max(1);
    let mut canvas = RgbaImage::from_pixel(out_w, out_h, bg_rgba(o));

    for (item, (x, y)) in items.iter().zip(pos.iter()) {
        let w = ((item.width as f32 * scale).round() as u32).max(1);
        let h = ((item.height as f32 * scale).round() as u32).max(1);

        let placed = if (w, h) == (item.width, item.height) {
            (*item.image).clone()
        } else {
            image::imageops::resize(&*item.image, w, h, FilterType::Triangle)
        };

        let px = (*x as f32 * scale).round() as i64;
        let py = (*y as f32 * scale).round() as i64;
        image::imageops::overlay(&mut canvas, &placed, px, py);
    }

    (canvas, total_w, total_h)
}

fn save_image(canvas: &RgbaImage, path: &Path, format: Format, quality: u8) -> Result<(), String> {
    match format {
        Format::Png | Format::Webp => canvas.save(path).map_err(|e| format!("保存失败：{e}")),
        Format::Jpg => {
            let (w, h) = canvas.dimensions();
            let mut flat = RgbaImage::from_pixel(w, h, Rgba([255, 255, 255, 255]));
            image::imageops::overlay(&mut flat, canvas, 0, 0);
            let rgb = image::DynamicImage::ImageRgba8(flat).to_rgb8();

            let file = std::fs::File::create(path).map_err(|e| format!("创建文件失败：{e}"))?;
            let writer = std::io::BufWriter::new(file);
            let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(writer, quality);
            encoder
                .encode_image(&rgb)
                .map_err(|e| format!("编码失败：{e}"))
        }
    }
}

// ---------------------------------------------------------------------------
// 素材加载
// ---------------------------------------------------------------------------

fn load_item(path: PathBuf) -> Result<Item, String> {
    let img = image::open(&path).map_err(|e| format!("无法读取 {}：{e}", path.display()))?;
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string());

    Ok(Item {
        path,
        name,
        width,
        height,
        image: Arc::new(rgba),
        thumb: None,
    })
}

fn item_from_rgba(rgba: RgbaImage, name: String) -> Item {
    let (width, height) = rgba.dimensions();
    Item {
        path: PathBuf::new(),
        name,
        width,
        height,
        image: Arc::new(rgba),
        thumb: None,
    }
}

fn dbg_log(msg: &str) {
    if !cfg!(debug_assertions) {
        return;
    }
    use std::io::Write;
    let path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("debug.log")))
        .unwrap_or_else(|| PathBuf::from("debug.log"));
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{msg}");
    }
}

fn ensure_thumb(item: &mut Item, ctx: &egui::Context) {
    if item.thumb.is_some() {
        return;
    }
    let scale = (THUMB_SIDE as f32 / item.width.max(item.height).max(1) as f32).min(1.0);
    let w = ((item.width as f32 * scale) as u32).max(1);
    let h = ((item.height as f32 * scale) as u32).max(1);
    let small = image::imageops::resize(&*item.image, w, h, FilterType::Triangle);
    let color = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], small.as_raw());
    item.thumb = Some(ctx.load_texture(
        format!("thumb-{}-{}", item.name, w),
        color,
        egui::TextureOptions::LINEAR,
    ));
}

/// 在独立线程里弹原生文件对话框（避免线程 COM 模型问题）。
fn spawn_pick_dialog() -> mpsc::Receiver<Option<DialogResult>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let files = rfd::FileDialog::new()
            .set_title("选择要拼接的图片")
            .add_filter(
                "图片文件",
                &["png", "jpg", "jpeg", "bmp", "webp", "gif", "tif", "tiff"],
            )
            .add_filter("所有文件", &["*"])
            .pick_files();
        let _ = tx.send(files.map(DialogResult::Open));
    });
    rx
}

fn paste_clipboard() -> Option<RgbaImage> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| -> Option<RgbaImage> {
            let mut cb = arboard::Clipboard::new().ok()?;
            let img = cb.get_image().ok()?;
            RgbaImage::from_raw(img.width as u32, img.height as u32, img.bytes.into_owned())
        })();
        let _ = tx.send(result);
    });
    rx.recv().ok().flatten()
}

// ---------------------------------------------------------------------------
// 应用
// ---------------------------------------------------------------------------

struct App {
    items: Vec<Item>,
    opts: Opts,
    preview: Option<egui::TextureHandle>,
    out_size: (u32, u32),
    preview_ms: u128,
    dirty: bool,
    dragging: Option<usize>,
    status: String,
    status_is_error: bool,
    export: ExportState,
    dialog: Option<mpsc::Receiver<Option<DialogResult>>>,
    last_export_dir: Option<PathBuf>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_fonts(&cc.egui_ctx);
        setup_style(&cc.egui_ctx);

        Self {
            items: Vec::new(),
            opts: Opts::default(),
            preview: None,
            out_size: (0, 0),
            preview_ms: 0,
            dirty: true,
            dragging: None,
            status: String::new(),
            status_is_error: false,
            export: ExportState::Idle,
            dialog: None,
            last_export_dir: None,
        }
    }

    fn set_status(&mut self, msg: impl Into<String>, is_error: bool) {
        self.status = msg.into();
        self.status_is_error = is_error;
    }

    fn add_paths(&mut self, paths: Vec<PathBuf>) {
        dbg_log(&format!("add_paths: {} items", paths.len()));
        let mut added = 0;
        for path in paths {
            match load_item(path) {
                Ok(item) => {
                    self.items.push(item);
                    added += 1;
                }
                Err(err) => {
                    dbg_log(&format!("load fail: {err}"));
                    self.set_status(err, true);
                }
            }
        }
        if added > 0 {
            self.set_status(format!("已添加 {added} 张图片"), false);
            self.dirty = true;
        }
    }

    fn update_preview(&mut self, ctx: &egui::Context) {
        if !self.dirty {
            return;
        }
        self.dirty = false;

        if self.items.is_empty() {
            self.preview = None;
            self.out_size = (0, 0);
            return;
        }

        let started = Instant::now();
        let (canvas, tw, th) = compose(&self.items, &self.opts, Some(PREVIEW_MAX));
        let color = egui::ColorImage::from_rgba_unmultiplied(
            [canvas.width() as usize, canvas.height() as usize],
            canvas.as_raw(),
        );
        self.preview = Some(ctx.load_texture("preview", color, egui::TextureOptions::LINEAR));
        self.out_size = (tw, th);
        self.preview_ms = started.elapsed().as_millis();
    }

    fn start_export(&mut self) {
        if self.items.is_empty() {
            return;
        }

        let sizes: Vec<(u32, u32)> = self.items.iter().map(|i| (i.width, i.height)).collect();
        let (_, tw, th) = layout(&sizes, &self.opts);
        if tw as u64 * th as u64 > MAX_OUTPUT_PIXELS {
            let msg = format!(
                "输出尺寸 {tw}×{th}（约 {:.1} 亿像素）过大，换网格布局或减少图片后再试",
                tw as f64 * th as f64 / 100_000_000.0
            );
            self.set_status(msg, true);
            return;
        }

        let default_name = format!("合图_{}.{}", timestamp(), self.opts.format.ext());

        let fmt = self.opts.format;
        let dir = self.last_export_dir.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut dialog = rfd::FileDialog::new()
                .set_title("保存拼接结果")
                .set_file_name(&default_name)
                .add_filter(fmt.label(), &[fmt.ext()]);
            if let Some(dir) = dir {
                dialog = dialog.set_directory(dir);
            }
            let _ = tx.send(dialog.save_file().map(DialogResult::Save));
        });
        self.dialog = Some(rx);
    }

    fn begin_export(&mut self, path: PathBuf) {
        let mut path = path;
        if path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_lowercase())
            != Some(self.opts.format.ext().to_string())
        {
            path.set_extension(self.opts.format.ext());
        }

        let opts = self.opts.clone();
        let items: Vec<Item> = self
            .items
            .iter()
            .map(|i| Item {
                path: i.path.clone(),
                name: i.name.clone(),
                width: i.width,
                height: i.height,
                image: Arc::clone(&i.image),
                thumb: None,
            })
            .collect();

        let (tx, rx) = mpsc::channel();
        let save_path = path.clone();
        std::thread::spawn(move || {
            let started = Instant::now();
            let (canvas, tw, th) = compose(&items, &opts, None);
            let result = save_image(&canvas, &save_path, opts.format, opts.quality).map(|_| {
                ExportResult {
                    path: save_path,
                    width: tw,
                    height: th,
                    ms: started.elapsed().as_millis(),
                }
            });
            let _ = tx.send(result);
        });

        self.export = ExportState::Running(rx);
        self.set_status("正在导出…", false);
    }

    fn poll_export(&mut self) {
        let done = if let ExportState::Running(rx) = &self.export {
            match rx.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err("导出线程异常结束".to_string()))
                }
            }
        } else {
            None
        };

        if let Some(result) = done {
            self.export = ExportState::Idle;
            match result {
                Ok(res) => {
                    self.last_export_dir = res.path.parent().map(|p| p.to_path_buf());
                    self.set_status(
                        format!(
                            "已保存 {}×{} · {} · {}",
                            res.width,
                            res.height,
                            fmt_ms(res.ms),
                            res.path.display()
                        ),
                        false,
                    );
                }
                Err(err) => {
                    dbg_log(&format!("load fail: {err}"));
                    self.set_status(err, true);
                }
            }
        }
    }

    fn poll_dialog(&mut self) {
        let msg = match &self.dialog {
            Some(rx) => match rx.try_recv() {
                Ok(v) => Some(v),
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => Some(None),
            },
            None => return,
        };
        self.dialog = None;
        match msg {
            Some(Some(DialogResult::Open(files))) => {
                dbg_log(&format!("dialog open: {} files", files.len()));
                if !files.is_empty() {
                    self.add_paths(files);
                }
            }
            Some(Some(DialogResult::Save(path))) => {
                dbg_log("dialog save path");
                self.begin_export(path);
            }
            Some(None) => dbg_log("dialog cancelled"),
            None => dbg_log("dialog disconnected"),
        }
    }

    fn remove_item(&mut self, index: usize) {
        if index < self.items.len() {
            self.items.remove(index);
            self.dirty = true;
        }
    }

    fn move_item(&mut self, from: usize, to: usize) {
        if from < self.items.len() && to < self.items.len() && from != to {
            let item = self.items.remove(from);
            self.items.insert(to, item);
            self.dirty = true;
        }
    }

    fn export_running(&self) -> bool {
        matches!(self.export, ExportState::Running(_))
    }
}

fn fmt_ms(ms: u128) -> String {
    if ms < 1000 {
        format!("{ms} ms")
    } else {
        format!("{:.2} s", ms as f64 / 1000.0)
    }
}

fn fmt_pixels(n: u64) -> String {
    if n >= 100_000_000 {
        format!("{:.2} 亿", n as f64 / 100_000_000.0)
    } else if n >= 10_000 {
        format!("{:.1} 万", n as f64 / 10_000.0)
    } else {
        n.to_string()
    }
}

/// 生成 20261027_081530 这样的时间戳（北京时间），不引入 chrono。
fn timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        + 8 * 3600;

    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}{mo:02}{d:02}_{h:02}{m:02}{s:02}")
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ---------------------------------------------------------------------------
// 样式
// ---------------------------------------------------------------------------

fn setup_fonts(ctx: &egui::Context) {
    let candidates = [
        "C:/Windows/Fonts/simhei.ttf",
        "C:/Windows/Fonts/Deng.ttf",
        "C:/Windows/Fonts/msyh.ttc",
        "C:/Windows/Fonts/simsun.ttc",
    ];

    let mut fonts = egui::FontDefinitions::default();
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert(
                "cjk".to_owned(),
                Arc::new(egui::FontData::from_owned(bytes)),
            );
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts
                    .families
                    .entry(family)
                    .or_default()
                    .insert(0, "cjk".to_owned());
            }
            break;
        }
    }
    ctx.set_fonts(fonts);
}

fn setup_style(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Light);
    ctx.all_styles_mut(|style| {
    style.visuals = egui::Visuals::light();
    style.visuals.panel_fill = COL_BG;
    style.visuals.window_fill = COL_PANEL;
    style.visuals.extreme_bg_color = COL_PANEL;
    style.visuals.faint_bg_color = Color32::from_rgb(0xF7, 0xF8, 0xFB);
    style.visuals.override_text_color = Some(COL_TEXT);

    let w = &mut style.visuals.widgets;
    w.noninteractive.bg_fill = COL_PANEL;
    w.noninteractive.bg_stroke = Stroke::new(1.0, COL_LINE);
    w.noninteractive.fg_stroke = Stroke::new(1.0, COL_TEXT);
    w.noninteractive.corner_radius = CornerRadius::same(10);

    w.inactive.bg_fill = Color32::from_rgb(0xE2, 0xE6, 0xEE);
    w.inactive.weak_bg_fill = COL_PANEL;
    w.inactive.bg_stroke = Stroke::new(1.0, COL_LINE);
    w.inactive.fg_stroke = Stroke::new(1.0, COL_TEXT2);
    w.inactive.corner_radius = CornerRadius::same(9);

    w.hovered.bg_fill = Color32::from_rgb(0xD8, 0xDE, 0xE9);
    w.hovered.weak_bg_fill = Color32::from_rgb(0xF7, 0xF8, 0xFB);
    w.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0xD3, 0xD8, 0xE2));
    w.hovered.fg_stroke = Stroke::new(1.0, COL_TEXT);
    w.hovered.corner_radius = CornerRadius::same(9);

    w.active.bg_fill = Color32::from_rgb(0xEF, 0xF1, 0xF6);
    w.active.weak_bg_fill = Color32::from_rgb(0xEF, 0xF1, 0xF6);
    w.active.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0xC9, 0xD2, 0xE0));
    w.active.fg_stroke = Stroke::new(1.0, COL_TEXT);
    w.active.corner_radius = CornerRadius::same(9);

    style.visuals.selection.bg_fill = COL_BRAND;
    style.visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(12.0, 7.0);
    style.spacing.slider_width = 140.0;

    });
}

// ---------------------------------------------------------------------------
// UI
// ---------------------------------------------------------------------------

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_export();
        self.poll_dialog();

        let ctx = ui.ctx().clone();
        for item in &mut self.items {
            ensure_thumb(item, &ctx);
        }
        self.update_preview(&ctx);

        self.top_bar(ui);
        self.bottom_bar(ui);
        self.left_panel(ui);
        self.right_panel(ui);
        self.center_panel(ui);

        if self.export_running() || self.dialog.is_some() {
            ctx.request_repaint();
        }
    }
}

impl App {
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        egui::containers::Panel::top("top")
            .frame(
                egui::Frame::NONE
                    .fill(COL_PANEL)
                    .inner_margin(egui::Margin {
                        left: 18,
                        right: 18,
                        top: 0,
                        bottom: 0,
                    }),
            )
            .exact_size(64.0)
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::hover());
                    ui.painter()
                        .rect_filled(rect, CornerRadius::same(10), COL_BRAND);
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "合",
                        FontId::proportional(19.0),
                        Color32::WHITE,
                    );
                    ui.add_space(10.0);
                    ui.vertical(|ui| {
                        ui.add_space(3.0);
                        ui.label(RichText::new("合图").size(16.0).strong().color(COL_TEXT));
                        ui.label(
                            RichText::new("多图拼接 · 每张图保持原始分辨率")
                                .size(11.0)
                                .color(COL_MUTED),
                        );
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let running = self.export_running();
                        let text = if running { "导出中…" } else { "导出图片" };
                        let btn =
                            egui::Button::new(RichText::new(text).size(13.0).color(Color32::WHITE))
                                .fill(COL_BRAND)
                                .corner_radius(10)
                                .min_size(Vec2::new(96.0, 34.0));

                        if ui
                            .add_enabled(!self.items.is_empty() && !running && self.dialog.is_none(), btn)
                            .clicked()
                        {
                            self.start_export();
                        }
                    });
                });
            });
    }

    fn bottom_bar(&mut self, ui: &mut egui::Ui) {
        egui::containers::Panel::bottom("status")
            .frame(
                egui::Frame::NONE
                    .fill(COL_BG)
                    .inner_margin(egui::Margin {
                        left: 14,
                        right: 14,
                        top: 6,
                        bottom: 8,
                    }),
            )
            .exact_size(30.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let color = if self.status_is_error {
                        COL_DANGER
                    } else {
                        COL_MUTED
                    };
                    ui.label(RichText::new(&self.status).size(11.5).color(color));
                });
            });
    }

    fn left_panel(&mut self, ui: &mut egui::Ui) {
        egui::containers::Panel::left("list")
            .exact_size(272.0)
            .resizable(false)
            .frame(
                egui::Frame::NONE
                    .fill(COL_PANEL)
                    .stroke(Stroke::new(1.0, COL_LINE))
                    .inner_margin(egui::Margin::same(10)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("图片").size(12.5).strong().color(COL_TEXT2));
                    let badged = format!("{}", self.items.len());
                    let (rect, _) = ui.allocate_exact_size(Vec2::new(20.0, 18.0), Sense::hover());
                    ui.painter()
                        .rect_filled(rect, CornerRadius::same(9), COL_BRAND_SOFT);
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        badged,
                        FontId::proportional(11.0),
                        COL_BRAND,
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                !self.items.is_empty(),
                                egui::Button::new(
                                    RichText::new("清空").size(11.5).color(COL_MUTED),
                                )
                                .frame(false),
                            )
                            .clicked()
                        {
                            self.items.clear();
                            self.dirty = true;
                            self.set_status("已清空", false);
                        }
                    });
                });
                ui.add_space(4.0);

                let mut action: Option<(usize, usize, bool)> = None;
                let list_height = (ui.available_height() - 96.0).max(80.0);

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .max_height(list_height)
                    .show(ui, |ui| {
                        for i in 0..self.items.len() {
                            let item = &self.items[i];
                            let thumb_id = item.thumb.as_ref().map(|t| t.id());
                            let name = item.name.clone();
                            let size_text = format!("{} × {}", item.width, item.height);

                            let (rect, resp) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), 58.0),
                                Sense::click_and_drag(),
                            );

                            let hovered = resp.hovered();
                            let is_dragging = self.dragging == Some(i);
                            let bg = if is_dragging {
                                COL_BRAND_SOFT
                            } else if hovered {
                                Color32::from_rgb(0xF7, 0xF8, 0xFB)
                            } else {
                                Color32::TRANSPARENT
                            };
                            ui.painter().rect_filled(rect, CornerRadius::same(10), bg);

                            let thumb_rect = egui::Rect::from_min_size(
                                rect.min + Vec2::new(7.0, 6.0),
                                Vec2::splat(46.0),
                            );
                            ui.painter().rect_filled(
                                thumb_rect,
                                CornerRadius::same(8),
                                Color32::from_rgb(0xF2, 0xF3, 0xF7),
                            );
                            if let Some(id) = thumb_id {
                                let uv = egui::Rect::from_min_max(
                                    egui::pos2(0.0, 0.0),
                                    egui::pos2(1.0, 1.0),
                                );
                                ui.painter().image(id, thumb_rect, uv, Color32::WHITE);
                            }

                            let badge_rect = egui::Rect::from_min_size(
                                thumb_rect.min + Vec2::new(2.0, 2.0),
                                Vec2::new(18.0, 16.0),
                            );
                            ui.painter().rect_filled(
                                badge_rect,
                                CornerRadius::same(5),
                                Color32::from_rgba_unmultiplied(17, 24, 39, 190),
                            );
                            ui.painter().text(
                                badge_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                format!("{}", i + 1),
                                FontId::proportional(10.0),
                                Color32::WHITE,
                            );

                            let text_x = thumb_rect.right() + 8.0;
                            let text_width = rect.right() - text_x - 28.0;
                            ui.painter().text(
                                egui::pos2(text_x, rect.center().y - 8.0),
                                egui::Align2::LEFT_CENTER,
                                truncate(&name, text_width, 12.0),
                                FontId::proportional(12.0),
                                COL_TEXT,
                            );
                            ui.painter().text(
                                egui::pos2(text_x, rect.center().y + 9.0),
                                egui::Align2::LEFT_CENTER,
                                &size_text,
                                FontId::proportional(11.0),
                                COL_MUTED,
                            );

                            if hovered {
                                let del_rect = egui::Rect::from_center_size(
                                    egui::pos2(rect.right() - 16.0, rect.center().y),
                                    Vec2::splat(20.0),
                                );
                                let del_resp = ui.interact(
                                    del_rect,
                                    egui::Id::new(("del", i)),
                                    Sense::click(),
                                );
                                if del_resp.hovered() {
                                    ui.painter().rect_filled(
                                        del_rect,
                                        CornerRadius::same(6),
                                        Color32::from_rgb(0xFE, 0xF2, 0xF2),
                                    );
                                }
                                ui.painter().text(
                                    del_rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    "×",
                                    FontId::proportional(14.0),
                                    COL_MUTED,
                                );
                                if del_resp.clicked() {
                                    action = Some((i, i, true));
                                }
                            }

                            if resp.drag_started() {
                                self.dragging = Some(i);
                            }
                            if let Some(from) = self.dragging {
                                if resp.hovered() && from != i {
                                    action = Some((from, i, false));
                                }
                            }
                        }

                        if self.items.is_empty() {
                            ui.add_space(40.0);
                            ui.vertical_centered(|ui| {
                                ui.label(RichText::new("还没有图片").size(12.5).color(COL_TEXT2));
                                ui.label(
                                    RichText::new("点下面的按钮开始").size(11.5).color(COL_MUTED),
                                );
                            });
                        }
                    });

                if ui.ctx().input(|i| i.pointer.any_released()) {
                    self.dragging = None;
                }

                if let Some((from, to, remove)) = action {
                    if remove {
                        self.remove_item(from);
                    } else {
                        self.move_item(from, to);
                        self.dragging = Some(to);
                    }
                }

                ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                    ui.add_space(4.0);
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new("粘贴截图 / 复制的图片")
                                    .size(12.0)
                                    .color(COL_TEXT2),
                            )
                            .fill(Color32::from_rgb(0xF6, 0xF7, 0xFA))
                            .corner_radius(9)
                            .min_size(Vec2::new(ui.available_width(), 32.0)),
                        )
                        .clicked()
                    {
                        if let Some(img) = paste_clipboard() {
                            let name = format!("剪贴板_{}.png", timestamp());
                            self.items.push(item_from_rgba(img, name));
                            self.dirty = true;
                            self.set_status("已粘贴 1 张图片", false);
                        } else {
                            self.set_status("剪贴板里没有图片", true);
                        }
                    }

                    ui.add_space(6.0);
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new("＋ 添加图片").size(12.5).color(COL_TEXT2),
                            )
                            .fill(COL_PANEL)
                            .stroke(Stroke::new(1.0, COL_LINE))
                            .corner_radius(9)
                            .min_size(Vec2::new(ui.available_width(), 34.0)),
                        )
                        .clicked()
                    {
                        dbg_log("btn add clicked");
                        self.dialog = Some(spawn_pick_dialog());
                    }
                });
            });
    }

    fn right_panel(&mut self, ui: &mut egui::Ui) {
        egui::containers::Panel::right("settings")
            .exact_size(300.0)
            .resizable(false)
            .frame(
                egui::Frame::NONE
                    .fill(COL_PANEL)
                    .stroke(Stroke::new(1.0, COL_LINE))
                    .inner_margin(egui::Margin::same(14)),
            )
            .show(ui, |ui| {
                ui.label(
                    RichText::new("拼接设置")
                        .size(12.5)
                        .strong()
                        .color(COL_TEXT2),
                );
                ui.add_space(6.0);

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.label(RichText::new("布局方式").size(12.0).color(COL_TEXT2));
                        ui.horizontal(|ui| {
                            let width = (ui.available_width() - 10.0) / 3.0;
                            for (mode, label) in [
                                (Mode::Horizontal, "横排"),
                                (Mode::Vertical, "竖排"),
                                (Mode::Grid, "网格"),
                            ] {
                                let selected = self.opts.mode == mode;
                                if ui
                                    .add(
                                        egui::Button::new(RichText::new(label).size(12.0).color(
                                            if selected { COL_TEXT } else { COL_MUTED },
                                        ))
                                        .fill(if selected {
                                            COL_PANEL
                                        } else {
                                            Color32::from_rgb(0xF2, 0xF3, 0xF7)
                                        })
                                        .stroke(if selected {
                                            Stroke::new(1.0, COL_LINE)
                                        } else {
                                            Stroke::NONE
                                        })
                                        .corner_radius(8)
                                        .min_size(Vec2::new(width, 28.0)),
                                    )
                                    .clicked()
                                    && !selected
                                {
                                    self.opts.mode = mode;
                                    self.dirty = true;
                                }
                            }
                        });

                        if self.opts.mode == Mode::Grid {
                            ui.add_space(6.0);
                            let max_cols = self.items.len().clamp(1, 12);
                            let mut cols = self.opts.cols.min(max_cols);
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("每行张数").size(12.0).color(COL_TEXT2));
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            RichText::new(format!("{cols}"))
                                                .size(11.5)
                                                .strong()
                                                .color(COL_TEXT),
                                        );
                                    },
                                );
                            });
                            if ui
                                .add(egui::Slider::new(&mut cols, 1..=max_cols).show_value(false))
                                .changed()
                            {
                                self.opts.cols = cols;
                                self.dirty = true;
                            }
                        }

                        ui.add_space(8.0);
                        self.number_row(ui, "图片间距", "gap", 0, 200);
                        ui.add_space(6.0);
                        self.number_row(ui, "外边距", "margin", 0, 200);

                        ui.add_space(10.0);
                        ui.separator();
                        ui.add_space(6.0);

                        ui.label(RichText::new("单元格内对齐").size(12.0).color(COL_TEXT2));
                        ui.add_space(2.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("水平").size(11.0).color(COL_MUTED));
                            ui.add_space(4.0);
                            for (align, label) in [
                                (HAlign::Left, "左"),
                                (HAlign::Center, "中"),
                                (HAlign::Right, "右"),
                            ] {
                                let selected = self.opts.halign == align;
                                if ui
                                    .add(
                                        egui::Button::new(RichText::new(label).size(11.5).color(
                                            if selected { COL_BRAND } else { COL_MUTED },
                                        ))
                                        .fill(if selected {
                                            COL_BRAND_SOFT
                                        } else {
                                            Color32::TRANSPARENT
                                        })
                                        .stroke(Stroke::NONE)
                                        .corner_radius(7)
                                        .min_size(Vec2::new(30.0, 24.0)),
                                    )
                                    .clicked()
                                {
                                    self.opts.halign = align;
                                    self.dirty = true;
                                }
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("垂直").size(11.0).color(COL_MUTED));
                            ui.add_space(4.0);
                            for (align, label) in [
                                (VAlign::Top, "上"),
                                (VAlign::Middle, "中"),
                                (VAlign::Bottom, "下"),
                            ] {
                                let selected = self.opts.valign == align;
                                if ui
                                    .add(
                                        egui::Button::new(RichText::new(label).size(11.5).color(
                                            if selected { COL_BRAND } else { COL_MUTED },
                                        ))
                                        .fill(if selected {
                                            COL_BRAND_SOFT
                                        } else {
                                            Color32::TRANSPARENT
                                        })
                                        .stroke(Stroke::NONE)
                                        .corner_radius(7)
                                        .min_size(Vec2::new(30.0, 24.0)),
                                    )
                                    .clicked()
                                {
                                    self.opts.valign = align;
                                    self.dirty = true;
                                }
                            }
                        });

                        ui.add_space(10.0);
                        ui.separator();
                        ui.add_space(6.0);

                        ui.label(RichText::new("背景").size(12.0).color(COL_TEXT2));
                        ui.horizontal(|ui| {
                            let colors = [
                                (None, "透明"),
                                (Some(Color32::WHITE), "白"),
                                (Some(Color32::BLACK), "黑"),
                                (Some(Color32::from_rgb(0xB8, 0xC0, 0xCE)), "灰"),
                            ];
                            for (color, _label) in colors {
                                let selected = match color {
                                    None => self.opts.transparent,
                                    Some(c) => !self.opts.transparent && self.opts.bg == c,
                                };
                                let (rect, resp) =
                                    ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
                                let fill =
                                    color.unwrap_or(Color32::from_rgb(0xFF, 0xFF, 0xFF));
                                ui.painter().rect_filled(rect, CornerRadius::same(8), fill);
                                let stroke_color =
                                    if selected { COL_BRAND } else { COL_LINE };
                                ui.painter().rect_stroke(
                                    rect,
                                    CornerRadius::same(8),
                                    Stroke::new(if selected { 2.0 } else { 1.0 }, stroke_color),
                                    egui::StrokeKind::Inside,
                                );
                                if color.is_none() {
                                    ui.painter().text(
                                        rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        "无",
                                        FontId::proportional(10.0),
                                        COL_MUTED,
                                    );
                                }
                                if resp.clicked() {
                                    match color {
                                        None => self.opts.transparent = true,
                                        Some(c) => {
                                            self.opts.transparent = false;
                                            self.opts.bg = c;
                                        }
                                    }
                                    self.dirty = true;
                                }
                            }
                        });

                        ui.add_space(10.0);
                        ui.separator();
                        ui.add_space(6.0);

                        ui.label(RichText::new("导出格式").size(12.0).color(COL_TEXT2));
                        ui.horizontal(|ui| {
                            let width = (ui.available_width() - 12.0) / 3.0;
                            for format in [Format::Png, Format::Jpg, Format::Webp] {
                                let selected = self.opts.format == format;
                                if ui
                                    .add(
                                        egui::Button::new(RichText::new(format.label()).size(12.0)
                                            .color(if selected { COL_TEXT } else { COL_MUTED }))
                                        .fill(if selected {
                                            COL_PANEL
                                        } else {
                                            Color32::from_rgb(0xF2, 0xF3, 0xF7)
                                        })
                                        .stroke(if selected {
                                            Stroke::new(1.0, COL_LINE)
                                        } else {
                                            Stroke::NONE
                                        })
                                        .corner_radius(8)
                                        .min_size(Vec2::new(width, 28.0)),
                                    )
                                    .clicked()
                                {
                                    self.opts.format = format;
                                }
                            }
                        });

                        if self.opts.format != Format::Png {
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("质量").size(12.0).color(COL_TEXT2));
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            RichText::new(format!("{}", self.opts.quality))
                                                .size(11.5)
                                                .color(COL_TEXT),
                                        );
                                    },
                                );
                            });
                            let mut q = self.opts.quality;
                            if ui
                                .add(egui::Slider::new(&mut q, 40..=100).show_value(false))
                                .changed()
                            {
                                self.opts.quality = q;
                            }
                        }

                        ui.add_space(12.0);
                        egui::Frame::NONE
                            .fill(Color32::from_rgb(0xF7, 0xF9, 0xFC))
                            .corner_radius(9)
                            .inner_margin(egui::Margin::same(10))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(
                                        "每张图按原始像素放置，导出时不放大、不缩小；总尺寸等于各图尺寸按布局累加。",
                                    )
                                    .size(10.5)
                                    .color(COL_MUTED),
                                );
                            });
                    });
            });
    }

    fn number_row(&mut self, ui: &mut egui::Ui, label: &str, which: &str, min: u32, max: u32) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(label).size(12.0).color(COL_TEXT2));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let value = if which == "gap" {
                    self.opts.gap
                } else {
                    self.opts.margin
                };
                ui.label(
                    RichText::new(format!("{value} px"))
                        .size(11.5)
                        .strong()
                        .color(COL_TEXT),
                );
            });
        });

        let mut value = if which == "gap" {
            self.opts.gap
        } else {
            self.opts.margin
        };
        if ui
            .add(egui::Slider::new(&mut value, min..=max).show_value(false))
            .changed()
        {
            if which == "gap" {
                self.opts.gap = value;
            } else {
                self.opts.margin = value;
            }
            self.dirty = true;
        }
    }

    fn center_panel(&mut self, ui: &mut egui::Ui) {
        egui::containers::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(COL_BG)
                    .inner_margin(egui::Margin::same(12)),
            )
            .show(ui, |ui| {
                let avail = ui.available_size();
                let origin = ui.min_rect().min;

                let card_rect = egui::Rect::from_min_size(
                    origin,
                    Vec2::new(avail.x, (avail.y - 50.0).max(80.0)),
                );
                ui.painter()
                    .rect_filled(card_rect, CornerRadius::same(14), COL_PANEL);
                ui.painter().rect_stroke(
                    card_rect,
                    CornerRadius::same(14),
                    Stroke::new(1.0, COL_LINE),
                    egui::StrokeKind::Inside,
                );

                if let Some(texture) = &self.preview {
                    let tex_size = texture.size_vec2();
                    let inner = card_rect.shrink(22.0);
                    let scale = (inner.width() / tex_size.x)
                        .min(inner.height() / tex_size.y)
                        .min(1.0);
                    let draw_size = tex_size * scale;
                    let draw_rect =
                        egui::Rect::from_center_size(card_rect.center(), draw_size);
                    let uv =
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                    ui.painter()
                        .image(texture.id(), draw_rect, uv, Color32::WHITE);
                } else {
                    let center = card_rect.center();
                    ui.painter().text(
                        center - Vec2::new(0.0, 10.0),
                        egui::Align2::CENTER_CENTER,
                        "把图片拼成一张大图",
                        FontId::proportional(15.0),
                        COL_TEXT,
                    );
                    ui.painter().text(
                        center + Vec2::new(0.0, 16.0),
                        egui::Align2::CENTER_CENTER,
                        "点左侧「添加图片」，或截图后点「粘贴截图」",
                        FontId::proportional(12.0),
                        COL_MUTED,
                    );
                }

                let info_rect = egui::Rect::from_min_size(
                    egui::pos2(card_rect.left(), card_rect.bottom() + 8.0),
                    Vec2::new(card_rect.width(), 38.0),
                );
                ui.painter()
                    .rect_filled(info_rect, CornerRadius::same(10), COL_PANEL);
                ui.painter().rect_stroke(
                    info_rect,
                    CornerRadius::same(10),
                    Stroke::new(1.0, COL_LINE),
                    egui::StrokeKind::Inside,
                );

                let items = [
                    (
                        "输出尺寸",
                        if self.out_size.0 > 0 {
                            format!("{} × {}", self.out_size.0, self.out_size.1)
                        } else {
                            "—".to_string()
                        },
                    ),
                    (
                        "总像素",
                        if self.out_size.0 > 0 {
                            fmt_pixels(self.out_size.0 as u64 * self.out_size.1 as u64)
                        } else {
                            "—".to_string()
                        },
                    ),
                    (
                        "耗时",
                        if self.out_size.0 > 0 {
                            fmt_ms(self.preview_ms)
                        } else {
                            "—".to_string()
                        },
                    ),
                ];

                let mut x = info_rect.left() + 16.0;
                for (label, value) in items {
                    ui.painter().text(
                        egui::pos2(x, info_rect.top() + 12.0),
                        egui::Align2::LEFT_CENTER,
                        label,
                        FontId::proportional(10.0),
                        COL_MUTED,
                    );
                    ui.painter().text(
                        egui::pos2(x, info_rect.top() + 26.0),
                        egui::Align2::LEFT_CENTER,
                        value,
                        FontId::proportional(12.0),
                        COL_TEXT,
                    );
                    x += 150.0;
                }
            });
    }
}

fn truncate(text: &str, width: f32, size: f32) -> String {
    let approx = (width / (size * 0.62)).max(3.0) as usize;
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= approx {
        text.to_string()
    } else {
        let mut s: String = chars.into_iter().take(approx.saturating_sub(1)).collect();
        s.push('…');
        s
    }
}

/// 窗口 / 任务栏图标。PNG 随二进制一起编进去，运行时不依赖外部文件。
fn window_icon() -> egui::IconData {
    const PNG: &[u8] = include_bytes!("../assets/icon.png");
    const SIDE: u32 = 256;

    let Ok(img) = image::load_from_memory(PNG) else {
        return egui::IconData { rgba: vec![0; 4], width: 1, height: 1 };
    };
    let img = img.to_rgba8();
    let (w, h) = img.dimensions();
    let (rgba, width, height) = if w.max(h) > SIDE {
        let scaled = image::imageops::resize(&img, SIDE, SIDE, FilterType::Lanczos3);
        (scaled.into_raw(), SIDE, SIDE)
    } else {
        (img.into_raw(), w, h)
    };

    egui::IconData { rgba, width, height }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 830.0])
            .with_min_inner_size([1040.0, 680.0])
            .with_title("合图 · 多图拼接工具")
            .with_icon(window_icon()),
        ..Default::default()
    };

    eframe::run_native(
        "合图 · 多图拼接工具",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}
