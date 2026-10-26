//! Edit › Generate Image…, Edit › Generative Fill… and Edit › AI API Key… dialogs (KIE AI, see
//! `photocraft_engine::genai_cmds`). The prompt dialogs run the command as a background job, so
//! the status bar shows the progress and Esc cancels; the key dialog stores the key in the OS
//! keychain and never shows it again.
use crate::{PhotocraftApp, state::DialogKind};
use photocraft_engine::genai_cmds::{FILL, GENERATE, MODELS, RESOLUTIONS, SET_KEY, STATUS};
use serde_json::{Map, Value, json};

/// The last AI prompt and its settings, offered again by the next Generate Image / Generative Fill.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GenAiLast {
    pub prompt: String,
    pub model: String,
    pub resolution: String,
    pub use_canvas: bool,
    /// Generative Fill dialog: Selection only (the prompt alone) rather than With context.
    pub area_only: bool,
}

/// The engine's `ai.models` list (`[{key,label,edit,generate}]`).
pub fn models(app: &mut PhotocraftApp) -> Vec<Value> {
    app.session.execute(MODELS, json!({})).ok().and_then(|r| r.get("models").and_then(Value::as_array).cloned()).unwrap_or_default()
}

/// `(key, label)` of the models that fit: `edit` = the request sends an image (fill with context,
/// generate from the canvas), else it needs text-to-image. A `current` id the list doesn't know
/// (typed through automation) is kept at the end so it isn't lost.
pub fn model_options(models: &[Value], edit: bool, current: &str) -> Vec<(String, String)> {
    let need = if edit { "edit" } else { "generate" };
    let mut out: Vec<(String, String)> = models
        .iter()
        .filter(|m| m.get(need).and_then(Value::as_bool).unwrap_or(false))
        .filter_map(|m| Some((m.get("key")?.as_str()?.to_string(), m.get("label")?.as_str()?.to_string())))
        .collect();
    let known = models.iter().any(|m| m.get("key").and_then(Value::as_str) == Some(current));
    if !current.is_empty() && !known {
        out.push((current.to_string(), current.to_string()));
    }
    out
}

/// `current` if it fits the request, else the first model that does (Nano Banana 2 leads the list).
pub fn fit_model(options: &[(String, String)], current: &str) -> String {
    options.iter().find(|(k, _)| k == current).or(options.first()).map_or_else(|| current.to_string(), |(k, _)| k.clone())
}

/// A model dropdown; returns whether the choice changed.
pub fn model_dropdown(ui: &mut egui::Ui, id: &str, current: &mut String, options: &[(String, String)], width: f32) -> bool {
    let opts: Vec<(String, &str)> = options.iter().map(|(k, l)| (k.clone(), l.as_str())).collect();
    crate::widgets::dropdown(ui, id, tl!("Model"), current, &opts, width)
}

const MARK: &str = "__genai";
/// Width of the dialog's fields.
const FIELD_W: f32 = 380.0;

pub fn owns(fields: &Map<String, Value>) -> bool {
    fields.contains_key(MARK)
}

/// Whether the menu item `id` opens one of these dialogs (when invoked without params).
pub fn intercepts(id: &str) -> bool {
    matches!(id, GENERATE | FILL | SET_KEY)
}

pub fn open(app: &mut PhotocraftApp, command: &str) -> Result<Value, String> {
    let status = app.session.execute(STATUS, json!({})).unwrap_or(Value::Null);
    let mut fields = Map::new();
    fields.insert(MARK.into(), json!(true));
    fields.insert("__command".into(), json!(command));
    fields.insert("hasKey".into(), status.get("hasKey").cloned().unwrap_or(json!(false)));
    fields.insert("source".into(), status.get("source").cloned().unwrap_or(Value::Null));
    match command {
        SET_KEY => {
            fields.insert("__label".into(), json!("AI API Key…"));
            fields.insert("key".into(), json!(""));
        }
        _ => {
            if command == FILL {
                app.session.active().and_then(|d| d.doc.selection.as_ref()).ok_or("Make a selection first: Generative Fill changes the selected area.")?;
            } else {
                app.session.active().ok_or("Open or create a document first.")?;
            }
            fields.insert("__label".into(), json!(if command == FILL { "Generative Fill…" } else { "Generate Image…" }));
            // The last prompt and settings come back, as in Photoshop's prompt bar.
            let last = app.ui.genai_last.clone();
            fields.insert("prompt".into(), json!(last.prompt));
            fields.insert("model".into(), json!(if last.model.is_empty() { status.get("model").and_then(Value::as_str).unwrap_or("nano-banana-2").to_string() } else { last.model }));
            fields.insert("resolution".into(), json!(if last.resolution.is_empty() { "1K".to_string() } else { last.resolution }));
            fields.insert("useCanvas".into(), json!(last.use_canvas));
            fields.insert("areaOnly".into(), json!(last.area_only));
            fields.insert("__models".into(), Value::Array(models(app)));
        }
    }
    let id = app.ui.open_dialog(DialogKind::Command, fields);
    Ok(json!({"dialog": id}))
}

