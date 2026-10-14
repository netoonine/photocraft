//! Generative AI through KIE AI (the shared `craft-ai` crate): Edit › Generate Image… (a prompt,
//! optionally with the canvas as reference, becomes a new pixel layer) and Edit › Generative
//! Fill… (the selected area, with some context around it, is edited by the model and comes back
//! as a new layer masked by the selection, non-destructively, as in Photoshop). The default model
//! is Nano Banana 2; `model` takes any KIE image model id.
//!
//! The request runs as a background job (progress, Cancel); the layer lands as one undo step.
//! The API key lives in the OS keychain (`ai.setApiKey`) or `KIE_API_KEY`, never in preferences.
//! Desktop only: on wasm the commands return an error.

use std::sync::atomic::{AtomicBool, Ordering};

use photocraft_algo::resample::{Resample, resize_surface, translate_surface};
use photocraft_doc::{Document, Layer, LayerContent, LayerMask};
use photocraft_geom::{Rect, Size};
use photocraft_raster::Surface;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always};
use crate::file_cmds::str_param;
use crate::jobs::JobCtx;
use crate::{EngineError, Result, Session};

pub const GENERATE: &str = "edit.generateImage";
pub const FILL: &str = "edit.generativeFill";
pub const SET_KEY: &str = "ai.setApiKey";
pub const CLEAR_KEY: &str = "ai.clearApiKey";
pub const STATUS: &str = "ai.status";

/// The model used when `model` is not given.
pub const DEFAULT_MODEL: &str = "nano-banana-2";
/// Output resolutions Nano Banana 2 offers.
pub const RESOLUTIONS: [&str; 3] = ["1K", "2K", "4K"];
/// Context kept around the selection for Generative Fill, as a fraction of its larger side.
const FILL_CONTEXT: f64 = 0.15;
const MAX_PROMPT: usize = 5000;

/// Aspect ratios Nano Banana 2 accepts, with their width / height.
const RATIOS: [(&str, f64); 14] = [
    ("1:1", 1.0),
    ("2:3", 2.0 / 3.0),
    ("3:2", 1.5),
    ("3:4", 0.75),
    ("4:3", 4.0 / 3.0),
    ("4:5", 0.8),
    ("5:4", 1.25),
    ("9:16", 9.0 / 16.0),
    ("16:9", 16.0 / 9.0),
    ("21:9", 21.0 / 9.0),
    ("1:4", 0.25),
    ("4:1", 4.0),
    ("1:8", 0.125),
    ("8:1", 8.0),
];

static NETWORK_DISABLED: AtomicBool = AtomicBool::new(false);

/// Refuse network and keychain access from this process from now on (tests, fuzzing). The
/// `PHOTOCRAFT_NO_AI` environment variable does the same for a whole shell (CI).
pub fn disable_network() {
    NETWORK_DISABLED.store(true, Ordering::Relaxed);
}

fn network_disabled() -> bool {
    NETWORK_DISABLED.load(Ordering::Relaxed) || std::env::var_os("PHOTOCRAFT_NO_AI").is_some_and(|v| !v.is_empty() && v != "0")
}

fn ensure_network() -> Result<()> {
    if network_disabled() { Err(EngineError::Other("AI features are disabled in this process".into())) } else { Ok(()) }
}

/// The supported ratio closest to `w`×`h` (in log space, so 2:1 and 1:2 are equally far from 1:1).
pub fn nearest_ratio(w: u32, h: u32) -> &'static str {
    if w == 0 || h == 0 {
        return "1:1";
    }
    let want = (w as f64 / h as f64).ln();
    RATIOS.iter().min_by(|a, b| (a.1.ln() - want).abs().total_cmp(&(b.1.ln() - want).abs())).map_or("1:1", |r| r.0)
}

/// Parameters shared by both commands.
struct Options {
    prompt: String,
    model: String,
    resolution: String,
    aspect: Option<String>,
}

