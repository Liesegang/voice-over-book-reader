//! One viewport for Chromium layout, page counting and native composition.
use eframe::egui::{Pos2, Rect, Vec2, pos2, vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GeometryError {
    InvalidScale,
    InvalidViewport,
    InvalidColumns,
    InvalidGap,
    NoPixelBudget,
    InvalidFrame,
    InvalidVisibleRect,
}

impl std::fmt::Display for GeometryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidScale => "Book display scale is outside the supported finite range",
            Self::InvalidViewport => "Book viewport must have positive finite dimensions",
            Self::InvalidColumns => "Book viewport supports one or two equal columns",
            Self::InvalidGap => "Book page gap must be finite and non-negative",
            Self::NoPixelBudget => "Book viewport has no room for its Chromium pixel grid",
            Self::InvalidFrame => "Book frame must have positive dimensions",
            Self::InvalidVisibleRect => "Book frame visible rectangle is outside its coded texture",
        })
    }
}

impl std::error::Error for GeometryError {}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ViewportGeometry {
    /// The integer DIP size answered by CEF GetViewRect and used by counters.
    pub css: [u32; 2],
    /// The complete physical surface required by that DIP size at this scale.
    pub physical: [u32; 2],
    pub gap_px: u32,
    pub columns: usize,
    scale: f32,
}

fn pixel_budget(points: f32, scale: f32) -> Result<u32, GeometryError> {
    let physical = f64::from(points) * f64::from(scale);
    if !points.is_finite() || points <= 0. || physical < 1. || physical > f64::from(i32::MAX) {
        return Err(GeometryError::InvalidViewport);
    }
    Ok(physical.floor() as u32)
}

fn fitting_axis(budget: u32, scale: f32) -> Result<(u32, u32), GeometryError> {
    let mut css = (f64::from(budget) / f64::from(scale))
        .floor()
        .min(f64::from(i32::MAX)) as u32;
    while css > 0 {
        let physical = (f64::from(css) * f64::from(scale)).ceil();
        if physical <= f64::from(budget) {
            let physical = physical as u32;
            // Match welding::ViewMetrics::logical exactly. Merely taking ceil
            // is insufficient at 125%: 513 DIP -> 642 px -> 514 DIP.
            if (physical as f32 / scale).round() as u32 == css {
                return Ok((css, physical));
            }
        }
        css -= 1;
    }
    Err(GeometryError::NoPixelBudget)
}

impl ViewportGeometry {
    pub fn new(
        available: Vec2,
        columns: usize,
        gap: f32,
        scale: f32,
    ) -> Result<Self, GeometryError> {
        // These are welding's supported scale bounds. Reject a value it would
        // silently clamp, so layout and actual CEF density cannot disagree.
        if !scale.is_finite() || !(0.25..=8.).contains(&scale) {
            return Err(GeometryError::InvalidScale);
        }
        if !(1..=2).contains(&columns) {
            return Err(GeometryError::InvalidColumns);
        }
        if !gap.is_finite() || gap < 0. {
            return Err(GeometryError::InvalidGap);
        }
        let width = pixel_budget(available.x, scale)?;
        let height = pixel_budget(available.y, scale)?;
        let gap = if columns == 1 { 0. } else { gap };
        let physical_gap = (f64::from(gap) * f64::from(scale)).round();
        if physical_gap >= f64::from(width) {
            return Err(GeometryError::NoPixelBudget);
        }
        let gap_px = physical_gap as u32;
        let per_pane = (width - gap_px) / columns as u32;
        let (css_width, physical_width) = fitting_axis(per_pane, scale)?;
        let (css_height, physical_height) = fitting_axis(height, scale)?;
        Ok(Self {
            css: [css_width, css_height],
            physical: [physical_width, physical_height],
            gap_px,
            columns,
            scale,
        })
    }

    pub fn extent(self) -> Vec2 {
        vec2(self.css[0] as f32, self.css[1] as f32)
    }

    /// The expected surface placement. Both prose panes retain the same DIP
    /// viewport, including when one physical pixel is left over outside them.
    pub fn pane_rect(self, available: Rect, slot: usize) -> Result<Rect, GeometryError> {
        self.paint_rect(available, slot, self.physical)
    }