pub fn body(ui: &mut egui::Ui, fields: &mut Map<String, Value>) {
    let command = fields.get("__command").and_then(Value::as_str).unwrap_or_default().to_string();
    let has_key = fields.get("hasKey").and_then(Value::as_bool).unwrap_or(false);
    if command == SET_KEY {
        let status = match fields.get("source").and_then(Value::as_str) {
            Some("environment") => tl!("The KIE_API_KEY environment variable is set and takes precedence."),
            Some(_) => tl!("A key is saved. Enter a new one to replace it, or leave empty to remove it."),
            None => tl!("No key yet. Get one at kie.ai."),
        };
        ui.label(status);
        ui.add_space(6.0);
        let mut key = fields.get("key").and_then(Value::as_str).unwrap_or("").to_string();
        ui.horizontal(|ui| {
            ui.label(tl!("KIE API key"));
            ui.add(egui::TextEdit::singleline(&mut key).password(true).desired_width(FIELD_W - 90.0));
        });
        fields.insert("key".into(), json!(key));
        return;
    }
    if !has_key {
        ui.colored_label(egui::Color32::from_rgb(0xd9, 0x8c, 0x1f), tl!("No KIE API key yet: set it in Edit › AI API Key…"));
        ui.add_space(4.0);
    }
    let hint = if command == FILL { tl!("Describe what to change in the selection") } else { tl!("Describe the image to generate") };
    let mut prompt = fields.get("prompt").and_then(Value::as_str).unwrap_or("").to_string();
    ui.add(egui::TextEdit::multiline(&mut prompt).hint_text(hint).desired_rows(4).desired_width(FIELD_W));
    fields.insert("prompt".into(), json!(prompt));
    ui.add_space(6.0);
    let mut area_only = fields.get("areaOnly").and_then(Value::as_bool).unwrap_or(false);
    if command == FILL {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut area_only, false, tl!("With context"))
                .on_hover_text(tl!("The AI sees the area around the selection, so the result blends into the image"));
            ui.selectable_value(&mut area_only, true, tl!("Selection only"))
                .on_hover_text(tl!("The AI paints from your prompt alone, shaped like the selection"));
        });
        fields.insert("areaOnly".into(), json!(area_only));
        ui.add_space(4.0);
    }
    let use_canvas = fields.get("useCanvas").and_then(Value::as_bool).unwrap_or(false);
    // Sending an image (fill with context, generate from the canvas) needs a model that edits.
    let sends_image = if command == FILL { !area_only } else { use_canvas };
    let models: Vec<Value> = fields.get("__models").and_then(Value::as_array).cloned().unwrap_or_default();
    let model_opts = model_options(&models, sends_image, fields.get("model").and_then(Value::as_str).unwrap_or(""));
    let mut model = fit_model(&model_opts, fields.get("model").and_then(Value::as_str).unwrap_or(""));
    let mut resolution = fields.get("resolution").and_then(Value::as_str).unwrap_or("1K").to_string();
    let options: Vec<(String, &str)> = RESOLUTIONS.iter().map(|r| (r.to_string(), *r)).collect();
    egui::Grid::new("genai-options").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        ui.label(tl!("Model"));
        model_dropdown(ui, "genai-model", &mut model, &model_opts, 200.0);
        ui.end_row();
        ui.label(tl!("Resolution"));
        crate::widgets::dropdown(ui, "genai-resolution", tl!("Resolution"), &mut resolution, &options, 200.0);
        ui.end_row();
    });
    fields.insert("model".into(), json!(model));
    fields.insert("resolution".into(), json!(resolution));
    if command == GENERATE {
        let mut use_canvas = fields.get("useCanvas").and_then(Value::as_bool).unwrap_or(false);
        crate::widgets::checkbox(ui, &mut use_canvas, tl!("Use the canvas as a reference"));
        fields.insert("useCanvas".into(), json!(use_canvas));
    }
    ui.add_space(4.0);
    ui.label(egui::RichText::new(tl!("Uses credits from your KIE AI account.")).small());
}