fn options(p: &Value, cmd: &str) -> Result<Options> {
    let bad = |msg: String| EngineError::BadParams { cmd: cmd.into(), msg };
    let prompt = str_param(p, "prompt", cmd)?.trim().to_string();
    if prompt.is_empty() {
        return Err(bad("\"prompt\" is empty".into()));
    }
    if prompt.chars().count() > MAX_PROMPT {
        return Err(bad(format!("\"prompt\" is longer than {MAX_PROMPT} characters")));
    }
    let model = match p.get("model") {
        None | Some(Value::Null) => DEFAULT_MODEL.to_string(),
        Some(Value::String(m)) if m.trim().is_empty() => DEFAULT_MODEL.to_string(),
        Some(Value::String(m)) if m.len() <= 100 && m.trim().chars().all(|c| c.is_ascii_alphanumeric() || "-_./:".contains(c)) => m.trim().to_string(),
        Some(_) => return Err(bad("\"model\" must be a KIE model id such as \"nano-banana-2\"".into())),
    };
    let resolution = match p.get("resolution") {
        None | Some(Value::Null) => "1K".to_string(),
        Some(Value::String(r)) => RESOLUTIONS.iter().find(|x| x.eq_ignore_ascii_case(r)).map(|x| x.to_string()).ok_or_else(|| bad(format!("\"resolution\" must be one of {RESOLUTIONS:?}")))?,
        Some(_) => return Err(bad(format!("\"resolution\" must be one of {RESOLUTIONS:?}"))),
    };
    let aspect = match p.get("aspectRatio") {
        None | Some(Value::Null) => None,
        Some(Value::String(a)) if a == "auto" || RATIOS.iter().any(|r| r.0 == a) => Some(a.clone()),
        Some(_) => return Err(bad("\"aspectRatio\" must be \"auto\" or a ratio such as \"16:9\"".into())),
    };
    Ok(Options { prompt, model, resolution, aspect })
}

/// A short layer name from the prompt.
fn layer_name(prefix: &str, prompt: &str) -> String {
    let words: String = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut short: String = words.chars().take(40).collect();
    if words.chars().count() > 40 {
        short.push('…');
    }
    format!("{prefix}: {short}")
}

/// The composite of `r` (white where transparent) as PNG bytes, for the model to look at.
fn png_of_region(doc: &Document, r: Rect) -> Result<Vec<u8>> {
    if r.is_empty() {
        return Err(EngineError::Other("the area is empty".into()));
    }
    let comp = photocraft_compose::composite_to_surface(doc, r, doc.pixel_format(), Some([1.0, 1.0, 1.0]));
    let mut tmp = Document::new("region", Size::new(r.width(), r.height()), doc.mode, doc.depth);
    tmp.icc_profile = doc.icc_profile.clone();
    tmp.layers.push(Layer::new("region", LayerContent::Raster(translate_surface(&comp, -r.x0, -r.y0))));
    let out = photocraft_io::export(&tmp, "png", &photocraft_io::ExportOptions::default()).map_err(|e| EngineError::Other(format!("could not encode the image for the AI: {e}")))?;
    Ok(out.bytes)
}

/// How a generated image is fitted to its target rectangle.
#[derive(Clone, Copy)]
enum Fit {
    /// Keep the aspect and cover the whole rectangle, centred; the overflow stays in the layer
    /// (Generate Image: the model's sizes are close to the canvas ratio, never exact).
    Cover,
    /// Exactly the rectangle (Generative Fill: the model saw that rectangle).
    Stretch,
}

