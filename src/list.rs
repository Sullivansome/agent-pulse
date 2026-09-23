//! Toolkit-free viewport geometry shared by drawing and pointer input.
use island_design::agent_pulse as t;
use std::ops::Range;

#[derive(Default)]
pub struct Viewport {
    pub offset: f32,
    drag: Option<f32>,
}

#[derive(Debug, Clone, Copy)]
pub struct Thumb {
    pub y: f32,
    pub height: f32,
}

impl Viewport {
    pub fn height(h: u32) -> f32 {
        (h as f32 - t::HEADER - t::BOTTOM).max(0.0)
    }
    pub fn max_offset(count: usize, h: u32) -> f32 {
        (count as f32 * t::ROW - Self::height(h)).max(0.0)
    }
    pub fn offset(&self, count: usize, h: u32) -> f32 {
        if self.offset.is_finite() {
            self.offset.clamp(0.0, Self::max_offset(count, h))
        } else {
            0.0
        }
    }
    pub fn clamp(&mut self, count: usize, h: u32) {
        self.offset = self.offset(count, h);
    }
    pub fn reset(&mut self) {
        self.offset = 0.0;
        self.drag = None;
    }
    pub fn scroll(&mut self, dy: f32, count: usize, h: u32) {
        if dy.is_finite() {
            self.offset = (self.offset(count, h) + dy).clamp(0.0, Self::max_offset(count, h));
        }
    }
    pub fn reveal(&mut self, index: usize, count: usize, h: u32) {
        self.clamp(count, h);
        let top = index.min(count.saturating_sub(1)) as f32 * t::ROW;
        if top < self.offset {
            self.offset = top;
        } else if top + t::ROW > self.offset + Self::height(h) {
            self.offset = top + t::ROW - Self::height(h);
        }
        self.clamp(count, h);
    }
    pub fn rows(&self, count: usize, h: u32) -> Range<usize> {
        let offset = self.offset(count, h);
        let start = (offset / t::ROW).floor() as usize;
        let end = ((offset + Self::height(h)) / t::ROW).ceil() as usize;
        start.min(count)..end.min(count)
    }
    pub fn row_y(&self, index: usize, count: usize, h: u32) -> f32 {
        t::HEADER + index as f32 * t::ROW - self.offset(count, h)
    }
    pub fn contains_y(h: u32, y: f32) -> bool {
        y.is_finite() && y >= t::HEADER && y < h as f32 - t::BOTTOM
    }
    pub fn hit(&self, count: usize, h: u32, y: f32) -> Option<usize> {
        if !Self::contains_y(h, y) {
            return None;
        }
        let index = ((y - t::HEADER + self.offset(count, h)) / t::ROW).floor() as usize;
        (index < count).then_some(index)
    }
    pub fn thumb(&self, count: usize, h: u32) -> Option<Thumb> {
        let height = Self::height(h);
        let max = Self::max_offset(count, h);
        if max <= 0.0 || height <= 0.0 {
            return None;
        }
        let thumb = (height * height / (count as f32 * t::ROW))
            .max(t::SCROLLBAR_MIN_THUMB)
            .min(height);
        Some(Thumb {
            y: t::HEADER + (height - thumb) * self.offset(count, h) / max,
            height: thumb,
        })
    }
    pub fn press_scrollbar(&mut self, y: f32, count: usize, h: u32) {
        if let Some(thumb) = self.thumb(count, h) {
            self.drag = Some(if (thumb.y..thumb.y + thumb.height).contains(&y) {
                y - thumb.y
            } else {
                thumb.height / 2.0
            });
            self.drag_to(y, count, h);
        }
    }
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }
    pub fn end_drag(&mut self) {
        self.drag = None;
    }
    pub fn drag_to(&mut self, y: f32, count: usize, h: u32) {
        if !y.is_finite() {
            return;
        }
        if let (Some(grab), Some(thumb)) = (self.drag, self.thumb(count, h)) {
            let travel = Self::height(h) - thumb.height;
            if travel > 0.0 {
                self.offset =
                    ((y - t::HEADER - grab) / travel).clamp(0.0, 1.0) * Self::max_offset(count, h);
            }
        }
    }
}
