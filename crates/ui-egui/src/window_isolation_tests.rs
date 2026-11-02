//! Real navigation controls in separate native viewports sharing one egui context.

use crate::{PdfCraftApp, chrome, docking, motion};
use egui::{Event, Key, Modifiers, ViewportId};

fn app() -> PdfCraftApp {
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    let bytes = app.session.create_blank(200.0, 300.0, 12).unwrap();
    app.open_bytes("navigation.pdf", None, bytes.as_ref().clone()).unwrap();
    app
}

struct Windows {
    ctx: egui::Context,
    time: f64,
}

impl Windows {
    fn new() -> Self {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        egui_extras::install_image_loaders(&ctx);
        ctx.enable_accesskit();
        Self { ctx, time: 0.0 }
    }

    fn frame(&mut self, app: &mut PdfCraftApp, id: ViewportId, events: Vec<Event>, command: Option<&str>) -> egui::FullOutput {
        self.time += 0.1;
        let mut raw = egui::RawInput {
            viewport_id: id,
            time: Some(self.time),
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0))),
            events,
            ..Default::default()
        };
        raw.viewports.insert(id, egui::ViewportInfo::default());
        let mut out = self.ctx.run_ui(raw, |ui| {
            app.ctx = Some(ui.ctx().clone());
            app.motion.begin_pass(ui.ctx(), app.animation_effects);
            if let Some(command) = command {
                assert!(app.execute(command));
            }
            app.shortcuts(ui.ctx());
            chrome::right_rail(app, ui);
            egui::CentralPanel::default().show(ui, |ui| docking::show(app, ui));
        });
        out.textures_delta.clear();
        out
    }

    fn click(&mut self, app: &mut PdfCraftApp, id: ViewportId, at: egui::Pos2) {
        self.frame(app, id, vec![Event::PointerMoved(at)], None);
        for pressed in [true, false] {
            self.frame(
                app,
                id,
                vec![Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE }],
                None,
            );
        }
    }
}

fn key(key: Key, modifiers: Modifiers) -> Event {
    Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
}

fn control(out: &egui::FullOutput, label: &str) -> egui::Pos2 {
    let update = out.platform_output.accesskit_update.as_ref().unwrap();
    let rect = update
        .nodes
        .iter()
        .find_map(|(_, node)| {
            (node.label().is_some_and(|text| text == label || text.starts_with(&format!("{label} (")))).then(|| node.bounds()).flatten()
        })
        .unwrap_or_else(|| panic!("missing control {label}"));
    egui::pos2(((rect.x0 + rect.x1) / 2.0) as f32, ((rect.y0 + rect.y1) / 2.0) as f32)
}

#[test]
fn undo_in_an_unedited_page_box_cannot_restore_another_windows_page_number() {
    let mut windows = Windows::new();
    let (mut first, mut second) = (app(), app());
    let child = ViewportId::from_hash_of("second-document");
    second.views[0].go_to_page(2);
    windows.frame(&mut first, ViewportId::ROOT, Vec::new(), None);
    windows.frame(&mut second, child, Vec::new(), Some("view.focus_page_input"));
    windows.frame(&mut second, child, vec![key(Key::Z, Modifiers::COMMAND)], None);
    assert_eq!(second.views[0].page_input, "3", "undo in an unedited field must not import the other PDF's page number");
    assert_eq!(first.views[0].current, 0);
}

#[test]
fn undo_in_find_cannot_import_another_windows_query() {
    let mut windows = Windows::new();
    let (mut first, mut second) = (app(), app());
    let child = ViewportId::from_hash_of("second-document");
    for (app, query) in [(&mut first, "alpha"), (&mut second, "beta")] {
        app.views[0].open_find();
        app.views[0].find.as_mut().unwrap().query = query.into();
    }
    windows.frame(&mut first, ViewportId::ROOT, Vec::new(), None);
    windows.frame(&mut second, child, Vec::new(), None);
    windows.frame(&mut second, child, vec![key(Key::Z, Modifiers::COMMAND)], None);
    assert_eq!(second.views[0].find.as_ref().unwrap().query, "beta", "find undo belongs to its own PDF window");
    assert_eq!(first.views[0].find.as_ref().unwrap().query, "alpha");
}