pub fn confirm(app: &mut PhotocraftApp, fields: &Map<String, Value>) -> Result<Value, String> {
    let command = fields.get("__command").and_then(Value::as_str).unwrap_or_default();
    if command == SET_KEY {
        let key = fields.get("key").and_then(Value::as_str).unwrap_or("").trim().to_string();
        return if key.is_empty() { app.run_command(photocraft_engine::genai_cmds::CLEAR_KEY, json!({"confirm": true})) } else { app.run_command(SET_KEY, json!({"key": key})) };
    }
    let prompt = fields.get("prompt").and_then(Value::as_str).unwrap_or("").trim().to_string();
    if prompt.is_empty() {
        return Err("Type a prompt first.".into());
    }
    let model = fields.get("model").and_then(Value::as_str).unwrap_or("").trim().to_string();
    let resolution = fields.get("resolution").and_then(Value::as_str).unwrap_or("1K").to_string();
    let use_canvas = fields.get("useCanvas").and_then(Value::as_bool).unwrap_or(false);
    let area_only = fields.get("areaOnly").and_then(Value::as_bool).unwrap_or(false);
    app.ui.genai_last = GenAiLast { prompt: prompt.clone(), model: model.clone(), resolution: resolution.clone(), use_canvas, area_only };
    let mut params = json!({"prompt": prompt, "model": model, "resolution": resolution});
    if command == GENERATE {
        params["useCanvas"] = json!(use_canvas);
    } else {
        params["context"] = json!(!area_only);
    }
    app.run_command(command, params)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_app() -> PhotocraftApp {
        photocraft_engine::genai_cmds::disable_network();
        let mut s = photocraft_engine::Session::new();
        s.execute("file.new", json!({"width": 32, "height": 24})).unwrap();
        PhotocraftApp::new(s, crate::Services::default())
    }

    fn invoke(app: &mut PhotocraftApp, id: &str) -> Result<Value, String> {
        let ctx = egui::Context::default();
        crate::menus::invoke(app, &ctx, id, json!({}))
    }

    #[test]
    fn the_model_list_fits_the_request() {
        let mut app = make_app();
        let all = models(&mut app);
        assert!(all.len() > 5);
        let editors = model_options(&all, true, "nano-banana-2");
        let makers = model_options(&all, false, "nano-banana-2");
        assert_eq!(editors[0].0, "nano-banana-2");
        assert!(!editors.iter().any(|(k, _)| k == "imagen-4-ultra"), "Imagen can't edit");
        assert!(makers.iter().any(|(k, _)| k == "imagen-4-ultra"));
        // A model that can't do the job falls back to the first that can.
        assert_eq!(fit_model(&editors, "imagen-4-ultra"), "nano-banana-2");
        assert_eq!(fit_model(&makers, "seedream-5-pro"), "seedream-5-pro");
        // A custom id typed through automation is kept.
        assert!(model_options(&all, false, "some/custom-model").iter().any(|(k, _)| k == "some/custom-model"));
    }

    #[test]
    fn the_menu_items_open_their_dialogs() {
        let mut app = make_app();
        let d = invoke(&mut app, GENERATE).unwrap()["dialog"].as_u64().unwrap();
        assert!(owns(&app.ui.dialog_mut(d).unwrap().fields));
        let d = invoke(&mut app, SET_KEY).unwrap()["dialog"].as_u64().unwrap();
        assert_eq!(app.ui.dialog_mut(d).unwrap().fields["__command"], SET_KEY);
    }

    #[test]
    fn generative_fill_without_a_selection_explains_itself() {
        let mut app = make_app();
        let e = invoke(&mut app, FILL).unwrap_err();
        assert!(e.contains("selection"), "{e}");
        app.run("select.rect", json!({"x": 2, "y": 2, "width": 8, "height": 8})).unwrap();
        assert!(invoke(&mut app, FILL).is_ok());
    }

    #[test]
    fn an_empty_prompt_is_refused_and_settings_are_remembered() {
        let mut app = make_app();
        let d = invoke(&mut app, GENERATE).unwrap()["dialog"].as_u64().unwrap();
        assert!(crate::dialogs::confirm(&mut app, d).is_err());
        let d = invoke(&mut app, GENERATE).unwrap()["dialog"].as_u64().unwrap();
        let f = &mut app.ui.dialog_mut(d).unwrap().fields;
        f.insert("prompt".into(), json!("a lighthouse at dusk"));
        f.insert("resolution".into(), json!("2K"));
        // The network is disabled in tests: the command fails, but the prompt is remembered.
        assert!(crate::dialogs::confirm(&mut app, d).is_err());
        assert_eq!(app.ui.genai_last.prompt, "a lighthouse at dusk");
        assert_eq!(app.ui.genai_last.resolution, "2K");
        let d = invoke(&mut app, GENERATE).unwrap()["dialog"].as_u64().unwrap();
        assert_eq!(app.ui.dialog_mut(d).unwrap().fields["prompt"], "a lighthouse at dusk");
    }
}
