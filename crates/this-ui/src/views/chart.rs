//! The drawing of [`Brick::Chart`](this_descriptor::views::Brick::Chart): inline SVG, no
//! library, no colour of its own.
//!
//! Every colour is a theme variable — `var(--accent)` for the series, `var(--line)` for the
//! axis, `var(--muted)` for the labels — so a chart follows the module's theme without code,
//! and never paints a fixed colour that would turn unreadable on the other theme.
//!
//! The drawing is a pure function of the values: it can be measured without a browser.
//! Scale: from `min(0, min)` to `max(0, max)`, so the baseline is the zero line and a
//! negative value hangs under it. One point per row; the SVG is as wide as its points need
//! ([`STEP`] px each), and the wrapper scrolls horizontally when that is wider than the
//! screen — a chart never squeezes its points into an unreadable comb.

use this_descriptor::views::{CHART_HEIGHT_DEFAULT, CHART_HEIGHT_MAX, CHART_HEIGHT_MIN};

/// Horizontal room of one point, in px.
pub const STEP: f64 = 24.0;
/// Width of one bar, in px.
const BAR: f64 = 16.0;
/// Side padding, in px.
const PAD: f64 = 6.0;
/// Room above the plot for the maximum label, in px.
const TOP: f64 = 14.0;
/// Room under the plot, in px.
const BOTTOM: f64 = 6.0;
/// A chart is never narrower than this, in px.
pub const MIN_WIDTH: f64 = 240.0;

/// What was drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartDraw {
    /// The `<svg>` element; empty when there is no point.
    pub svg: String,
    /// Points drawn: one `<rect>` (bar) or one `<circle>` (line) each.
    pub points: usize,
    /// Natural width of the drawing, in px.
    pub width: u32,
    /// Height of the drawing, in px (clamped into the contract's range).
    pub height: u32,
}

/// The height actually used: the declared one clamped into `80..=400`, else the default.
pub fn height_of(declared: Option<u32>) -> u32 {
    declared
        .unwrap_or(CHART_HEIGHT_DEFAULT)
        .clamp(CHART_HEIGHT_MIN, CHART_HEIGHT_MAX)
}

fn num(v: f64) -> String {
    let r = (v * 10.0).round() / 10.0;
    if r.fract() == 0.0 {
        format!("{}", r as i64)
    } else {
        format!("{r}")
    }
}