#[test]
fn bookmark_filter_does_not_hide_another_windows_bookmarks() {
    let mut windows = Windows::new();
    let (mut first, mut second) = (app(), app());
    let child = ViewportId::from_hash_of("second-document");
    for app in [&mut first, &mut second] {
        app.apply_edit(pdfcraft_engine::Edit::AddBookmark { parent: Vec::new(), index: 0, title: "Chapter Alpha".into(), page: 0 });
        app.set_option("panel", "bookmarks").unwrap();
    }
    let out = windows.frame(&mut first, ViewportId::ROOT, Vec::new(), None);
    windows.click(&mut first, ViewportId::ROOT, control(&out, "Search"));
    windows.frame(&mut first, ViewportId::ROOT, vec![Event::Text("missing".into())], None);
    let out = windows.frame(&mut second, child, Vec::new(), None);
    let painted: Vec<_> = out
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.text()),
            _ => None,
        })
        .collect();
    assert!(painted.contains(&"Chapter Alpha"), "another window's query hid this PDF's bookmark: {painted:?}");
}

#[test]
fn reduced_motion_zoom_cache_cannot_zoom_another_viewport() {
    let ctx = egui::Context::default();
    let child = ViewportId::from_hash_of("second-document");
    let mut first = motion::Motion::default();
    let mut second = motion::Motion::default();
    ctx.run_ui(egui::RawInput { events: vec![Event::Zoom(1.25)], ..Default::default() }, |ui| {
        first.begin_pass(ui.ctx(), motion::AnimationEffects::Off);
        assert_eq!(motion::zoom_delta(ui.ctx()), 1.25);
        let mut raw = egui::RawInput { viewport_id: child, ..Default::default() };
        raw.viewports.insert(child, egui::ViewportInfo::default());
        ctx.run_ui(raw, |child_ui| {
            second.begin_pass(child_ui.ctx(), motion::AnimationEffects::On);
            assert_eq!(motion::zoom_delta(child_ui.ctx()), 1.0, "a viewport without a gesture must not inherit another window's zoom");
        })
        .textures_delta
        .clear();
    })
    .textures_delta
    .clear();
}

#[test]
fn navigation_buttons_and_shortcuts_change_only_their_own_window() {
    let child = ViewportId::from_hash_of("second-document");
    for (target, other) in [(ViewportId::ROOT, child), (child, ViewportId::ROOT), (child, ViewportId::from_hash_of("third-document"))] {
        check_navigation(target, other);
    }
}