/// Decode `bytes` and add them as a new pixel layer over the active one, fitted to `target`,
/// with `mask`: one undo step named `label`.
fn insert_generated(s: &mut Session, label: &str, name: &str, bytes: &[u8], target: Rect, fit: Fit, mask: Option<LayerMask>) -> Result<Value> {
    let fmt = s.active().ok_or(EngineError::NoDocument)?.doc.pixel_format();
    let imported = photocraft_io::import("generated.png", bytes).map_err(|e| EngineError::Other(format!("the AI returned an image PhotoCraft can't read: {e}")))?;
    let (w, h) = (imported.document.size.width as f64, imported.document.size.height as f64);
    if w < 1.0 || h < 1.0 || target.is_empty() {
        return Err(EngineError::Other("the AI returned an empty image".into()));
    }
    let (tw, th) = (target.width() as f64, target.height() as f64);
    let (sx, sy) = match fit {
        Fit::Cover => {
            let k = (tw / w).max(th / h);
            (k, k)
        }
        Fit::Stretch => (tw / w, th / h),
    };
    let mut px = crate::file_cmds::flattened(&imported.document, fmt);
    if (sx - 1.0).abs() > 1e-9 || (sy - 1.0).abs() > 1e-9 {
        px = resize_surface(&px, sx, sy, Resample::Bicubic);
    }
    let dx = target.x0 + ((tw - w * sx) / 2.0).round() as i32;
    let dy = target.y0 + ((th - h * sy) / 2.0).round() as i32;
    let px: Surface = translate_surface(&px, dx, dy);
    let name = name.to_string();
    let id = s.edit(label, move |doc, active| {
        let mut layer = Layer::new(name, LayerContent::Raster(px));
        layer.mask = mask;
        let id = doc.insert_above(*active, layer);
        *active = Some(id);
        Ok(id)
    })?;
    Ok(json!({ "layer": id.0, "bounds": [dx, dy, dx + (w * sx).round() as i32, dy + (h * sy).round() as i32] }))
}

/// The blocking request, run on the job's worker thread.
#[cfg(not(target_arch = "wasm32"))]
fn request(req: craft_ai::ImageRequest, ctx: &JobCtx) -> Result<Vec<u8>> {
    ensure_network()?;
    let kie = craft_ai::Kie::from_default_key().map_err(|e| EngineError::Other(e.to_string()))?;
    let hooks = craft_ai::Hooks { cancelled: &|| ctx.cancelled(), progress: &|f, m| ctx.progress(f, m) };
    kie.generate_image(&req, &hooks).map_err(|e| match e {
        craft_ai::Error::Cancelled => EngineError::Cancelled,
        e => EngineError::Other(e.to_string()),
    })
}

#[cfg(target_arch = "wasm32")]
fn request(_: AiRequest, _: &JobCtx) -> Result<Vec<u8>> {
    Err(EngineError::Other("AI generation needs the desktop app".into()))
}

#[cfg(not(target_arch = "wasm32"))]
type AiRequest = craft_ai::ImageRequest;

#[cfg(target_arch = "wasm32")]
struct AiRequest;

#[cfg(not(target_arch = "wasm32"))]
fn ai_request(o: Options, images: Vec<Vec<u8>>, aspect: String) -> AiRequest {
    craft_ai::ImageRequest { model: o.model, prompt: o.prompt, images, aspect_ratio: aspect, resolution: o.resolution, extra: Default::default() }
}

#[cfg(target_arch = "wasm32")]
fn ai_request(_: Options, _: Vec<Vec<u8>>, _: String) -> AiRequest {
    AiRequest
}

fn has_doc(s: &Session) -> std::result::Result<(), String> {
    s.active().map(|_| ()).ok_or_else(|| "no document open".into())
}

fn has_selection(s: &Session) -> std::result::Result<(), String> {
    let d = s.active().ok_or("no document open")?;
    let sel = d.doc.selection.as_ref().ok_or("make a selection first")?;
    if sel.content_bounds().intersect(&d.doc.bounds()).is_empty() { Err("the selection is empty".into()) } else { Ok(()) }
}

fn run_generate(s: &mut Session, p: &Value) -> Result<Value> {
    let o = options(p, GENERATE)?;
    let use_canvas = p.get("useCanvas").and_then(Value::as_bool).unwrap_or(false);
    let d = s.active().ok_or(EngineError::NoDocument)?;
    let canvas = d.doc.bounds();
    let aspect = o.aspect.clone().unwrap_or_else(|| nearest_ratio(canvas.width(), canvas.height()).to_string());
    let images = if use_canvas { vec![png_of_region(&d.doc, canvas)?] } else { Vec::new() };
    ensure_network()?;
    let name = layer_name("Generated", &o.prompt);
    let req = ai_request(o, images, aspect);
    crate::jobs::run(s, "Generate Image", true, move |ctx| request(req, ctx), move |s, bytes| insert_generated(s, "Generate Image", &name, &bytes, canvas, Fit::Cover, None))
}

