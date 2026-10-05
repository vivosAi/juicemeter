//! The menu bar juice box, drawn as a template image (macOS tints it for light/dark bars).
//! The juice level is the meter.

/// Pixel size; macOS scales tray icons to 18pt tall, so this is 2x.
const W: i32 = 28;
const H: i32 = 36;
const STROKE: i32 = 3;

struct Canvas(Vec<u8>);

impl Canvas {
    fn set(&mut self, x: i32, y: i32) {
        self.blend(x, y, 1.0);
    }

    /// Partial ink, for smooth edges; keeps the stronger of what's there and `coverage`.
    fn blend(&mut self, x: i32, y: i32, coverage: f64) {
        if (0..W).contains(&x) && (0..H).contains(&y) {
            let i = ((y * W + x) * 4) as usize;
            let a = (coverage.clamp(0.0, 1.0) * 255.0).round() as u8;
            self.0[i + 3] = self.0[i + 3].max(a);
        }
    }

    fn rect(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        for y in y0..y1 {
            for x in x0..x1 {
                self.set(x, y);
            }
        }
    }

    /// A thick line, by stamping squares along it.
    fn line(&mut self, (x0, y0): (f64, f64), (x1, y1): (f64, f64)) {
        let steps = ((x1 - x0).abs().max((y1 - y0).abs()) * 2.0).ceil() as i32;
        for i in 0..=steps {
            let t = i as f64 / steps as f64;
            let (x, y) = (x0 + (x1 - x0) * t, y0 + (y1 - y0) * t);
            self.rect(x as i32 - 1, y as i32 - 1, x as i32 + 2, y as i32 + 2);
        }
    }
}

/// RGBA pixels of the juice box filled to `fill` (0..1).
pub fn juice_box(fill: f64) -> (Vec<u8>, u32, u32) {
    let mut c = Canvas(vec![0; (W * H * 4) as usize]);
    // Box outline.
    let (x0, y0, x1, y1) = (2, 10, 26, 36);
    c.rect(x0, y0, x1, y0 + STROKE);
    c.rect(x0, y1 - STROKE, x1, y1);
    c.rect(x0, y0, x0 + STROKE, y1);
    c.rect(x1 - STROKE, y0, x1, y1);
    // Straw: up and to the right out of the lid, then a short bend.
    c.line((18.0, 10.0), (21.0, 2.0));
    c.line((21.0, 2.0), (26.0, 2.0));
    // Juice, inset 2px from the stroke, rising from the bottom, with one gentle wave on
    // top like the logo. Edge pixels get partial ink so the curve reads at menu bar size.
    // Always a sliver, so an empty box still reads as "empty" rather than as broken.
    let (fx0, fx1, fy0, fy1) = (x0 + STROKE + 2, x1 - STROKE - 2, y0 + STROKE + 2, y1 - STROKE - 2);
    let height = (fy1 - fy0) as f64;
    let level = (height * fill.clamp(0.0, 1.0)).max(1.5);
    // Flatten the wave near empty and full so it never pokes through the bottom or lid.
    let amp = 1.3_f64.min(level / 3.0).min((height - level) / 2.0 + 0.4);
    let width = (fx1 - fx0) as f64;
    for x in fx0..fx1 {
        let phase = (x - fx0) as f64 + 0.5;
        let surface = fy1 as f64 - level + amp * (std::f64::consts::TAU * phase / width).sin();
        for y in fy0..fy1 {
            c.blend(x, y, y as f64 + 1.0 - surface);
        }
    }
    (c.0, W as u32, H as u32)
}

#[cfg(test)]
mod tests {
    #[test]
    fn fuller_box_has_more_ink() {
        let ink = |f| super::juice_box(f).0.chunks(4).map(|p| p[3] as u32).sum::<u32>();
        assert!(ink(1.0) > ink(0.5) && ink(0.5) > ink(0.0));
    }

    #[test]
    fn juice_surface_is_a_wave_not_a_line() {
        // At half full, the top edge sits at different heights across the box.
        let (px, w, _) = super::juice_box(0.5);
        let top = |x: u32| (0..36).find(|&y| (14..30).contains(&y) && px[((y * w + x) * 4 + 3) as usize] > 128);
        assert_ne!(top(9), top(18));
    }
}