fn check_navigation(target: ViewportId, other: ViewportId) {
    let mut windows = Windows::new();
    let (mut first, mut second) = (app(), app());
    let out = windows.frame(&mut first, target, Vec::new(), None);
    windows.frame(&mut second, other, Vec::new(), None);
    let untouched = (second.views[0].current, second.views[0].zoom, second.views[0].fit, second.views[0].rotation);
    let next = control(&out, "Next page");
    let zoom = control(&out, "Zoom in");
    windows.click(&mut first, target, next);
    assert_eq!(first.views[0].current, 1);
    let before = first.views[0].zoom;
    windows.click(&mut first, target, zoom);
    assert!(first.views[0].zoom > before);
    // Keyboard navigation is addressed to the document rather than a focused rail button.
    let document = first.views[0].viewport_rect().center();
    windows.click(&mut first, target, document);
    for (key_code, modifiers) in
        [(Key::ArrowRight, Modifiers::NONE), (Key::Plus, Modifiers::COMMAND), (Key::Plus, Modifiers::COMMAND | Modifiers::SHIFT)]
    {
        let changed_before = (first.views[0].current, first.views[0].zoom, first.views[0].rotation);
        windows.frame(&mut first, target, vec![key(key_code, modifiers)], None);
        windows.frame(&mut second, other, Vec::new(), None);
        if key_code == Key::ArrowRight {
            assert_eq!(first.views[0].current, changed_before.0 + 1, "the target PDF must receive the page key");
        } else if modifiers.shift {
            assert_ne!(first.views[0].rotation, changed_before.2, "the target PDF must rotate");
        } else {
            assert!(first.views[0].zoom > changed_before.1, "the target PDF must receive the zoom shortcut");
        }
        assert_eq!((second.views[0].current, second.views[0].zoom, second.views[0].fit, second.views[0].rotation), untouched);
    }
    windows.frame(&mut first, target, Vec::new(), Some("view.focus_page_input"));
    windows.frame(&mut first, target, vec![Event::Text("5".into())], None);
    windows.frame(&mut first, target, vec![key(Key::Enter, Modifiers::NONE)], None);
    windows.frame(&mut second, other, Vec::new(), None);
    assert_eq!(first.views[0].current, 4, "the page-number shortcut and Enter must address the target window's field");
    assert_eq!((second.views[0].current, second.views[0].zoom, second.views[0].fit, second.views[0].rotation), untouched);
}

#[test]
fn pinch_and_command_wheel_zoom_only_the_hovered_window() {
    let child = ViewportId::from_hash_of("second-document");
    for (target, other) in [(ViewportId::ROOT, child), (child, ViewportId::ROOT)] {
        for effects in ["on", "off"] {
            for gesture in [
                Event::Zoom(1.2),
                Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, 40.0),
                    phase: egui::TouchPhase::Move,
                    modifiers: Modifiers::COMMAND,
                },
            ] {
                let mut windows = Windows::new();
                let (mut first, mut second) = (app(), app());
                first.set_option("animation_effects", effects).unwrap();
                second.set_option("animation_effects", effects).unwrap();
                first.set_option("zoom", "150").unwrap();
                second.set_option("zoom", "80").unwrap();
                windows.frame(&mut first, target, Vec::new(), None);
                windows.frame(&mut second, other, Vec::new(), None);
                let pointer = first.views[0].viewport_rect().center();
                let untouched = (second.views[0].current, second.views[0].zoom, second.views[0].page_screen_rect(0));
                windows.frame(&mut first, target, vec![Event::PointerMoved(pointer), gesture], None);
                for _ in 0..5 {
                    windows.frame(&mut first, target, Vec::new(), None);
                    windows.frame(&mut second, other, Vec::new(), None);
                }
                assert!(first.views[0].zoom > 1.5, "the target window must receive the gesture");
                assert_eq!((second.views[0].current, second.views[0].zoom, second.views[0].page_screen_rect(0)), untouched);
            }
        }
    }
}

#[test]
fn single_page_wheel_turns_only_the_hovered_windows_page() {
    let child = ViewportId::from_hash_of("second-document");
    for (target, other) in [(ViewportId::ROOT, child), (child, ViewportId::ROOT)] {
        let mut windows = Windows::new();
        let (mut first, mut second) = (app(), app());
        for app in [&mut first, &mut second] {
            app.set_option("layout", "single").unwrap();
            app.set_option("zoom", "50").unwrap();
        }
        windows.frame(&mut first, target, Vec::new(), None);
        windows.frame(&mut second, other, Vec::new(), None);
        let pointer = first.views[0].viewport_rect().center();
        windows.frame(
            &mut first,
            target,
            vec![
                Event::PointerMoved(pointer),
                Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Line,
                    delta: egui::vec2(0.0, -1.0),
                    phase: egui::TouchPhase::Move,
                    modifiers: Modifiers::NONE,
                },
            ],
            None,
        );
        windows.frame(&mut second, other, Vec::new(), None);
        assert_eq!(first.views[0].current, 1);
        assert_eq!(second.views[0].current, 0);
    }
}