fn run_fill(s: &mut Session, p: &Value) -> Result<Value> {
    let o = options(p, FILL)?;
    // With context (the default) the model sees the selected area plus its surroundings and edits
    // that crop, so the result fits the scene. Without, it only gets the prompt and paints a new
    // image shaped like the selection.
    let context = p.get("context").and_then(Value::as_bool).unwrap_or(true);
    has_selection(s).map_err(EngineError::Other)?;
    let d = s.active().ok_or(EngineError::NoDocument)?;
    let canvas = d.doc.bounds();
    let sel = d.doc.selection.as_ref().ok_or_else(|| EngineError::Other("make a selection first".into()))?;
    let b = sel.content_bounds().intersect(&canvas);
    let region = if context {
        let pad = (b.width().max(b.height()) as f64 * FILL_CONTEXT).round() as i32;
        b.inflate(pad.max(8)).intersect(&canvas)
    } else {
        b
    };
    let images = if context { vec![png_of_region(&d.doc, region)?] } else { Vec::new() };
    // A soft edge hides the seam where the model's slightly different render meets the photo.
    let feather = (b.width().max(b.height()) as f32 * 0.02).clamp(2.0, 40.0);
    let mask = crate::commands::selection_mask(&d.doc)?.map(|m| LayerMask { feather, ..m });
    ensure_network()?;
    let name = layer_name("Generative Fill", &o.prompt);
    let aspect = nearest_ratio(region.width(), region.height()).to_string();
    let mut o = o;
    if context {
        // The model edits the whole crop; the mask keeps only the selected part, so ask it to
        // leave the framing alone or the edit would not line up with the photo around it.
        o.prompt = format!("{}\n\nEdit this image as asked. Keep the framing, perspective, scale and lighting identical so the result lines up with the original.", o.prompt);
    }
    let req = ai_request(o, images, aspect);
    crate::jobs::run(s, "Generative Fill", true, move |ctx| request(req, ctx), move |s, bytes| insert_generated(s, "Generative Fill", &name, &bytes, region, Fit::Stretch, mask))
}

#[cfg(not(target_arch = "wasm32"))]
fn key_status() -> Value {
    if network_disabled() {
        return json!({ "available": false, "hasKey": false, "source": null, "model": DEFAULT_MODEL });
    }
    let source = craft_ai::key::source();
    json!({ "available": true, "hasKey": source.is_some(), "source": source, "model": DEFAULT_MODEL, "provider": "KIE AI" })
}

#[cfg(target_arch = "wasm32")]
fn key_status() -> Value {
    json!({ "available": false, "hasKey": false, "source": null, "model": DEFAULT_MODEL })
}

fn run_status(_: &mut Session, _: &Value) -> Result<Value> {
    Ok(key_status())
}

fn run_set_key(_: &mut Session, p: &Value) -> Result<Value> {
    let key = str_param(p, "key", SET_KEY)?.trim().to_string();
    ensure_network()?;
    #[cfg(not(target_arch = "wasm32"))]
    {
        craft_ai::key::store(&key).map_err(|e| EngineError::Other(e.to_string()))?;
        Ok(key_status())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = key;
        Err(EngineError::Other("AI generation needs the desktop app".into()))
    }
}

