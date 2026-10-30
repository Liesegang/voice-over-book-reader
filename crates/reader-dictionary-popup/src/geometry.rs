/// Physical pixel geometry. A negative origin is normal on a secondary monitor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

pub fn fit_popup(mut popup: Bounds, work: Bounds) -> Bounds {
    let inset = 6_u32.min(work.width / 4).min(work.height / 4);
    popup.width = popup.width.min(work.width.saturating_sub(inset * 2)).max(1);
    popup.height = popup
        .height
        .min(work.height.saturating_sub(inset * 2))
        .max(1);
    let min_x = i64::from(work.x) + i64::from(inset);
    let min_y = i64::from(work.y) + i64::from(inset);
    let max_x =
        (i64::from(work.x) + i64::from(work.width) - i64::from(popup.width) - i64::from(inset))
            .max(min_x);
    let max_y =
        (i64::from(work.y) + i64::from(work.height) - i64::from(popup.height) - i64::from(inset))
            .max(min_y);
    popup.x = i64::from(popup.x)
        .clamp(min_x, max_x)
        .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    popup.y = i64::from(popup.y)
        .clamp(min_y, max_y)
        .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    popup
}

pub fn beside_word(anchor: (i32, i32), size: (u32, u32), work: Bounds) -> Bounds {
    let mut popup = Bounds {
        x: anchor.0.saturating_sub(18),
        y: anchor.1.saturating_add(20),
        width: size.0,
        height: size.1,
    };
    if i64::from(popup.y) + i64::from(popup.height) > i64::from(work.y) + i64::from(work.height) - 6
    {
        popup.y = anchor
            .1
            .saturating_sub(size.1.min(i32::MAX as u32) as i32)
            .saturating_sub(20);
    }
    fit_popup(popup, work)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clamps_to_work_area_on_negative_secondary_monitor() {
        let work = Bounds {
            x: -1920,
            y: -200,
            width: 1920,
            height: 1040,
        };
        let fitted = beside_word((-4, 780), (700, 500), work);
        assert_eq!(fitted.x, -706);
        assert_eq!(fitted.y, 260);
        assert!(fitted.x >= work.x && fitted.y >= work.y);
        assert!(fitted.x + fitted.width as i32 <= 0 && fitted.y + fitted.height as i32 <= 840);
    }
    #[test]
    fn oversized_saved_window_fits_available_pixels_without_panics() {
        let fitted = fit_popup(
            Bounds {
                x: i32::MAX,
                y: i32::MIN,
                width: 1600,
                height: 1000,
            },
            Bounds {
                x: 0,
                y: 0,
                width: 640,
                height: 480,
            },
        );
        assert_eq!(
            fitted,
            Bounds {
                x: 6,
                y: 6,
                width: 628,
                height: 468
            }
        );
        let tiny = fit_popup(
            fitted,
            Bounds {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
        );
        assert_eq!(
            tiny,
            Bounds {
                x: 0,
                y: 0,
                width: 1,
                height: 1
            }
        );
    }
}