#[test]
fn comparing_a_copy_keeps_other_document_windows_and_normal_opens_independent() {
    let mut windows = Windows::new();
    let (mut first, mut second) = (app(), app());
    let child = ViewportId::from_hash_of("comparison-window");
    let (sender, receiver) = std::sync::mpsc::channel();
    first.single_document_window = true;
    first.window_open = Some(sender);
    let newer = first.active_ids().unwrap().1;
    let old_bytes = second.session.get(second.views[0].id).unwrap().bytes.as_ref().clone();
    first.use_files(crate::files::FilePurpose::Compare, vec![("old-copy.pdf".into(), old_bytes.clone())]);
    assert_eq!(first.active_ids().unwrap().1, newer);
    assert_eq!(first.views.len(), 2);
    assert!(receiver.try_recv().is_err(), "comparison copies stay inside their requesting window");
    first.dialog = None;
    first.run_compare();
    first.compare.as_mut().unwrap().side_by_side = true;
    let old = first.compare.as_ref().unwrap().old;
    let old_index = first.views.iter().position(|v| v.id == old).unwrap();
    assert!(first.session.get(old).unwrap().path.is_none(), "the reference cannot overwrite its source file");
    for _ in 0..3 {
        windows.frame(&mut first, ViewportId::ROOT, Vec::new(), None);
        windows.frame(&mut second, child, Vec::new(), None);
    }
    let untouched = (second.views[0].current, second.views[0].zoom, second.views[0].page_screen_rect(0));
    let new_index = first.active_ids().unwrap().0;
    first.views[new_index].set_zoom(1.5);
    first.views[new_index].go_to_page(4);
    for _ in 0..5 {
        windows.frame(&mut first, ViewportId::ROOT, Vec::new(), None);
        windows.frame(&mut second, child, Vec::new(), None);
    }
    assert_eq!(first.views[old_index].zoom, first.views[new_index].zoom);
    assert_eq!((second.views[0].current, second.views[0].zoom, second.views[0].page_screen_rect(0)), untouched);
    assert_eq!(second.session.get(second.views[0].id).unwrap().bytes.as_ref(), &old_bytes);
    first.open_bytes("another.pdf", None, old_bytes).unwrap();
    assert!(matches!(receiver.try_recv(), Ok(crate::WindowOpen::Bytes { .. })), "ordinary opens still create separate windows");
    assert_eq!(first.views.len(), 2);
}

#[test]
fn comparison_copies_are_replaced_and_close_with_their_subject() {
    let mut first = app();
    first.single_document_window = true;
    let subject = first.views[0].id;
    let bytes = first.session.get(subject).unwrap().bytes.as_ref().clone();
    let mut previous = None;
    for _ in 0..4 {
        first.use_files(crate::files::FilePurpose::Compare, vec![("older.pdf".into(), bytes.clone())]);
        let reference = first.comparison_copy.unwrap().1;
        assert_eq!(first.views.len(), 2, "one subject and one reference, regardless of repeated selection");
        assert_eq!(first.active_ids().unwrap().1, subject);
        if let Some(old) = previous {
            assert!(first.session.get(old).is_none());
        }
        previous = Some(reference);
    }
    assert!(first.first_dirty().is_none());
    first.close_tab(first.active_ids().unwrap().0);
    assert!(first.views.is_empty());
    assert!(first.active.is_none());
    assert!(first.window_closed);
    assert!(first.session.get(subject).is_none());
    assert!(first.session.get(previous.unwrap()).is_none());
    assert!(first.comparison_copy.is_none());
}