    /// Place actual visible pixels at device-pixel coordinates without scaling.
    /// A pair meets at its common inner seam; any outer slack stays outside it.
    pub fn paint_rect(
        self,
        available: Rect,
        slot: usize,
        visible: [u32; 2],
    ) -> Result<Rect, GeometryError> {
        if slot >= self.columns {
            return Err(GeometryError::InvalidColumns);
        }
        if !available.is_finite() || available.width() <= 0. || available.height() <= 0. {
            return Err(GeometryError::InvalidViewport);
        }
        if visible.contains(&0) {
            return Err(GeometryError::InvalidFrame);
        }
        let scale = f64::from(self.scale);
        let left = (f64::from(available.left()) * scale).round();
        let right = (f64::from(available.right()) * scale).round();
        let top = (f64::from(available.top()) * scale).round();
        let bottom = (f64::from(available.bottom()) * scale).round();
        let width = f64::from(visible[0]);
        let height = f64::from(visible[1]);
        let x = if self.columns == 1 {
            ((left + right - width) / 2.).floor()
        } else {
            let pane = f64::from(self.physical[0]);
            let gap = f64::from(self.gap_px);
            let pair_left = ((left + right - (2. * pane + gap)) / 2.).floor();
            if slot == 0 {
                pair_left + pane - width
            } else {
                pair_left + pane + gap
            }
        };
        let y = ((top + bottom - height) / 2.).floor();
        Ok(Rect::from_min_max(
            Pos2::new((x / scale) as f32, (y / scale) as f32),
            Pos2::new(((x + width) / scale) as f32, ((y + height) / scale) as f32),
        ))
    }
}

/// CEF mouse events and DOM hit tests start at the visible view's top-left.
/// The coded texture's clean-aperture offset belongs only to texture sampling.
pub(super) fn view_point(paint_rect: Rect, position: Pos2) -> Vec2 {
    position - paint_rect.min
}

