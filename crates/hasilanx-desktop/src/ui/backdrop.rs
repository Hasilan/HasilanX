use crate::theme::{BackdropStyle, UiPalette};
use gpui_kit::*;

/// GPU-painted atmospheric horizon & risograph halftone-sky backdrop inspired by Zeron.
/// Uses native GPU fragment-shader `linear_gradient` (zero banding) layered with an organic
/// multi-octave halftone cloud field that fades smoothly into the card surface.
pub fn render_hero_backdrop(style: BackdropStyle, p: UiPalette) -> AnyElement {
    if style == BackdropStyle::None {
        return div().into_any_element();
    }

    canvas(
        |bounds, _, _| bounds,
        move |bounds, _, window, _| {
            let w = f32::from(bounds.size.width);
            let h = f32::from(bounds.size.height);
            if w < 10.0 || h < 10.0 {
                return;
            }
            let x0 = f32::from(bounds.origin.x);
            let y0 = f32::from(bounds.origin.y);
            let sky_h = (h * 0.66).max(240.0).min(h);
            let transparent_base = {
                let mut c: Hsla = p.canvas.into();
                c.a = 0.0;
                c
            };

            // 1. Silky-smooth GPU fragment-shader vertical atmospheric gradient (zero striping)
            let (top_col, mid_col, mid_pos): (Hsla, Hsla, f32) = match (style, p.is_dark) {
                (BackdropStyle::Dusk, true) => {
                    let mut top: Hsla = rgb(0x1f306e).into();
                    top.a = 0.68;
                    let mut mid: Hsla = rgb(0x4b2654).into();
                    mid.a = 0.32;
                    (top, mid, 0.45)
                }
                (BackdropStyle::Dusk, false) => {
                    let mut top: Hsla = rgb(0x7c97ec).into();
                    top.a = 0.42;
                    let mut mid: Hsla = rgb(0xe4b8dc).into();
                    mid.a = 0.26;
                    (top, mid, 0.46)
                }
                (BackdropStyle::Aurora, true) => {
                    let mut top: Hsla = rgb(0x0d3840).into();
                    top.a = 0.62;
                    let mut mid: Hsla = rgb(0x163d35).into();
                    mid.a = 0.24;
                    (top, mid, 0.44)
                }
                (BackdropStyle::Aurora, false) => {
                    let mut top: Hsla = rgb(0x6ec7db).into();
                    top.a = 0.38;
                    let mut mid: Hsla = rgb(0xaee8d5).into();
                    mid.a = 0.22;
                    (top, mid, 0.45)
                }
                (BackdropStyle::Minimal, true) => {
                    let mut top: Hsla = p.accent.into();
                    top.a = 0.14;
                    let mut mid: Hsla = p.accent.into();
                    mid.a = 0.04;
                    (top, mid, 0.45)
                }
                (BackdropStyle::Minimal, false) => {
                    let mut top: Hsla = p.accent.into();
                    top.a = 0.10;
                    let mut mid: Hsla = p.accent.into();
                    mid.a = 0.03;
                    (top, mid, 0.45)
                }
                (BackdropStyle::None, _) => return,
            };

            let upper_h = (sky_h * mid_pos).round();
            let lower_h = (sky_h - upper_h).max(1.0);
            let upper_rect = Bounds::new(point(px(x0), px(y0)), size(px(w), px(upper_h)));
            let lower_rect =
                Bounds::new(point(px(x0), px(y0 + upper_h)), size(px(w), px(lower_h)));

            window.paint_quad(fill(
                upper_rect,
                linear_gradient(
                    180.0,
                    linear_color_stop(top_col, 0.0),
                    linear_color_stop(mid_col, 1.0),
                ),
            ));
            window.paint_quad(fill(
                lower_rect,
                linear_gradient(
                    180.0,
                    linear_color_stop(mid_col, 0.0),
                    linear_color_stop(transparent_base, 1.0),
                ),
            ));

            if style == BackdropStyle::Minimal {
                return;
            }

            // 2. Organic multi-octave risograph halftone cloud field (lightweight GPU quad budget)
            let step = 14.0f32;
            let dot_rows = ((sky_h * 0.90) / step).floor() as usize;
            let dot_cols = (w / step).floor() as usize;

            let (sky_dot, cloud_dot): (Hsla, Hsla) = match (style, p.is_dark) {
                (BackdropStyle::Dusk, true) => (rgb(0x7c97ec).into(), rgb(0xf2a6c8).into()),
                (BackdropStyle::Dusk, false) => (rgb(0x5265cf).into(), rgb(0xe88cb2).into()),
                (BackdropStyle::Aurora, true) => (rgb(0x38bdf8).into(), rgb(0x34d399).into()),
                (BackdropStyle::Aurora, false) => (rgb(0x0284c7).into(), rgb(0x0d9488).into()),
                _ => (p.accent.into(), p.accent.into()),
            };

            for r in 1..dot_rows {
                let y_frac = r as f32 / dot_rows.max(1) as f32;
                let top_soft = (y_frac * 5.0).min(1.0);
                let inv_y = 1.0 - y_frac;
                let vertical_fade = inv_y * inv_y * top_soft;
                if vertical_fade < 0.04 {
                    continue;
                }
                let py = y0 + r as f32 * step + step * 0.5;
                let row_offset = if r % 2 == 1 { step * 0.5 } else { 0.0 };

                for c in 0..dot_cols {
                    let px_pos = x0 + c as f32 * step + row_offset;
                    if px_pos > x0 + w - 4.0 {
                        continue;
                    }
                    let x_frac = c as f32 / dot_cols.max(1) as f32;

                    // Multi-octave organic cloud wave field framing left & right horizons
                    let edge_raw = (x_frac - 0.5).abs() * 2.0;
                    let edge_lift = edge_raw * (0.35 + 0.65 * edge_raw);
                    let w1 = (x_frac * 5.2 + y_frac * 2.6 + 0.4).sin() * 0.5 + 0.5;
                    let w2 = (x_frac * 11.4 - y_frac * 6.8 + 1.7).cos() * 0.5 + 0.5;
                    let cloud_mix = (w1 * 0.62 + w2 * 0.38) * (0.50 + 0.50 * edge_lift);

                    // Gently clear dots around the centered hero title for maximum legibility
                    let dx = (x_frac - 0.5) * 2.2;
                    let dy = (y_frac - 0.72) * 2.4;
                    let center_calm = (dx * dx + dy * dy).min(1.0).sqrt();

                    let density =
                        vertical_fade * (0.26 + 0.74 * cloud_mix) * (0.35 + 0.65 * center_calm);
                    if density < 0.06 {
                        continue;
                    }

                    let diameter = (1.2 + density * 2.8).clamp(1.2, 3.8);
                    let mut col = lerp_hsla(sky_dot, cloud_dot, cloud_mix.clamp(0.0, 1.0));
                    col.a = density * if p.is_dark { 0.34 } else { 0.30 };

                    let mut quad = fill(
                        Bounds::new(
                            point(px(px_pos - diameter * 0.5), px(py - diameter * 0.5)),
                            size(px(diameter), px(diameter)),
                        ),
                        col,
                    );
                    if diameter > 2.2 {
                        quad.corner_radii = px(diameter * 0.5).into();
                    }
                    window.paint_quad(quad);
                }
            }
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

fn lerp_hsla(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let t = t.clamp(0.0, 1.0);
    let mut dh = b.h - a.h;
    if dh > 0.5 {
        dh -= 1.0;
    } else if dh < -0.5 {
        dh += 1.0;
    }
    let mut h = a.h + dh * t;
    if h < 0.0 {
        h += 1.0;
    } else if h >= 1.0 {
        h -= 1.0;
    }
    Hsla {
        h,
        s: a.s + (b.s - a.s) * t,
        l: a.l + (b.l - a.l) * t,
        a: a.a + (b.a - a.a) * t,
    }
}