fn shown(v: f64) -> String {
    let r = (v * 100.0).round() / 100.0;
    if r.fract() == 0.0 {
        format!("{}", r as i64)
    } else {
        format!("{r}")
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('"', "&quot;")
}

/// Draw `values` as `kind` (`"bar"` or `"line"`; anything else draws as a line — the
/// contract refuses it before a render exists).
pub fn draw(kind: &str, values: &[f64], height: Option<u32>, title: Option<&str>) -> ChartDraw {
    let h = height_of(height);
    if values.is_empty() {
        return ChartDraw {
            svg: String::new(),
            points: 0,
            width: 0,
            height: h,
        };
    }
    let width = (values.len() as f64 * STEP + 2.0 * PAD).max(MIN_WIDTH);
    let hf = f64::from(h);
    let plot = hf - TOP - BOTTOM;
    let lo = values.iter().copied().fold(0.0_f64, f64::min);
    let mut hi = values.iter().copied().fold(0.0_f64, f64::max);
    if hi <= lo {
        hi = lo + 1.0;
    }
    let y = |v: f64| TOP + (hi - v) / (hi - lo) * plot;
    let zero = y(0.0);
    let x = |i: usize| PAD + STEP * (i as f64 + 0.5);

    let aria = title.map(escape).unwrap_or_default();
    let mut svg = format!(
        r#"<svg class="vw-chart-svg" viewBox="0 0 {w} {h}" width="{w}" height="{h}" role="img" aria-label="{aria}" data-chart-kind="{kind}" data-chart-points="{n}">"#,
        w = num(width),
        n = values.len(),
        kind = escape(kind),
    );
    svg.push_str(&format!(
        r#"<line x1="0" y1="{z}" x2="{w}" y2="{z}" style="stroke:var(--line);stroke-width:1"/>"#,
        z = num(zero),
        w = num(width),
    ));
    svg.push_str(&format!(
        r#"<text x="{p}" y="10" style="fill:var(--muted);font-size:10px">{t}</text>"#,
        p = num(PAD),
        t = shown(hi.max(0.0)),
    ));
    if kind == "bar" {
        for (i, v) in values.iter().enumerate() {
            let (top, bottom) = if *v >= 0.0 {
                (y(*v), zero)
            } else {
                (zero, y(*v))
            };
            svg.push_str(&format!(
                r#"<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="2" style="fill:var(--accent)"/>"#,
                x = num(x(i) - BAR / 2.0),
                y = num(top),
                w = num(BAR),
                h = num((bottom - top).max(1.0)),
            ));
        }
    } else {
        let d: Vec<String> = values
            .iter()
            .enumerate()
            .map(|(i, v)| {
                format!(
                    "{}{} {}",
                    if i == 0 { "M" } else { "L" },
                    num(x(i)),
                    num(y(*v))
                )
            })
            .collect();
        svg.push_str(&format!(
            r#"<path d="{}" style="fill:none;stroke:var(--accent);stroke-width:2;stroke-linejoin:round;stroke-linecap:round"/>"#,
            d.join(" ")
        ));
        for (i, v) in values.iter().enumerate() {
            svg.push_str(&format!(
                r#"<circle cx="{}" cy="{}" r="3" style="fill:var(--accent)"/>"#,
                num(x(i)),
                num(y(*v)),
            ));
        }
    }
    svg.push_str("</svg>");
    ChartDraw {
        svg,
        points: values.len(),
        width: width as u32,
        height: h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIVE: [f64; 5] = [3.0, 7.0, 2.0, 9.0, 5.0];

    #[test]
    fn five_bars_are_five_rects() {
        let d = draw("bar", &FIVE, None, Some("Ventes"));
        assert_eq!(d.points, 5);
        assert_eq!(d.svg.matches("<rect").count(), 5);
        assert!(!d.svg.contains("<path"));
        assert_eq!(d.height, 120);
    }

    #[test]
    fn a_line_of_five_points_is_one_path_and_five_circles() {
        let d = draw("line", &FIVE, Some(200), None);
        assert_eq!(d.points, 5);
        assert_eq!(d.svg.matches("<path").count(), 1);
        assert_eq!(d.svg.matches("<circle").count(), 5);
        assert_eq!(d.height, 200);
    }

    #[test]
    fn an_empty_series_draws_nothing() {
        for kind in ["bar", "line"] {
            let d = draw(kind, &[], None, None);
            assert_eq!(d.points, 0);
            assert!(d.svg.is_empty());
            assert!(!d.svg.contains("<path"));
        }
    }

    #[test]
    fn the_chart_follows_the_theme_and_paints_no_fixed_colour() {
        for kind in ["bar", "line"] {
            let svg = draw(kind, &FIVE, None, None).svg;
            for var in ["var(--accent)", "var(--line)", "var(--muted)"] {
                assert!(svg.contains(var), "{kind}: {var}");
            }
            assert!(!svg.contains('#'), "{kind}: a fixed colour");
            assert!(!svg.contains("rgb"), "{kind}: a fixed colour");
        }
    }

    #[test]
    fn the_height_is_clamped_and_the_width_grows_with_the_points() {
        assert_eq!(height_of(Some(10)), 80);
        assert_eq!(height_of(Some(9000)), 400);
        assert_eq!(height_of(None), 120);
        let many: Vec<f64> = (0..50).map(f64::from).collect();
        let d = draw("bar", &many, None, None);
        assert_eq!(d.width, 50 * 24 + 12);
        assert!(d.width as f64 > MIN_WIDTH);
    }

    #[test]
    fn negative_values_hang_under_the_zero_line() {
        let d = draw("bar", &[4.0, -4.0], Some(120), None);
        assert_eq!(d.points, 2);
        // Zero line halfway through the plot: (120-14-6)/2 + 14 = 64.
        assert!(d.svg.contains(r#"y1="64""#), "{}", d.svg);
    }

    #[test]
    fn a_flat_series_does_not_divide_by_zero() {
        let d = draw("line", &[0.0, 0.0, 0.0], None, None);
        assert_eq!(d.points, 3);
        assert!(!d.svg.contains("NaN") && !d.svg.contains("inf"));
    }
}