/// Crop CEF coded padding using its reported visible rectangle, retaining one
/// destination pixel for every visible texel. Invalid metadata is never sampled.
pub(super) fn crop_uv(coded: [u32; 2], visible: [i32; 4]) -> Result<Rect, GeometryError> {
    let [x, y, width, height] = visible;
    if coded.contains(&0) || x < 0 || y < 0 || width <= 0 || height <= 0 {
        return Err(GeometryError::InvalidVisibleRect);
    }
    let right = x
        .checked_add(width)
        .ok_or(GeometryError::InvalidVisibleRect)? as u32;
    let bottom = y
        .checked_add(height)
        .ok_or(GeometryError::InvalidVisibleRect)? as u32;
    if right > coded[0] || bottom > coded[1] {
        return Err(GeometryError::InvalidVisibleRect);
    }
    Ok(Rect::from_min_max(
        pos2(x as f32 / coded[0] as f32, y as f32 / coded[1] as f32),
        pos2(
            right as f32 / coded[0] as f32,
            bottom as f32 / coded[1] as f32,
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_pixel_aligned(rect: Rect, scale: f32) {
        for value in [rect.left(), rect.top(), rect.right(), rect.bottom()] {
            let pixels = value * scale;
            assert!((pixels - pixels.round()).abs() < 0.001, "{pixels}");
        }
    }

    #[test]
    fn odd_width_spreads_share_css_viewport_and_fit_each_display_scale() {
        for scale in [1., 1.25, 1.5, 2.] {
            let available = Rect::from_min_size(pos2(3.2, 2.4), vec2(1281., 761.));
            let geometry = ViewportGeometry::new(available.size(), 2, 0., scale).unwrap();
            assert!(geometry.physical[0] * 2 <= (available.width() * scale).floor() as u32);
            assert!(geometry.physical[1] <= (available.height() * scale).floor() as u32);
            assert_eq!(geometry.extent().x, geometry.css[0] as f32);
            for axis in 0..2 {
                assert_eq!(
                    (geometry.physical[axis] as f32 / scale).round() as u32,
                    geometry.css[axis]
                );
                assert_eq!(
                    (f64::from(geometry.css[axis]) * f64::from(scale)).ceil() as u32,
                    geometry.physical[axis]
                );
            }
            let left = geometry.pane_rect(available, 0).unwrap();
            let right = geometry.pane_rect(available, 1).unwrap();
            assert_pixel_aligned(left, scale);
            assert_pixel_aligned(right, scale);
            assert!((left.width() - right.width()).abs() < 0.001);
            assert!((left.right() - right.left()).abs() < 0.001);
            assert!((left.width() * scale - geometry.physical[0] as f32).abs() < 0.001);
        }
    }

    #[test]
    fn actual_visible_frames_keep_exact_texel_sizes_and_touch_the_inner_seam() {
        for scale in [1., 1.25, 1.5, 2.] {
            let available = Rect::from_min_size(Pos2::ZERO, vec2(1281., 761.));
            let geometry = ViewportGeometry::new(available.size(), 2, 0., scale).unwrap();
            let a = [geometry.physical[0] - 1, geometry.physical[1] - 1];
            let b = [geometry.physical[0] + 1, geometry.physical[1]];
            let left = geometry.paint_rect(available, 0, a).unwrap();
            let right = geometry.paint_rect(available, 1, b).unwrap();
            assert_pixel_aligned(left, scale);
            assert_pixel_aligned(right, scale);
            assert!((left.right() - right.left()).abs() < 0.001);
            for (rect, size) in [(left, a), (right, b)] {
                assert!((rect.width() * scale - size[0] as f32).abs() < 0.001);
                assert!((rect.height() * scale - size[1] as f32).abs() < 0.001);
            }
        }
    }

    #[test]
    fn single_actual_frame_is_centered_without_stretching() {
        let available = Rect::from_min_size(pos2(0.2, 0.4), vec2(1281., 761.));
        let geometry = ViewportGeometry::new(available.size(), 1, 0., 1.25).unwrap();
        let rect = geometry.paint_rect(available, 0, [1234, 567]).unwrap();
        assert_pixel_aligned(rect, 1.25);
        assert!((rect.width() * 1.25 - 1234.).abs() < 0.001);
        assert!((rect.height() * 1.25 - 567.).abs() < 0.001);
        assert!((rect.center().x - available.center().x).abs() <= 1. / 1.25);
    }

    #[test]
    fn configured_gap_and_roundtrip_at_fractional_density_are_preserved() {
        let geometry = ViewportGeometry::new(vec2(1028., 760.), 2, 0., 1.25).unwrap();
        assert_eq!(
            geometry.css[0], 512,
            "513 DIP would fit 642 px but return 514 from GetViewRect"
        );
        let available = Rect::from_min_size(Pos2::ZERO, vec2(1281., 761.));
        let geometry = ViewportGeometry::new(available.size(), 2, 17., 1.25).unwrap();
        let a = geometry.pane_rect(available, 0).unwrap();
        let b = geometry.pane_rect(available, 1).unwrap();
        assert!(((b.left() - a.right()) * 1.25 - geometry.gap_px as f32).abs() < 0.001);
    }

    #[test]
    fn invalid_geometry_returns_errors_instead_of_collapsed_rectangles() {
        for scale in [0., -1., f32::NAN, f32::INFINITY, 0.1, 9.] {
            assert_eq!(
                ViewportGeometry::new(vec2(1281., 761.), 2, 0., scale),
                Err(GeometryError::InvalidScale)
            );
        }
        assert!(ViewportGeometry::new(vec2(f32::NAN, 100.), 1, 0., 1.).is_err());
        assert!(ViewportGeometry::new(vec2(0., 100.), 1, 0., 1.).is_err());
        assert!(ViewportGeometry::new(vec2(100., 100.), 0, 0., 1.).is_err());
        assert!(ViewportGeometry::new(vec2(100., 100.), 2, -1., 1.).is_err());
        assert!(ViewportGeometry::new(vec2(1., 100.), 2, 0., 1.).is_err());
        assert!(ViewportGeometry::new(vec2(100., 100.), 2, 100., 1.).is_err());
        let geometry = ViewportGeometry::new(vec2(100., 100.), 2, 0., 1.).unwrap();
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(100., 100.));
        assert!(geometry.paint_rect(rect, 2, [50, 100]).is_err());
        assert!(geometry.paint_rect(rect, 0, [0, 100]).is_err());
    }

    #[test]
    fn crop_uv_excludes_coded_padding_and_rejects_invalid_ranges() {
        assert_eq!(
            crop_uv([640, 768], [0, 0, 640, 760]).unwrap(),
            Rect::from_min_max(pos2(0., 0.), pos2(1., 760. / 768.))
        );
        assert_eq!(
            crop_uv([640, 768], [4, 8, 632, 752]).unwrap(),
            Rect::from_min_max(pos2(4. / 640., 8. / 768.), pos2(636. / 640., 760. / 768.))
        );
        for visible in [
            [-1, 0, 640, 760],
            [0, -1, 640, 760],
            [0, 0, 0, 760],
            [0, 0, 640, -1],
            [1, 0, 640, 760],
            [0, 1, 640, 768],
            [i32::MAX, 0, 1, 1],
        ] {
            assert!(crop_uv([640, 768], visible).is_err());
        }
        assert!(crop_uv([0, 768], [0, 0, 1, 1]).is_err());
    }

    #[test]
    fn nonzero_texture_crop_does_not_offset_view_coordinates() {
        let available = Rect::from_min_size(pos2(10., 20.), vec2(1281., 761.));
        let scale = 1.25;
        let geometry = ViewportGeometry::new(available.size(), 1, 0., scale).unwrap();
        let visible = [8, 4, 640, 760];
        let uv = crop_uv([656, 768], visible).unwrap();
        let paint = geometry.paint_rect(available, 0, [640, 760]).unwrap();
        assert_eq!(uv.min, pos2(8. / 656., 4. / 768.));
        assert_eq!(view_point(paint, paint.min), Vec2::ZERO);
        let local = view_point(paint, paint.min + vec2(20. / scale, 10. / scale));
        assert!((local.x * scale - 20.).abs() < 0.001);
        assert!((local.y * scale - 10.).abs() < 0.001);
        assert_eq!(paint.min + local, paint.min + vec2(16., 8.));
    }
}
