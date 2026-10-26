//! Contextual task bar, as in Photoshop: a small floating bar shown while there is a selection
//! (any selection tool), docked at the bottom centre of the canvas, offering Generate with AI. Two modes, both `edit.generativeFill` as a
//! background job (status bar progress, Esc cancels):
//! - With context: the model sees the selection and the area around it, so the result blends
//!   into the image.
//! - Selection only: the prompt alone; a new image shaped like the selection fills it.
//!
//! The bar hides while a gesture, transform, crop, text edit or dialog is in progress. Its ×
//! hides it until the selection changes.
use crate::PhotocraftApp;
use crate::canvas::ViewXform;
use egui::{Context, pos2, vec2};
use photocraft_engine::genai_cmds::FILL;
use photocraft_geom::Rect as DRect;
use serde_json::{Value, json};

/// The contextual task bar's state: open or not, its prompt and mode, and the selection it was
/// closed for (it comes back when the selection changes).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContextBarUi {
    pub expanded: bool,
    pub prompt: String,
    /// Selection only (the prompt alone) rather than With context.
    pub area_only: bool,
    /// Focus the prompt on the next frame (just expanded).
    pub focus: bool,
    pub dismissed: Option<crate::context_bar::SelectionKey>,
    /// The engine's model list, read once when the bar first expands.
    pub models: Vec<Value>,
}

/// Identifies one selection: its document and bounds (a new or changed selection shows the bar
/// again after ×).
pub type SelectionKey = (u64, [i32; 4]);

/// Gap between the bar and the bottom of the canvas, in points.
const GAP: f32 = 10.0;

fn busy_with_gesture(app: &PhotocraftApp) -> bool {
    app.drag.is_some()
        || app.press_stroke
        || !app.ui.polygon.is_empty()
        || app.brush_resize.is_some()
        || app.guide_drag.is_some()
        || app.ui.transform.is_some()
        || app.ui.text_edit.is_some()
        || !app.ui.dialogs.is_empty()
        || crate::cam_transform::active(app)
        || crate::crop_ui::active(app)
}

/// The selection the bar belongs to, unless the bar should not show now.
pub fn target(app: &mut PhotocraftApp) -> Option<(SelectionKey, DRect)> {
    if !app.ui.panels.context_bar || busy_with_gesture(app) {
        return None;
    }
    let (doc, canvas, sel) = app.session.active().and_then(|st| Some((st.doc.id.0, st.doc.bounds(), st.doc.selection.clone()?)))?;
    // Same cache key as the canvas outline, so the bar adds no work.
    let b = app.cached_bounds(u64::MAX - doc, &sel).intersect(&canvas);
    if b.is_empty() {
        return None;
    }
    let key = (doc, [b.x0, b.y0, b.x1, b.y1]);
    (app.ui.context_bar.dismissed != Some(key)).then_some((key, b))
}

/// The `edit.generativeFill` params for the bar's prompt and mode, with the dialog's last model
/// and resolution.
pub fn params(bar: &ContextBarUi, last: &crate::genai_ui::GenAiLast) -> Value {
    let mut p = json!({ "prompt": bar.prompt.trim(), "context": !bar.area_only });
    if !last.model.trim().is_empty() {
        p["model"] = json!(last.model.trim());
    }
    if !last.resolution.is_empty() {
        p["resolution"] = json!(last.resolution);
    }
    p
}

enum Action {
    Expand,
    Collapse,
    Dismiss,
    Generate,
}

