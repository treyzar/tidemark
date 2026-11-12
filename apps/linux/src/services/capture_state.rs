//! Pure editor identity and letterboxed image-coordinate mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}
#[derive(Clone, Copy, Debug)]
pub struct Viewport {
    pub scale: f64,
    pub offset_x: f64,
    pub offset_y: f64,
    width: u32,
    height: u32,
}
impl Viewport {
    pub fn fit(widget_width: f64, widget_height: f64, width: u32, height: u32) -> Option<Self> {
        if !widget_width.is_finite()
            || !widget_height.is_finite()
            || widget_width <= 0.0
            || widget_height <= 0.0
            || width == 0
            || height == 0
        {
            return None;
        }
        let scale = (widget_width / f64::from(width)).min(widget_height / f64::from(height));
        Some(Self {
            scale,
            offset_x: (widget_width - f64::from(width) * scale) / 2.0,
            offset_y: (widget_height - f64::from(height) * scale) / 2.0,
            width,
            height,
        })
    }
    pub fn selection(self, start: (f64, f64), end: (f64, f64)) -> Option<Rect> {
        if ![start.0, start.1, end.0, end.1]
            .iter()
            .all(|v| v.is_finite())
        {
            return None;
        }
        let sx = (start.0 - self.offset_x) / self.scale;
        let sy = (start.1 - self.offset_y) / self.scale;
        if sx < 0.0
            || sy < 0.0
            || sx >= f64::from(self.width)
            || sy >= f64::from(self.height)
            || ((start.0 - end.0).abs() < 2.0 && (start.1 - end.1).abs() < 2.0)
        {
            return None;
        }
        let ex = ((end.0 - self.offset_x) / self.scale).clamp(0.0, f64::from(self.width));
        let ey = ((end.1 - self.offset_y) / self.scale).clamp(0.0, f64::from(self.height));
        let left = sx.min(ex).floor() as u32;
        let top = sy.min(ey).floor() as u32;
        let right = sx.max(ex).ceil().min(f64::from(self.width)) as u32;
        let bottom = sy.max(ey).ceil().min(f64::from(self.height)) as u32;
        (right > left && bottom > top).then_some(Rect {
            x: left,
            y: top,
            width: right - left,
            height: bottom - top,
        })
    }
    pub fn outline(self, r: Rect) -> (f64, f64, f64, f64) {
        (
            self.offset_x + f64::from(r.x) * self.scale,
            self.offset_y + f64::from(r.y) * self.scale,
            f64::from(r.width) * self.scale,
            f64::from(r.height) * self.scale,
        )
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Capture,
    Save,
    Crop,
    Pixelate,
    Ocr,
    Qr,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    generation: u64,
    sequence: u64,
}
pub struct Session {
    smoke: bool,
    closed: bool,
    generation: u64,
    sequence: u64,
    pending: Option<Request>,
}
impl Session {
    pub fn new(smoke: bool) -> Self {
        Self {
            smoke,
            closed: false,
            generation: 0,
            sequence: 0,
            pending: None,
        }
    }
    pub fn interactive(&self) -> bool {
        !self.smoke && !self.closed
    }
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }
    pub fn image_changed(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }
    pub fn close(&mut self) {
        self.closed = true;
        self.image_changed();
    }
    pub fn begin(&mut self, _operation: Operation) -> Option<Request> {
        if !self.interactive() || self.busy() {
            return None;
        }
        self.sequence = self.sequence.wrapping_add(1);
        let request = Request {
            generation: self.generation,
            sequence: self.sequence,
        };
        self.pending = Some(request);
        Some(request)
    }
    pub fn finish(&mut self, request: Request) -> bool {
        if self.pending != Some(request) {
            return false;
        }
        self.pending = None;
        !self.closed && self.generation == request.generation
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn letterbox_offsets_and_reverse_drag_map_to_image_pixels() {
        let v = Viewport::fit(600.0, 400.0, 400, 200).unwrap();
        assert_eq!(v.offset_y, 50.0);
        assert_eq!(
            v.selection((450.0, 275.0), (150.0, 125.0)),
            Some(Rect {
                x: 100,
                y: 50,
                width: 200,
                height: 100
            })
        );
        assert!(v.selection((20.0, 20.0), (200.0, 200.0)).is_none());
        assert_eq!(
            v.selection((150.0, 125.0), (900.0, 900.0)),
            Some(Rect {
                x: 100,
                y: 50,
                width: 300,
                height: 150
            })
        );
    }
    #[test]
    fn invalid_geometry_and_zero_drag_never_create_selections() {
        assert!(Viewport::fit(0.0, 100.0, 20, 20).is_none());
        assert!(Viewport::fit(f64::NAN, 100.0, 20, 20).is_none());
        let v = Viewport::fit(100.0, 100.0, 200, 100).unwrap();
        assert!(v.selection((20.0, 30.0), (20.0, 30.0)).is_none());
        assert!(v.selection((f64::NAN, 30.0), (20.0, 30.0)).is_none());
    }
    #[test]
    fn jobs_are_single_inflight_and_image_revision_rejects_old_results() {
        let mut state = Session::new(false);
        let request = state.begin(Operation::Capture).unwrap();
        assert!(state.begin(Operation::Ocr).is_none());
        state.image_changed();
        assert!(!state.finish(request));
        let fresh = state.begin(Operation::Ocr).unwrap();
        assert!(!state.finish(request));
        assert!(state.busy());
        assert!(state.finish(fresh));
    }
    #[test]
    fn smoke_disables_every_side_effect_operation() {
        let mut state = Session::new(true);
        for op in [
            Operation::Capture,
            Operation::Save,
            Operation::Crop,
            Operation::Pixelate,
            Operation::Ocr,
            Operation::Qr,
        ] {
            assert!(state.begin(op).is_none());
        }
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    #[test]
    fn closing_editor_invalidates_work_and_prevents_restart() {
        let mut state = Session::new(false);
        let request = state.begin(Operation::Capture).unwrap();
        state.close();
        assert!(!state.finish(request));
        assert!(!state.interactive());
        assert!(state.begin(Operation::Capture).is_none());
    }
    #[test]
    fn selection_stays_in_image_coordinates_after_resize() {
        let rect = Rect {
            x: 100,
            y: 50,
            width: 200,
            height: 100,
        };
        assert_eq!(
            Viewport::fit(600.0, 400.0, 400, 200).unwrap().outline(rect),
            (150.0, 125.0, 300.0, 150.0)
        );
        assert_eq!(
            Viewport::fit(300.0, 400.0, 400, 200).unwrap().outline(rect),
            (75.0, 162.5, 150.0, 75.0)
        );
    }
}