#[test]
fn reference_pane_ignores_edit_tools_without_changing_subject_or_reference() {
    let mut windows = Windows::new();
    let mut first = app();
    let subject = first.views[0].id;
    let bytes = first.session.get(subject).unwrap().bytes.as_ref().clone();
    first.open_comparison_copy("older.pdf", bytes.clone()).unwrap();
    first.dialog = None;
    first.run_compare();
    first.compare.as_mut().unwrap().side_by_side = true;
    let reference = first.comparison_copy.unwrap().1;
    let ri = first.views.iter().position(|view| view.id == reference).unwrap();
    for tool in [crate::QuickTool::AddText, crate::QuickTool::EditText, crate::QuickTool::Comment(crate::comments::CommentTool::Note)] {
        first.quick_tool = tool;
        windows.frame(&mut first, ViewportId::ROOT, Vec::new(), None);
        let point = first.views[ri].viewport_rect().center();
        windows.click(&mut first, ViewportId::ROOT, point);
        windows.frame(&mut first, ViewportId::ROOT, vec![Event::Text("reference must not edit".into())], None);
        assert_eq!(first.active_ids().unwrap().1, subject);
        for view in &first.views {
            assert!(view.pending_edit.is_none());
            assert!(view.content.draft.is_none());
            assert!(view.line_editor.is_none());
            assert!(!first.session.get(view.id).unwrap().dirty);
            assert_eq!(first.session.get(view.id).unwrap().bytes.as_ref(), &bytes);
        }
        assert_eq!(first.quick_tool, tool, "reference cannot switch the subject's tool");
    }
}

#[test]
fn encrypted_comparison_copy_keeps_subject_through_wrong_password_cancel_and_retry() {
    use std::sync::Arc;
    for asynchronous in [false, true] {
        let mut first = app();
        first.single_document_window = true;
        if asynchronous {
            first.ctx = Some(egui::Context::default());
        }
        let subject = first.views[0].id;
        let bytes = first.session.get(subject).unwrap().bytes.clone();
        let mut encrypted = pdfcraft_cos::Document::open(bytes).unwrap();
        encrypted
            .set_encryption(&pdfcraft_cos::NewEncryption {
                algorithm: pdfcraft_cos::Algorithm::Aes256,
                user_password: "comparison-password",
                owner_password: "comparison-owner",
                permissions: -1,
                encrypt_metadata: true,
                seed: [9; 32],
            })
            .unwrap();
        let bytes = Arc::new(pdfcraft_cos::write_full(&encrypted, &Default::default()).unwrap());
        let settle = |app: &mut PdfCraftApp| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while !app.pending_opens.is_empty() {
                app.poll_pending_opens();
                assert!(std::time::Instant::now() < deadline, "password worker did not complete");
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        first.open_comparison_copy("encrypted-old.pdf", bytes.as_ref().clone()).unwrap();
        assert_eq!(first.password_prompt.as_ref().unwrap().comparison_subject, Some(subject));
        assert!(first.dialog.is_none(), "comparison dialog must not block password completion");
        first.submit_password(Some("incorrect".into()));
        settle(&mut first);
        assert!(first.password_prompt.as_ref().unwrap().error.is_some());
        assert_eq!(first.password_prompt.as_ref().unwrap().comparison_subject, Some(subject));
        assert_eq!(first.views.len(), 1);
        first.submit_password(None);
        assert!(first.password_prompt.is_none());
        assert!(first.comparison_copy.is_none());
        first.open_comparison_copy("encrypted-old.pdf", bytes.as_ref().clone()).unwrap();
        first.submit_password(Some("comparison-password".into()));
        settle(&mut first);
        assert!(first.password_prompt.is_none());
        assert_eq!(first.active_ids().unwrap().1, subject);
        assert_eq!(first.views.len(), 2);
        assert_eq!(first.comparison_copy.unwrap().0, subject);
        assert_eq!(first.dialog, Some(crate::Dialog::CompareFiles));
        first.run_compare();
        assert!(first.compare.is_some());
    }
}
