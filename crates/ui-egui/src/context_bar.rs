//! Contextual task bar, as in Photoshop: a small floating bar under the active selection (any
//! selection tool) offering Generate with AI. Two modes, both `edit.generativeFill` as a
//! background job (status bar progress, Esc cancels):
//! - With context: the model sees the selection and the area around it, so the result blends
//!   into the image.
//! - Selection only: the prompt alone; a new image shaped like the selection fills it.
//!
//! The bar hides while a gesture, transform, crop, text edit or dialog is in progress. Its ×
//! hides it until the selection changes.
use crate::PhotocraftApp;
use crate::canvas::ViewXform;
use crate::state::ContextBarUi;
use egui::{Context, pos2, vec2};
use photocraft_engine::genai_cmds::FILL;
use photocraft_geom::Rect as DRect;
use serde_json::{Value, json};

/// Identifies one selection: its document and bounds (a new or changed selection shows the bar
/// again after ×).
pub type SelectionKey = (u64, [i32; 4]);

/// Gap between the selection and the bar, in points.
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
pub fn params(bar: &ContextBarUi, last: &crate::state::GenAiLast) -> Value {
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
    Deselect,
    Generate,
}

/// Draw the bar for the active document's selection (once a frame, after the canvas).
pub fn show(app: &mut PhotocraftApp, ctx: &Context) {
    let Some((key, b)) = target(app) else { return };
    let Some(xf) = ViewXform::active(app) else { return };
    let sel = xf.doc_rect(b);
    let view = xf.rect;
    if !view.intersects(sel) {
        return;
    }
    let id = egui::Id::new("photocraft-context-bar");
    let size = ctx.memory(|m| m.area_rect(id)).map_or(vec2(260.0, 36.0), |r| r.size());
    let mut pos = pos2(sel.center().x - size.x / 2.0, sel.max.y + GAP);
    if pos.y + size.y > view.bottom() - 4.0 {
        pos.y = sel.min.y - GAP - size.y;
    }
    pos.x = pos.x.min(view.right() - size.x - 4.0).max(view.left() + 4.0);
    pos.y = pos.y.min(view.bottom() - size.y - 4.0).max(view.top() + 4.0);

    let t = crate::theme::Tokens::get(ctx);
    let busy = app.session.has_jobs();
    let mut bar = app.ui.context_bar.clone();
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
                        let label = egui::RichText::new(format!("✦ {}", tl!("Generate with AI"))).color(egui::Color32::WHITE);
                        if ui.add_enabled(!busy, egui::Button::new(label).fill(t.accent)).clicked() {
                            action = Some(Action::Expand);
                        }
                        if ui.button(tl!("Deselect")).clicked() {
                            action = Some(Action::Deselect);
                        }
                        if ui.add(egui::Button::new(egui::RichText::new("×").color(t.text_dim)).frame(false)).on_hover_text(tl!("Close")).clicked() {
                            action = Some(Action::Dismiss);
                        }
                    });
                    return;
                }
                ui.set_width(380.0);
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut bar.area_only, false, tl!("With context"))
                        .on_hover_text(tl!("The AI sees the area around the selection, so the result blends into the image"));
                    ui.selectable_value(&mut bar.area_only, true, tl!("Selection only"))
                        .on_hover_text(tl!("The AI paints from your prompt alone, shaped like the selection"));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(egui::Button::new(egui::RichText::new("×").color(t.text_dim)).frame(false)).on_hover_text(tl!("Close")).clicked() {
                            action = Some(Action::Collapse);
                        }
                    });
                });
                ui.horizontal(|ui| {
                    let edit = ui.add(egui::TextEdit::singleline(&mut bar.prompt).hint_text(tl!("Describe what to generate")).desired_width(280.0));
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
        Some(Action::Deselect) => {
            if let Err(e) = app.run("select.deselect", json!({})) {
                app.ui.status = e;
                app.ui.status_error = true;
            }
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
    use crate::state::GenAiLast;

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

    #[test]
    fn showing_the_bar_on_a_selection_does_not_panic() {
        let mut app = make_app();
        app.run("select.rect", json!({"x": 4, "y": 4, "width": 20, "height": 10})).unwrap();
        let ctx = egui::Context::default();
        for expanded in [false, true] {
            app.ui.context_bar.expanded = expanded;
            let _ = ctx.run_ui(Default::default(), |ui| show(&mut app, ui.ctx()));
        }
    }
}