/// Draw the bar for the active document's selection (once a frame, after the canvas).
pub fn show(app: &mut PhotocraftApp, ctx: &Context) {
    let Some((key, _)) = target(app) else { return };
    let Some(xf) = ViewXform::active(app) else { return };
    let view = xf.rect;
    let id = egui::Id::unique("photocraft-context-bar");
    let size = ctx.memory(|m| m.area_rect(id)).map_or(vec2(260.0, 36.0), |r| r.size());
    // Docked at the bottom centre of the canvas (Photoshop's pinned position), not next to the
    // selection: right-clicks around the selection must keep reaching the canvas menus.
    let pos = pos2((view.center().x - size.x / 2.0).max(view.left() + 4.0), (view.bottom() - size.y - GAP).max(view.top() + 4.0));

    let t = crate::theme::Tokens::get(ctx);
    let busy = app.session.has_jobs();
    let mut bar = app.ui.context_bar.clone();
    if bar.expanded && bar.models.is_empty() {
        bar.models = crate::genai_ui::models(app);
    }
    // With context the model gets an image, so only models that edit are offered.
    let model_opts = crate::genai_ui::model_options(&bar.models, !bar.area_only, &app.ui.genai_last.model);
    let mut model = crate::genai_ui::fit_model(&model_opts, &app.ui.genai_last.model);
    let mut action = None;
    egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        egui::Frame::NONE
            .fill(t.card)
            .stroke(egui::Stroke::new(1.0, t.card_border))
            .corner_radius(t.radius)
            .inner_margin(egui::Margin::same(6))
            .shadow(egui::Shadow { offset: [0, 2], blur: 8, spread: 0, color: t.shadow })
            .show(ui, |ui| {
                if !bar.expanded {
                    ui.horizontal(|ui| {
                        let label = egui::RichText::new(tl!("Generate with AI")).color(egui::Color32::WHITE);
                        if ui.add_enabled(!busy, egui::Button::new(label).fill(t.accent)).clicked() {
                            action = Some(Action::Expand);
                        }
                        if ui.add(egui::Button::new(egui::RichText::new("×").color(t.text_dim)).frame(false)).on_hover_text(tl!("Close")).clicked() {
                            action = Some(Action::Dismiss);
                        }
                    });
                    return;
                }
                ui.set_width(460.0);
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut bar.area_only, false, tl!("With context"))
                        .on_hover_text(tl!("The AI sees the area around the selection, so the result blends into the image"));
                    ui.selectable_value(&mut bar.area_only, true, tl!("Selection only"))
                        .on_hover_text(tl!("The AI paints from your prompt alone, shaped like the selection"));
                    ui.add_space(8.0);
                    crate::genai_ui::model_dropdown(ui, "context-bar-model", &mut model, &model_opts, 160.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(egui::Button::new(egui::RichText::new("×").color(t.text_dim)).frame(false)).on_hover_text(tl!("Close")).clicked() {
                            action = Some(Action::Collapse);
                        }
                    });
                });
                ui.horizontal(|ui| {
                    let edit = ui.add(egui::TextEdit::singleline(&mut bar.prompt).hint_text(tl!("Describe what to generate")).desired_width(360.0));
                    if bar.focus {
                        edit.request_focus();
                        bar.focus = false;
                    }
                    let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let ready = !busy && !bar.prompt.trim().is_empty();
                    let go = egui::Button::new(egui::RichText::new(tl!("Generate")).color(egui::Color32::WHITE)).fill(t.accent);
                    if (ui.add_enabled(ready, go).clicked() || enter) && ready {
                        action = Some(Action::Generate);
                    }
                });
            });
    });
    // The chosen model is shared with the Generate Image / Generative Fill dialogs.
    if !model.is_empty() {
        app.ui.genai_last.model = model;
    }
    match action {
        Some(Action::Expand) => {
            bar.expanded = true;
            bar.focus = true;
        }
        Some(Action::Collapse) => bar.expanded = false,
        Some(Action::Dismiss) => {
            bar.dismissed = Some(key);
            bar.expanded = false;
        }
        Some(Action::Generate) => {
            let p = params(&bar, &app.ui.genai_last);
            app.ui.genai_last.prompt = bar.prompt.trim().to_string();
            bar.expanded = false;
            if let Err(e) = app.run(FILL, p) {
                app.ui.status = e;
                app.ui.status_error = true;
            }
        }
        None => {}
    }
    app.ui.context_bar = bar;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::genai_ui::GenAiLast;

    fn make_app() -> PhotocraftApp {
        photocraft_engine::genai_cmds::disable_network();
        let mut s = photocraft_engine::Session::new();
        s.execute("file.new", json!({"width": 64, "height": 48})).unwrap();
        PhotocraftApp::new(s, crate::Services::default())
    }

    #[test]
    fn the_two_modes_send_context_on_and_off() {
        let mut bar = ContextBarUi { prompt: "  a red kite  ".into(), ..Default::default() };
        let p = params(&bar, &GenAiLast::default());
        assert_eq!(p, json!({"prompt": "a red kite", "context": true}));
        bar.area_only = true;
        let last = GenAiLast { model: "nano-banana-2".into(), resolution: "2K".into(), ..Default::default() };
        assert_eq!(params(&bar, &last), json!({"prompt": "a red kite", "context": false, "model": "nano-banana-2", "resolution": "2K"}));
    }

    #[test]
    fn the_bar_follows_the_selection_and_close_hides_it_until_the_selection_changes() {
        let mut app = make_app();
        assert!(target(&mut app).is_none(), "no selection, no bar");
        app.run("select.rect", json!({"x": 4, "y": 4, "width": 20, "height": 10})).unwrap();
        let (key, b) = target(&mut app).unwrap();
        assert_eq!([b.x0, b.y0, b.x1, b.y1], [4, 4, 24, 14]);
        app.ui.context_bar.dismissed = Some(key);
        assert!(target(&mut app).is_none(), "closed for this selection");
        app.run("select.rect", json!({"x": 30, "y": 20, "width": 10, "height": 10})).unwrap();
        assert!(target(&mut app).is_some(), "a new selection brings it back");
        app.ui.panels.context_bar = false;
        assert!(target(&mut app).is_none(), "Window › Contextual Task Bar off");
    }

    #[test]
    fn the_bar_hides_while_a_dialog_is_open() {
        let mut app = make_app();
        app.run("select.rect", json!({"x": 4, "y": 4, "width": 20, "height": 10})).unwrap();
        app.ui.open_dialog(crate::state::DialogKind::Command, serde_json::Map::new());
        assert!(target(&mut app).is_none());
    }

    /// The whole app, rendered: the bar shows with a selection, not without, and expands.
    #[test]
    fn the_bar_appears_on_the_canvas_with_a_selection() {
        use egui_kittest::{Harness, kittest::Queryable};
        let build = |selection: bool| {
            let mut h = Harness::builder().with_size(vec2(1200.0, 800.0)).build_eframe(move |cc| {
                PhotocraftApp::setup_context(&cc.egui_ctx, Default::default());
                let mut app = make_app();
                if selection {
                    app.run("select.rect", json!({"x": 4, "y": 4, "width": 20, "height": 10})).unwrap();
                }
                app
            });
            h.run_steps(5);
            h
        };
        let label = crate::i18n::t("Generate with AI").to_string();
        assert!(build(false).query_by_label(&label).is_none());
        let mut h = build(true);
        assert!(h.query_by_label(&label).is_some(), "the bar shows under a selection");
        h.state_mut().ui.context_bar.expanded = true;
        h.run_steps(3);
        assert!(h.query_by_label(crate::i18n::t("With context")).is_some());
        assert!(h.query_by_label(crate::i18n::t("Selection only")).is_some());
        assert!(h.state().ui.context_bar.models.len() > 5, "the model list loads when the bar expands");
        assert_eq!(h.state().ui.genai_last.model, "nano-banana-2", "Nano Banana 2 is preselected");
    }
}
