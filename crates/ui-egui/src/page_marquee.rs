//! 两种页面缩略图视图共用的框选手势。坐标相对滚动内容，选择只影响视图。
use crate::{DocView, theme::Tokens};
use egui::{Modifiers, Pos2, Rect, Sense, Stroke, Vec2, pos2, vec2};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Surface {
    Panel,
    Organize,
}

pub(crate) struct PageMarquee {
    surface: Surface,
    start: Pos2,
    press: Pos2,
    width: f32,
    before: BTreeSet<usize>,
    anchor: Option<usize>,
    modifiers: Modifiers,
    active: bool,
    pub(crate) seen: u64,
}

pub(crate) fn cancel(view: &mut DocView) {
    if let Some(state) = view.page_marquee.take() {
        view.selected = state.before;
        view.select_anchor = state.anchor;
    }
}

/// 在卡片之前注册背景；卡片、按钮和滚动条获得较高的指针优先级。
pub(crate) fn background(ui: &mut egui::Ui, surface: Surface) -> egui::Response {
    ui.interact(ui.clip_rect(), ui.id().with(("page-marquee", surface as u8)), Sense::click_and_drag())
}

/// `targets` 包含全部页面的几何位置，不能仅传可见行。控件所在区域不可起框。
#[allow(clippy::too_many_arguments)]
pub(crate) fn update(
    ui: &mut egui::Ui,
    view: &mut DocView,
    surface: Surface,
    origin: Pos2,
    background: &egui::Response,
    targets: impl IntoIterator<Item = (usize, Rect)>,
    excluded: &[Rect],
    enabled: bool,
    t: &Tokens,
) {
    if view.page_marquee.as_ref().is_some_and(|s| s.surface != surface) {
        return;
    }
    if !enabled || !ui.is_enabled() || !ui.input(|i| i.focused) {
        cancel(view);
        return;
    }
    let clip = ui.clip_rect();
    let (point, pressed, down, released, modifiers) = ui.input(|i| {
        let pressed_modifiers = i
            .events
            .iter()
            .find_map(|event| match event {
                egui::Event::PointerButton { button: egui::PointerButton::Primary, pressed: true, modifiers, .. } => Some(*modifiers),
                _ => None,
            })
            .unwrap_or(i.modifiers);
        (i.pointer.interact_pos(), i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.primary_released(), pressed_modifiers)
    });
    if view.page_marquee.is_none() {
        if !pressed || view.panel_drag.is_some() || view.org_drag.is_some() || view.export_drag.is_some() {
            return;
        }
        let Some(p) = point.filter(|p| clip.contains(*p)) else { return };
        if !background.contains_pointer() || excluded.iter().any(|r| r.contains(p)) {
            return;
        }
        view.page_marquee = Some(PageMarquee {
            surface,
            start: p - origin.to_vec2(),
            press: p,
            width: clip.width(),
            before: view.selected.clone(),
            anchor: view.select_anchor,
            modifiers,
            active: false,
            seen: ui.ctx().cumulative_frame_nr(),
        });
    }
    if view.page_marquee.as_ref().is_some_and(|s| (s.width - clip.width()).abs() > 1.0) {
        cancel(view);
        return;
    }
    let Some(state) = view.page_marquee.as_mut() else { return };
    state.seen = ui.ctx().cumulative_frame_nr();
    // 丢失释放事件时取消，绝不在回到窗口后接续旧手势。
    if !down && !released {
        cancel(view);
        return;
    }
    let Some(p) = point else { return };
    state.active |= p.distance(state.press) >= 5.0;
    if state.active {
        let end = clip.clamp(p) - origin.to_vec2();
        let selection = Rect::from_two_pos(state.start, end);
        let hits: BTreeSet<usize> = targets
            .into_iter()
            .filter_map(|(page, rect)| {
                let overlap = rect.translate(-origin.to_vec2()).intersect(selection);
                (overlap.width() > 0.0 && overlap.height() > 0.0).then_some(page)
            })
            .collect();
        view.selected = combine(&state.before, &hits, state.modifiers);
        // 保留原有范围锚点；新选择从框内第一页开始。
        view.select_anchor =
            if state.modifiers.shift || state.modifiers.command { state.anchor.or_else(|| hits.first().copied()) } else { hits.first().copied() };
        let painter = ui.painter().with_clip_rect(clip);
        let visible = selection.translate(origin.to_vec2()).intersect(clip);
        painter.rect_filled(visible, 0, t.accent.linear_multiply(0.16));
        painter.rect_stroke(visible, 0, Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        // 只在左右边界附近以内滚动；拖到别的面板不导出，也不无限滚动。
        if down && p.x >= clip.left() - 24.0 && p.x <= clip.right() + 24.0 {
            let edge = 36.0;
            let speed = if p.y < clip.top() + edge {
                ((clip.top() + edge - p.y) / edge).clamp(0.0, 1.0)
            } else if p.y > clip.bottom() - edge {
                -((p.y - clip.bottom() + edge) / edge).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let dt = ui.input(|i| i.stable_dt).clamp(0.0, 0.05);
            ui.scroll_with_delta_animation(vec2(0.0, speed * 600.0 * dt), egui::style::ScrollAnimation::none());
        }
        ui.ctx().request_repaint();
    }
    if released {
        view.page_marquee = None;
    }
}

fn combine(before: &BTreeSet<usize>, hits: &BTreeSet<usize>, modifiers: Modifiers) -> BTreeSet<usize> {
    if modifiers.command {
        before.symmetric_difference(hits).copied().collect()
    } else if modifiers.shift {
        before.union(hits).copied().collect()
    } else {
        hits.clone()
    }
}

/// 整理页面的卡片矩形。命中与绘制复用同一几何，虚拟行也能参与框选。
pub(crate) fn grid_card(grid: Rect, cell: Vec2, left: f32, cols: usize, page: usize) -> Rect {
    let cols = cols.max(1);
    Rect::from_min_size(pos2(grid.left() + left + (page % cols) as f32 * cell.x, grid.top() + (page / cols) as f32 * cell.y), cell).shrink(6.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn modifiers_use_the_initial_selection_not_previous_frame() {
        let before = [1, 4].into();
        let hits = [0, 1, 2].into();
        assert_eq!(combine(&before, &hits, Modifiers::NONE), hits);
        assert_eq!(combine(&before, &hits, Modifiers::SHIFT), [0, 1, 2, 4].into());
        assert_eq!(combine(&before, &hits, Modifiers::COMMAND), [0, 2, 4].into());
        assert_eq!(combine(&before, &BTreeSet::new(), Modifiers::COMMAND), before);
    }
}
