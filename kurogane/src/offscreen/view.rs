//! Size, scale and place of an offscreen browser's view.
//!
//! The host gives its surface in pixels. CEF takes the view in whole DIPs and
//! makes each frame `ceil(dip * scale)` pixels. The view is the smallest
//! whole-DIP size whose frame covers the surface.

/// View an offscreen browser lays out and paints.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct View {
    /// Surface width in pixels, the last one with any area.
    width: u32,
    /// Surface height in pixels, the last one with any area.
    height: u32,
    /// Surface has no area, which hides the browser.
    empty: bool,
    /// Device scale factor.
    scale: f64,
    /// Surface's top-left corner on the screen in pixels.
    origin: (i32, i32),
    /// Visible as the host asked.
    visible: bool,
}

impl View {
    /// View of a `width` by `height` pixel surface at `scale`, visible at the
    /// screen's origin.
    pub(crate) fn new(width: u32, height: u32, scale: f64) -> Self {
        let mut view = Self {
            width: 1,
            height: 1,
            empty: false,
            scale,
            origin: (0, 0),
            visible: true,
        };
        view.set_surface(width, height);
        view
    }

    /// Sets the surface's size in pixels; one without area keeps the layout
    /// and hides the browser.
    pub(crate) fn set_surface(&mut self, width: u32, height: u32) {
        self.empty = width == 0 || height == 0;
        if !self.empty {
            self.width = width;
            self.height = height;
        }
    }

    /// Sets the device scale factor.
    pub(crate) fn set_scale(&mut self, scale: f64) {
        self.scale = scale;
    }

    /// Sets the surface's top-left corner on the screen in pixels.
    pub(crate) fn set_origin(&mut self, x: i32, y: i32) {
        self.origin = (x, y);
    }

    /// Sets whether the host shows the browser.
    pub(crate) fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    /// Device scale factor.
    pub(crate) fn scale(&self) -> f64 {
        self.scale
    }

    /// Returns whether the browser paints, visible with a surface of some area.
    pub(crate) fn shown(&self) -> bool {
        self.visible && !self.empty
    }

    /// Returns the view's size in whole DIPs.
    pub(crate) fn dip_size(&self) -> (i32, i32) {
        (
            dip_extent(self.width, self.scale),
            dip_extent(self.height, self.scale),
        )
    }

    /// Returns the screen point of the view point `(x, y)` in DIPs. Windows and
    /// Linux take screen pixels; macOS takes screen DIPs (cef_render_handler.h).
    pub(crate) fn screen_point(&self, x: i32, y: i32) -> (i32, i32) {
        let (left, top) = self.origin;
        if cfg!(target_os = "macos") {
            (
                (f64::from(left) / self.scale).round() as i32 + x,
                (f64::from(top) / self.scale).round() as i32 + y,
            )
        } else {
            (
                left + (f64::from(x) * self.scale).round() as i32,
                top + (f64::from(y) * self.scale).round() as i32,
            )
        }
    }
}

/// Returns the smallest whole number of DIPs, at least one, whose pixels at
/// `scale` cover `pixels`. Chromium scales in `f32`
/// (`gfx::ScaleToCeiledSize`).
fn dip_extent(pixels: u32, scale: f64) -> i32 {
    let scale = scale as f32;
    let target = pixels as f32;
    let covers = |dip: i32| (dip as f32 * scale).ceil() >= target;
    let mut dip = ((target / scale).floor() as i32).max(1);
    while !covers(dip) {
        dip += 1;
    }
    while dip > 1 && covers(dip - 1) {
        dip -= 1;
    }
    dip
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_scales_divide_exactly() {
        assert_eq!(dip_extent(1920, 1.0), 1920);
        assert_eq!(dip_extent(1600, 2.0), 800);
    }

    #[test]
    fn fractional_scales_take_the_smallest_covering_size() {
        // 667 * 1.5 = 1000.5, ceiled to 1001
        assert_eq!(dip_extent(1001, 1.5), 667);
        assert_eq!(dip_extent(1920, 1.25), 1536);
        assert_eq!(dip_extent(1001, 1.75), 572);
    }

    #[test]
    fn a_view_is_never_empty() {
        assert_eq!(dip_extent(0, 2.0), 1);
        assert_eq!(dip_extent(1, 3.0), 1);
    }

    #[test]
    fn an_empty_surface_hides_and_keeps_the_layout() {
        let mut view = View::new(800, 600, 1.0);
        view.set_surface(0, 600);
        assert!(!view.shown());
        assert_eq!(view.dip_size(), (800, 600));
        view.set_surface(1024, 768);
        assert!(view.shown());
        assert_eq!(view.dip_size(), (1024, 768));
    }
}
