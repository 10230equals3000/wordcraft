//! A scratch pad for Insert › Ink Equation. Its ink is kept outside the document until Insert
//! succeeds, so clearing, correcting or cancelling the drawing never edits the document.

use egui::{Color32, Pos2, Rect, Sense, Stroke, Ui, vec2};
use serde::Serialize;
use serde_json::json;
use wordcraft_doc::freeform::MAX_POINTS;
use wordcraft_engine::cmd::ink_math::{self, MAX_STROKES, MAX_TOTAL_POINTS};

use super::{Tokens, WordApp, buttons};

#[derive(Clone, Debug, Default, Serialize)]
pub struct InkEquationForm {
    strokes: Vec<Vec<[f32; 2]>>,
    drawing: bool,
    limited: bool,
    preview: Option<String>,
    message: String,
}

impl InkEquationForm {
    fn points(&self) -> usize {
        self.strokes.iter().map(Vec::len).sum()
    }

    fn begin(&mut self, point: [f32; 2]) {
        if self.limited || self.strokes.len() >= MAX_STROKES || self.points() >= MAX_TOTAL_POINTS {
            self.limited = true;
            self.preview = None;
            self.message = tl!("Ink limit reached. Undo a stroke or clear the drawing.").to_string();
            return;
        }
        self.strokes.push(vec![point]);
        self.drawing = true;
        self.preview = None;
        self.message.clear();
    }

    fn sample(&mut self, point: [f32; 2]) {
        if !self.drawing {
            return;
        }
        let total = self.points();
        let Some(stroke) = self.strokes.last_mut() else { return };
        if stroke.last().is_some_and(|p| (p[0] - point[0]).hypot(p[1] - point[1]) <= 0.5) {
            return;
        }
        if stroke.len() >= MAX_POINTS || total >= MAX_TOTAL_POINTS {
            self.drawing = false;
            self.limited = true;
            self.preview = None;
            self.message = tl!("Ink limit reached. Undo a stroke or clear the drawing.").to_string();
            return;
        }
        stroke.push(point);
    }

    fn finish(&mut self) {
        self.drawing = false;
        self.update_preview();
    }

    fn update_preview(&mut self) {
        let strokes: Vec<ink_math::Stroke> = self.strokes.iter().map(|pts| ink_math::Stroke { pts: pts.clone() }).collect();
        self.preview = (!self.limited).then(|| ink_math::recognise(&strokes)).flatten();
        self.message.clear();
    }

    fn undo(&mut self) {
        self.strokes.pop();
        self.drawing = false;
        self.limited = false;
        self.update_preview();
    }

    fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Convert a pointer position to the scratch pad's local points, clamped while dragging out of it.
fn local(rect: Rect, at: Pos2) -> [f32; 2] {
    [(at.x - rect.left()).clamp(0.0, rect.width()), (at.y - rect.top()).clamp(0.0, rect.height())]
}

fn pad(ui: &mut Ui, form: &mut InkEquationForm) {
    let (rect, response) = ui.allocate_exact_size(vec2(440.0, 180.0), Sense::click_and_drag());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, ui.is_enabled(), tl!("Ink equation drawing area")));
    let (pressed, down, released, at) =
        ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.primary_released(), i.pointer.latest_pos()));
    if pressed
        && response.contains_pointer()
        && let Some(at) = at
    {
        form.begin(local(rect, at));
    }
    if form.drawing
        && (down || released)
        && let Some(at) = at
    {
        form.sample(local(rect, at));
    }
    if form.drawing && released {
        form.finish();
    }
    let theme = Tokens::get(ui.ctx());
    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, 3.0, theme.input);
    painter.rect_stroke(rect, 3.0, Stroke::new(1.0, theme.input_border), egui::StrokeKind::Inside);
    let ink: Color32 = theme.text;
    for stroke in &form.strokes {
        let pts: Vec<Pos2> = stroke.iter().map(|[x, y]| rect.min + vec2(*x, *y)).collect();
        if pts.len() == 1 {
            if let Some(p) = pts.first() {
                painter.circle_filled(*p, 1.2, ink);
            }
        } else if pts.len() > 1 {
            painter.add(egui::Shape::line(pts, Stroke::new(2.0, ink)));
        }
    }
}

