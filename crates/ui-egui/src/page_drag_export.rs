//! Export a copy through the system file drag. The receiver chooses the destination;
//! neither completion nor cancellation ever applies an edit to the source document.

use std::path::PathBuf;

use crate::PdfCraftApp;

fn export_name(name: &str, pages: &[usize]) -> String {
    let stem = std::path::Path::new(name).file_stem().and_then(|s| s.to_str()).unwrap_or("document");
    let stem: String = stem.chars().filter(|c| !c.is_control() && !matches!(c, '/' | '\\' | ':')).take(60).collect();
    let stem = if stem.trim().is_empty() { "document" } else { stem.trim() };
    if let [page] = pages { format!("{stem} (page {}).pdf", page.saturating_add(1)) } else { format!("{stem} ({} pages).pdf", pages.len()) }
}

/// A small original paper icon, generated in memory; no external artwork or fonts.
fn preview() -> Result<Vec<u8>, String> {
    let mut pixels = Vec::with_capacity(48 * 64 * 4);
    for y in 0..64 {
        for x in 0..48 {
            let edge = !(2..46).contains(&x) || !(2..62).contains(&y);
            let line = (10..38).contains(&x) && [18, 19, 28, 29, 38, 39, 48, 49].contains(&y);
            pixels.extend_from_slice(if edge || line { &[64, 104, 160, 255] } else { &[255, 255, 255, 240] });
        }
    }
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 48, 64);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header().map_err(|e| e.to_string())?.write_image_data(&pixels).map_err(|e| e.to_string())?;
    }
    Ok(bytes)
}

impl PdfCraftApp {
    fn prepare_drag_export(&mut self, pages: &[usize]) -> Result<(tempfile::TempDir, PathBuf), String> {
        if !self.commit_form_typing() {
            return Err("表单内容未能保存，请先检查正在编辑的字段。".into());
        }
        let (_, id) = self.active_ids().ok_or("没有打开的文档")?;
        let doc = self.session.get(id).ok_or("文档已关闭")?;
        let name = export_name(&doc.name, pages);
        // Session::extract enforces assembly permission and preserves the current edited pages.
        let bytes = self.session.extract(id, pages).map_err(|e| e.to_string())?;
        let folder = tempfile::Builder::new().prefix("pdfcraft-page-drag-").tempdir().map_err(|e| e.to_string())?;
        let path = folder.path().join(name);
        std::fs::write(&path, bytes.as_ref()).map_err(|e| e.to_string())?;
        Ok((folder, path))
    }

    pub(crate) fn process_page_drag_export(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let Some(pages) = self.active.and_then(|i| self.views.get_mut(i)).and_then(|v| v.export_drag.take()) else { return };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let (folder, path) = self.prepare_drag_export(&pages)?;
            let icon = preview()?;
            let window = frame.winit_window().ok_or("无法取得拖动窗口")?;
            let repaint = ctx.clone();
            let viewport = ctx.viewport_id();
            let finished = self.drag_export_finished.clone();
            // Keep the exported copy alive while Finder or another receiver reads it. TempDir
            // removes it when this document window closes; no user-selected path is removed.
            self.drag_exports.push(folder);
            drag::start_drag(
                window.as_ref(),
                drag::DragItem::Files(vec![path]),
                drag::Image::Raw(icon),
                move |_, _| {
                    finished.store(true, std::sync::atomic::Ordering::Release);
                    repaint.request_repaint_of(viewport);
                },
                drag::Options { mode: drag::DragMode::Copy, ..Default::default() },
            )
            .map_err(|e| e.to_string())
        }));
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => self.notify(format!("页面拖出失败：{error}")),
            Err(_) => self.notify("页面拖出失败，请重试。原文档未因拖出而改变。"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exported_pages_preserve_source_order_dirty_state_and_current_edits() {
        let mut app = PdfCraftApp::new();
        let bytes = app.session.create_blank(200.0, 300.0, 4).unwrap();
        app.open_bytes("测试.pdf", None, bytes.as_ref().clone()).unwrap();
        app.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![1], degrees: 90 });
        let (_, id) = app.active_ids().unwrap();
        let before = app.session.get(id).unwrap().bytes.clone();
        let generation = app.session.get(id).unwrap().edit_generation();
        let (folder, path) = app.prepare_drag_export(&[1]).unwrap();
        assert_eq!(path.file_name().unwrap(), "测试 (page 2).pdf");
        assert_eq!(app.session.get(id).unwrap().bytes, before);
        assert_eq!(app.session.get(id).unwrap().edit_generation(), generation);
        assert_eq!(app.session.get(id).unwrap().info.pages.len(), 4);
        let exported = std::fs::read(&path).unwrap();
        app.open_bytes("export.pdf", None, exported).unwrap();
        let (_, exported_id) = app.active_ids().unwrap();
        let info = &app.session.get(exported_id).unwrap().info;
        assert_eq!(info.pages.len(), 1);
        assert_eq!(info.pages[0].width, 300.0);
        assert_eq!(info.pages[0].height, 200.0);
        drop(folder);
        assert!(!path.exists());
    }

    #[test]
    fn export_name_is_a_bounded_single_filename_and_preview_is_valid() {
        let name = export_name(&format!("{}:file.pdf", "页".repeat(200)), &[0, 2]);
        assert!(name.len() < 250 && !name.contains(':'));
        assert_eq!(std::path::Path::new(&name).components().count(), 1);
        let bytes = preview().unwrap();
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let reader = decoder.read_info().unwrap();
        assert_eq!((reader.info().width, reader.info().height), (48, 64));
    }
}