fn run_clear_key(_: &mut Session, p: &Value) -> Result<Value> {
    if p.get("confirm").and_then(Value::as_bool) != Some(true) {
        return Err(EngineError::BadParams { cmd: CLEAR_KEY.into(), msg: "pass {\"confirm\": true} to remove the saved key".into() });
    }
    ensure_network()?;
    #[cfg(not(target_arch = "wasm32"))]
    {
        craft_ai::key::clear().map_err(|e| EngineError::Other(e.to_string()))?;
        Ok(key_status())
    }
    #[cfg(target_arch = "wasm32")]
    Err(EngineError::Other("AI generation needs the desktop app".into()))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: GENERATE,
            label: "Generate Image…",
            menu: &["Edit"],
            shortcut: None,
            params: r##"{"prompt":string,"model":string="nano-banana-2","resolution":"1K"|"2K"|"4K"="1K","aspectRatio":"auto"|"16:9"|…=canvas,"useCanvas":bool=false} → {layer,bounds} (KIE AI; a background job; the image lands as a new pixel layer fitted to the canvas)"##,
            enabled: has_doc,
            run: run_generate,
            journal: true,
        },
        CommandSpec {
            id: FILL,
            label: "Generative Fill…",
            menu: &["Edit"],
            shortcut: None,
            params: r##"{"prompt":string,"context":bool=true,"model":string="nano-banana-2","resolution":"1K"|"2K"|"4K"="1K"} → {layer,bounds} (needs a selection; context: KIE AI edits the selected area plus its surroundings; context false: a new image from the prompt alone, shaped like the selection; a new layer masked by the selection)"##,
            enabled: has_selection,
            run: run_fill,
            journal: true,
        },
        CommandSpec {
            id: SET_KEY,
            label: "AI API Key…",
            menu: &["Edit"],
            shortcut: None,
            params: r##"{"key":string} → {hasKey,source} (stores the KIE AI key in the OS keychain; never recorded)"##,
            enabled: always,
            run: run_set_key,
            journal: false,
        },
        CommandSpec {
            id: CLEAR_KEY,
            label: "Remove AI API Key",
            menu: &[],
            shortcut: None,
            params: r##"{"confirm":true} → {hasKey,source}"##,
            enabled: always,
            run: run_clear_key,
            journal: false,
        },
        CommandSpec {
            id: STATUS,
            label: "AI Status",
            menu: &[],
            shortcut: None,
            params: r##"{} → {available,hasKey,source,model,provider}"##,
            enabled: always,
            run: run_status,
            journal: false,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_codecs::Image;

    fn session() -> Session {
        disable_network();
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 64, "height": 48, "background": "#3366cc"})).unwrap();
        s
    }

    /// A solid red PNG of `w`×`h`, as the model would return.
    fn red_png(w: u32, h: u32) -> Vec<u8> {
        let mut doc = Document::new("r", Size::new(w, h), photocraft_color::ColorMode::Rgb, photocraft_color::SampleType::U8);
        let mut surf = Surface::new(photocraft_color::PixelFormat::RGBA8);
        surf.write_region(Rect::new(0, 0, w as i32, h as i32), &[1.0, 0.0, 0.0, 1.0].repeat((w * h) as usize));
        doc.layers.push(Layer::new("r", LayerContent::Raster(surf)));
        photocraft_io::export(&doc, "png", &photocraft_io::ExportOptions::default()).unwrap().bytes
    }

    fn layers(s: &Session) -> usize {
        s.active().unwrap().doc.layer_count()
    }

    #[test]
    fn nearest_ratio_picks_the_closest_supported_shape() {
        assert_eq!(nearest_ratio(1000, 1000), "1:1");
        assert_eq!(nearest_ratio(1920, 1080), "16:9");
        assert_eq!(nearest_ratio(1080, 1920), "9:16");
        assert_eq!(nearest_ratio(3000, 2000), "3:2");
        assert_eq!(nearest_ratio(100, 1000), "1:8");
        assert_eq!(nearest_ratio(0, 10), "1:1");
    }

    #[test]
    fn bad_params_are_errors_before_any_request() {
        let mut s = session();
        for p in [
            json!({}),
            json!({"prompt": ""}),
            json!({"prompt": "   "}),
            json!({"prompt": 5}),
            json!({"prompt": "x", "resolution": "8K"}),
            json!({"prompt": "x", "resolution": 2}),
            json!({"prompt": "x", "model": "bad model!"}),
            json!({"prompt": "x", "aspectRatio": "7:3"}),
            json!({"prompt": "x".repeat(MAX_PROMPT + 1)}),
        ] {
            assert!(matches!(s.execute(GENERATE, p.clone()), Err(EngineError::BadParams { .. })), "{p}");
        }
        assert_eq!(layers(&s), 1);
    }

    #[test]
    fn generative_fill_needs_a_selection() {
        let mut s = session();
        assert!(!s.is_enabled(FILL));
        assert!(s.execute(FILL, json!({"prompt": "a cat"})).is_err());
        s.execute("select.rect", json!({"x": 10, "y": 10, "width": 20, "height": 10})).unwrap();
        assert!(s.is_enabled(FILL));
        assert!(!Session::new().is_enabled(GENERATE));
    }

    #[test]
    fn with_the_network_disabled_nothing_is_sent_and_the_document_is_unchanged() {
        let mut s = session();
        let before = s.active().unwrap().revision;
        assert!(s.execute(GENERATE, json!({"prompt": "a cat"})).is_err());
        s.execute("select.rect", json!({"x": 10, "y": 10, "width": 20, "height": 10})).unwrap();
        let before_fill = s.active().unwrap().revision;
        assert!(s.execute(FILL, json!({"prompt": "a cat"})).is_err());
        assert!(s.execute(FILL, json!({"prompt": "a cat", "context": false})).is_err());
        assert_eq!(s.active().unwrap().revision, before_fill);
        assert!(before <= before_fill);
        // Keychain commands refuse too, and clearing needs an explicit confirm.
        assert!(s.execute(SET_KEY, json!({"key": "abc"})).is_err());
        assert!(matches!(s.execute(CLEAR_KEY, json!({})), Err(EngineError::BadParams { .. })));
        assert!(s.execute(CLEAR_KEY, json!({"confirm": true})).is_err());
        assert_eq!(s.execute(STATUS, json!({})).unwrap()["available"], false);
    }

    #[test]
    fn the_region_png_has_the_region_size() {
        let s = session();
        let png = png_of_region(&s.active().unwrap().doc, Rect::new(4, 6, 36, 22)).unwrap();
        let img: Image = photocraft_codecs::decode(&png).unwrap();
        assert_eq!((img.width(), img.height()), (32, 16));
        assert!(png_of_region(&s.active().unwrap().doc, Rect::new(4, 4, 4, 4)).is_err());
    }

    #[test]
    fn a_fill_result_lands_stretched_on_the_region_masked_by_the_selection_in_one_step() {
        let mut s = session();
        s.execute("select.rect", json!({"x": 10, "y": 10, "width": 20, "height": 10})).unwrap();
        let mask = crate::commands::selection_mask(&s.active().unwrap().doc).unwrap();
        let steps = s.active().unwrap().history.past_len();
        let region = Rect::new(5, 5, 35, 25);
        let r = insert_generated(&mut s, "Generative Fill", "Generative Fill: cat", &red_png(512, 512), region, Fit::Stretch, mask).unwrap();
        assert_eq!(r["bounds"], json!([5, 5, 35, 25]));
        assert_eq!(s.active().unwrap().history.past_len(), steps + 1);
        let doc = &s.active().unwrap().doc;
        assert_eq!(doc.layer_count(), 2);
        let px = photocraft_compose::render(doc, Rect::new(0, 0, 64, 48));
        let at = |x: usize, y: usize| px.px[y * 64 + x];
        assert!(at(15, 15)[0] > 0.9 && at(15, 15)[2] < 0.1, "inside the selection is the generated red: {:?}", at(15, 15));
        assert!(at(7, 7)[2] > 0.5, "outside the selection the photo shows: {:?}", at(7, 7));
        assert!(at(50, 40)[2] > 0.5);
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(layers(&s), 1);
    }

    #[test]
    fn a_generated_image_covers_the_canvas_keeping_its_aspect() {
        let mut s = session();
        let canvas = s.active().unwrap().doc.bounds();
        let r = insert_generated(&mut s, "Generate Image", "Generated: cat", &red_png(100, 100), canvas, Fit::Cover, None).unwrap();
        // 64×48 canvas: a square covers it at 64×64, centred, the overflow kept in the layer.
        assert_eq!(r["bounds"], json!([0, -8, 64, 56]));
        assert_eq!(s.active().unwrap().doc.layers.last().unwrap().name, "Generated: cat");
    }

    #[test]
    fn garbage_from_the_model_is_an_error_not_a_panic() {
        let mut s = session();
        let canvas = s.active().unwrap().doc.bounds();
        assert!(insert_generated(&mut s, "Generate Image", "x", b"not an image", canvas, Fit::Cover, None).is_err());
        assert!(insert_generated(&mut s, "Generate Image", "x", &[], canvas, Fit::Cover, None).is_err());
        assert_eq!(layers(&s), 1);
    }

    #[test]
    fn layer_names_are_short() {
        assert_eq!(layer_name("Generated", "a  red\nfox"), "Generated: a red fox");
        assert!(layer_name("Generated", &"word ".repeat(50)).ends_with('…'));
    }
}