pub(super) fn body(app: &mut WordApp, ui: &mut Ui, form: &mut InkEquationForm) -> bool {
    ui.label(tl!("Write an equation with the mouse, pen, or touch."));
    ui.label(tl!("Supports 0, 1, +, −, x, =, /, ÷, ≠, √ and simple fractions or scripts."));
    pad(ui, form);
    ui.horizontal(|ui| {
        if ui.add_enabled(!form.strokes.is_empty(), egui::Button::new(tl!("Undo Stroke"))).clicked() {
            form.undo();
        }
        if ui.add_enabled(!form.strokes.is_empty(), egui::Button::new(tl!("Clear"))).clicked() {
            form.clear();
        }
    });
    ui.horizontal(|ui| {
        ui.label(tl!("Recognized:"));
        ui.label(form.preview.as_deref().unwrap_or(tl!("Not recognized yet")));
    });
    if !form.message.is_empty() {
        ui.colored_label(Tokens::get(ui.ctx()).text_dim, &form.message);
    }
    let (insert, cancel) = buttons(ui, tl!("Insert"));
    if cancel {
        return true;
    }
    if insert {
        if form.preview.is_none() || form.limited {
            form.message = tl!("The ink could not be recognized as an equation.").to_string();
            return false;
        }
        let strokes: Vec<_> = form.strokes.iter().map(|points| json!({"points": points})).collect();
        match app.run("insert.inkEquation", json!({"strokes": strokes})) {
            Ok(_) => {
                app.canvas.want_focus = true;
                return true;
            }
            Err(error) => form.message = error,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialogs::Dialog;
    use egui_kittest::kittest::Queryable;
    use wordcraft_doc::{Document, para::InlineObject};
    use wordcraft_engine::Session;

    fn app() -> WordApp {
        WordApp::new(Session::new(Document::new()), crate::Services::default())
    }

    #[test]
    fn user_command_opens_pad_but_programmatic_calls_still_need_strokes() {
        let mut app = app();
        assert!(app.execute("insert.inkEquation", json!({})).is_err());
        assert!(app.dialog.is_none());
        assert_eq!(app.run("insert.inkEquation", json!({})).unwrap(), json!({"pending": "inkEquation"}));
        assert!(matches!(app.dialog, Some(Dialog::InkEquation { .. })));
    }

    #[test]
    fn equation_gallery_opens_the_ink_pad() {
        let mut fonts = false;
        let mut h = egui_kittest::Harness::builder().with_size(vec2(500.0, 800.0)).build_ui_state(
            move |ui, app: &mut WordApp| {
                if !fonts {
                    crate::theme::install_fonts(ui.ctx());
                    fonts = true;
                    return;
                }
                crate::equation_tab::builtin_menu(ui, app);
            },
            app(),
        );
        h.run_steps(3);
        h.get_by_label("Ink Equation").click();
        h.run_steps(2);
        assert!(matches!(h.state().dialog, Some(Dialog::InkEquation { .. })));
    }

    #[test]
    fn handwriting_pad_inserts_undoably_and_keeps_failed_ink() {
        let mut app = app();
        app.run("insert.inkEquation", json!({})).unwrap();
        let mut h = egui_kittest::Harness::builder().with_size(vec2(1440.0, 900.0)).build_ui_state(
            |ui, app: &mut WordApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
        h.run_steps(4);
        let rect = h.get_by_label("Ink equation drawing area").rect();
        let from = rect.min + vec2(70.0, 30.0);
        let to = rect.min + vec2(70.0, 65.0);
        h.event(egui::Event::PointerMoved(from));
        h.event(egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
        h.step();
        h.event(egui::Event::PointerMoved(to));
        h.step();
        h.event(egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
        h.run_steps(3);
        let Some(Dialog::InkEquation { form }) = &h.state().dialog else { panic!("the pad should still be open") };
        assert_eq!(form.preview.as_deref(), Some("1"));
        assert!(h.state().session.doc.body.iter().filter_map(|b| b.as_para()).all(|p| p.objects.is_empty()));
        let dialog = h.ctx.memory(|m| m.area_rect(egui::Id::unique(("dialog", "Ink Equation")))).unwrap();
        h.query_all_by_label("Insert").find(|n| dialog.contains(n.rect().center())).unwrap().click();
        h.run_steps(3);
        assert!(h.state().dialog.is_none());
        let eq = h.state().session.doc.body.iter().filter_map(|b| b.as_para()).flat_map(|p| &p.objects).find_map(|o| match o {
            InlineObject::Equation { linear, .. } => Some(linear.as_str()),
            _ => None,
        });
        assert_eq!(eq, Some("1"));
        h.state_mut().run("edit.undo", json!({})).unwrap();
        assert!(h.state().session.doc.body.iter().filter_map(|b| b.as_para()).all(|p| p.objects.is_empty()));

        // An unrecognized tap is retained for correction, never inserted as an empty equation.
        h.state_mut().run("insert.inkEquation", json!({})).unwrap();
        h.run_steps(3);
        let rect = h.get_by_label("Ink equation drawing area").rect();
        let at = rect.center();
        h.event(egui::Event::PointerMoved(at));
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
        h.step();
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
        h.run_steps(3);
        let Some(Dialog::InkEquation { form }) = &h.state().dialog else { panic!("the pad should still be open") };
        assert_eq!(form.strokes.len(), 1);
        assert!(form.preview.is_none());
        let dialog = h.ctx.memory(|m| m.area_rect(egui::Id::unique(("dialog", "Ink Equation")))).unwrap();
        h.query_all_by_label("Insert").find(|n| dialog.contains(n.rect().center())).unwrap().click();
        h.run_steps(3);
        let Some(Dialog::InkEquation { form }) = &h.state().dialog else { panic!("failed recognition must keep the pad open") };
        assert_eq!(form.strokes.len(), 1);
        assert!(!form.message.is_empty());
        assert!(h.state().session.doc.body.iter().filter_map(|b| b.as_para()).all(|p| p.objects.is_empty()));
    }
}
