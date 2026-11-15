//! The instruction-budget chart: instructions per cart size against the
//! limit, as inline SVG. The limit line follows each measured point's own
//! `limit` (flat up to 200 lines, then scaled with the line count), and the
//! labels are placed where they do not cross the data.

use promochaos_core::money::group_int;
use promochaos_core::results::BudgetPoint;
use promochaos_core::rules::limits::{INSTRUCTIONS, LINES_RATE};

const X0: f64 = 56.0;
const X1: f64 = 700.0;
const Y0: f64 = 260.0; // bottom
const Y1: f64 = 20.0; // top
const WIDTH: f64 = 760.0;
/// Rough advance of one character of 12px Geist, for keeping labels apart.
const CHAR_W: f64 = 6.7;

fn millions(n: f64) -> String {
    let m = n / 1_000_000.0;
    if (m - m.round()).abs() < 1e-9 {
        format!("{} M", m.round() as i64)
    } else {
        format!("{m:.1} M")
    }
}

fn lines_text(n: usize) -> String {
    if n == 1 {
        "1 line".to_string()
    } else {
        format!("{n} lines")
    }
}

/// The smallest of 1, 1.5, 2, 2.5, 3, 4, 5, 6 or 8 times a power of ten that
/// is at least `raw`.
fn nice_step(raw: f64) -> f64 {
    if !raw.is_finite() || raw <= 0.0 {
        return 1_000_000.0;
    }
    let mag = 10f64.powi(raw.log10().floor() as i32);
    for m in [1.0, 1.5, 2.0, 2.5, 3.0, 4.0, 5.0, 6.0, 8.0, 10.0] {
        if m * mag >= raw * (1.0 - 1e-9) {
            return m * mag;
        }
    }
    10.0 * mag
}

/// The line count above which Shopify scales the limits (200).
fn knee_lines() -> usize {
    (1.0 / LINES_RATE).round() as usize
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn sorted(points: &[BudgetPoint]) -> Vec<BudgetPoint> {
    let mut pts: Vec<BudgetPoint> = points.to_vec();
    pts.sort_by_key(|p| p.lines);
    pts.dedup_by_key(|p| p.lines);
    pts
}

/// The limit label: "Shopify limit: 11 M up to 200 lines, then scales", from
/// the measured limits.
fn limit_text(pts: &[BudgetPoint]) -> String {
    let base = pts.first().map_or(INSTRUCTIONS, |p| p.limit);
    if pts.iter().any(|p| p.limit > base) {
        format!(
            "Shopify limit: {} up to {} lines, then scales",
            millions(base as f64),
            knee_lines()
        )
    } else {
        format!("Shopify limit: {}", millions(base as f64))
    }
}

/// The chart's summary sentence, shown above it.
pub fn summary(points: &[BudgetPoint]) -> String {
    let pts = sorted(points);
    let first_over = pts.iter().find(|p| p.over());
    let growth = match (
        pts.iter().find(|p| p.lines == 100),
        pts.iter().find(|p| p.lines == 200),
    ) {
        (Some(a), Some(b)) if a.instructions > 0 => {
            let r = b.instructions as f64 / a.instructions as f64;
            // Doubling the lines: about 2x is linear, about 4x is quadratic.
            // A quadratic loop on top of linear parsing lands in between
            // (3.1x for the Rust example), which is not "in line".
            if r >= 3.5 {
                Some("The count grows with the square of the line count")
            } else if r >= 2.4 {
                Some(
                    "The count grows faster than the line count: part of the work grows with its square",
                )
            } else if r >= 1.7 {
                Some("The count grows in line with the line count")
            } else {
                Some("The count grows slowly with the line count")
            }
        }
        _ => None,
    };
    let mut s = "Instructions used per cart size.".to_string();
    if let Some(g) = growth {
        s.push(' ');
        s.push_str(g);
        s.push(if first_over.is_some() { ',' } else { '.' });
    }
    match first_over {
        Some(p) => {
            if growth.is_some() {
                s.push_str(&format!(
                    " and the first cart over the limit has {}.",
                    lines_text(p.lines)
                ));
            } else {
                s.push_str(&format!(
                    " The first cart over the limit has {}.",
                    lines_text(p.lines)
                ));
            }
        }
        None => match pts.last() {
            Some(last) => s.push_str(&format!(
                " Every cart size measured, up to {}, is within the limit.",
                lines_text(last.lines)
            )),
            None => s.push_str(" No cart size was measured."),
        },
    }
    s
}

/// The limit line's vertices in data units: (lines, limit). Flat from 0 to
/// the first point, then through every measured point, with the knee at
/// 200 lines added when two points straddle it.
fn limit_vertices(pts: &[BudgetPoint], x_max: f64) -> Vec<(f64, f64)> {
    let Some(first) = pts.first() else {
        let flat = INSTRUCTIONS as f64;
        return vec![(0.0, flat), (x_max, flat)];
    };
    let base = first.limit;
    let knee = knee_lines();
    let mut v = vec![(0.0, base as f64)];
    let mut prev: Option<&BudgetPoint> = None;
    for p in pts {
        if let Some(a) = prev
            && a.limit == base
            && p.limit > base
            && a.lines < knee
            && p.lines > knee
        {
            v.push((knee as f64, base as f64));
        }
        v.push((p.lines as f64, p.limit as f64));
        prev = Some(p);
    }
    v
}

#[derive(Debug, Clone, Copy)]
struct Rect {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl Rect {
    /// The box of one line of 12px text at `baseline`.
    fn text(x: f64, baseline: f64, chars: usize, anchor_end: bool) -> Rect {
        let w = chars as f64 * CHAR_W;
        let (x0, x1) = if anchor_end { (x - w, x) } else { (x, x + w) };
        Rect {
            x0,
            y0: baseline - 11.0,
            x1,
            y1: baseline + 3.0,
        }
    }

    fn union(self, o: Rect) -> Rect {
        Rect {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }

    fn inside_plot(&self) -> bool {
        self.x0 >= X0 + 2.0 && self.x1 <= WIDTH - 4.0 && self.y0 >= 2.0 && self.y1 <= Y0 - 2.0
    }

    fn near(&self, px: f64, py: f64, pad: f64) -> bool {
        px >= self.x0 - pad && px <= self.x1 + pad && py >= self.y0 - pad && py <= self.y1 + pad
    }

    fn overlaps(&self, o: &Rect) -> bool {
        self.x0 < o.x1 && o.x0 < self.x1 && self.y0 < o.y1 && o.y0 < self.y1
    }

    /// Whether any part of the polyline comes within `pad` of the box.
    fn hits(&self, line: &[(f64, f64)], pad: f64) -> bool {
        if line.len() == 1 {
            return self.near(line[0].0, line[0].1, pad);
        }
        line.windows(2).any(|w| {
            let (a, b) = (w[0], w[1]);
            let n = ((b.0 - a.0).hypot(b.1 - a.1) / 2.0).ceil().max(1.0) as usize;
            (0..=n).any(|i| {
                let t = i as f64 / n as f64;
                self.near(a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t, pad)
            })
        })
    }
}

/// Everything a label must keep clear of.
struct Obstacles<'a> {
    data: &'a [(f64, f64)],
    limit: &'a [(f64, f64)],
    boxes: Vec<Rect>,
}

impl Obstacles<'_> {
    fn clear(&self, r: &Rect, data_pad: f64) -> bool {
        r.inside_plot()
            && !r.hits(self.data, data_pad)
            && !r.hits(self.limit, 2.0)
            && !self.boxes.iter().any(|b| b.overlaps(r))
    }
}

pub fn svg(points: &[BudgetPoint]) -> String {
    let pts = sorted(points);
    let max_lines = pts.iter().map(|p| p.lines).max().unwrap_or(250).max(1);
    let x_max = if max_lines <= 250 {
        250.0
    } else {
        ((max_lines as f64) / 50.0).ceil() * 50.0
    };
    let top_value = pts
        .iter()
        .map(|p| p.instructions.max(p.limit) as f64)
        .fold(INSTRUCTIONS as f64, f64::max);
    let step = nice_step(top_value * 1.05 / 4.0);
    let y_max = step * 4.0;
    let x = |lines: f64| X0 + (X1 - X0) * (lines / x_max);
    let y = |v: f64| Y0 - (Y0 - Y1) * (v / y_max).min(1.05);

    let mut s = String::new();
    let aria = {
        let parts: Vec<String> = pts
            .iter()
            .map(|p| {
                format!(
                    "{} {} instructions against a limit of {}{}",
                    lines_text(p.lines),
                    group_int(p.instructions),
                    group_int(p.limit),
                    if p.over() { ", over" } else { "" }
                )
            })
            .collect();
        format!(
            "Instructions per cart size. {}. {}.",
            limit_text(&pts),
            parts.join("; ")
        )
    };
    s.push_str(&format!(
        "<svg viewBox=\"0 0 760 300\" width=\"760\" height=\"300\" role=\"img\" aria-label=\"{}\" class=\"chart\">\n",
        esc(&aria)
    ));
    s.push_str(
        "<g style=\"stroke:var(--rule,#e3e6eb)\" stroke-width=\"1\" aria-hidden=\"true\">\n",
    );
    for i in 0..=4 {
        let yy = y(step * i as f64);
        s.push_str(&format!(
            "<line x1=\"{X0}\" y1=\"{yy:.1}\" x2=\"{X1}\" y2=\"{yy:.1}\"></line>\n"
        ));
    }
    s.push_str(
        "</g>\n<g style=\"fill:var(--muted,#4a5565)\" font-size=\"12\" text-anchor=\"end\" aria-hidden=\"true\">\n",
    );
    for i in 0..=4 {
        let v = step * i as f64;
        let label = if i == 0 { "0".to_string() } else { millions(v) };
        s.push_str(&format!(
            "<text x=\"46\" y=\"{:.1}\">{label}</text>\n",
            y(v) + 4.0
        ));
    }
    s.push_str(
        "</g>\n<g style=\"fill:var(--muted,#4a5565)\" font-size=\"12\" text-anchor=\"middle\" aria-hidden=\"true\">\n",
    );
    let x_step = if x_max <= 250.0 {
        50.0
    } else {
        (x_max / 5.0 / 50.0).ceil() * 50.0
    };
    let mut t = 0.0;
    while t <= x_max + 0.5 {
        let label = if (t - x_max).abs() < 0.5 {
            format!("{} lines", t as i64)
        } else {
            format!("{}", t as i64)
        };
        s.push_str(&format!(
            "<text x=\"{:.1}\" y=\"282\">{label}</text>\n",
            x(t)
        ));
        t += x_step;
    }
    s.push_str("</g>\n");

    // Screen coordinates of the limit line and the data line.
    let limit_line: Vec<(f64, f64)> = limit_vertices(&pts, x_max)
        .into_iter()
        .map(|(l, v)| (x(l), y(v)))
        .collect();
    let data_line: Vec<(f64, f64)> = pts
        .iter()
        .map(|p| (x(p.lines as f64), y(p.instructions as f64)))
        .collect();
    let mut obstacles = Obstacles {
        data: &data_line,
        limit: &limit_line,
        boxes: Vec::new(),
    };

    let fmt_points = |v: &[(f64, f64)]| {
        v.iter()
            .map(|(a, b)| format!("{a:.1},{b:.1}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    s.push_str(&format!(
        "<polyline points=\"{}\" fill=\"none\" style=\"stroke:var(--muted,#4a5565)\" stroke-width=\"1.5\" stroke-dasharray=\"5 4\" aria-hidden=\"true\"></polyline>\n",
        fmt_points(&limit_line)
    ));

    // The limit label: above the flat part on the left, as on the board, or
    // wherever else it does not cross the data.
    let label = limit_text(&pts);
    let (left_y, right_y) = (
        limit_line.first().map_or(Y0, |p| p.1),
        limit_line.last().map_or(Y0, |p| p.1),
    );
    let candidates = [
        (X0 + 8.0, left_y - 8.0, false),
        (X1, right_y - 8.0, true),
        (X0 + 8.0, left_y + 16.0, false),
        (X1, right_y + 16.0, true),
    ];
    let chosen = candidates
        .iter()
        .copied()
        .find(|&(lx, ly, end)| {
            obstacles.clear(&Rect::text(lx, ly, label.chars().count(), end), 6.0)
        })
        .unwrap_or(candidates[0]);
    let (lx, ly, end) = chosen;
    obstacles
        .boxes
        .push(Rect::text(lx, ly, label.chars().count(), end));
    s.push_str(&format!(
        "<text x=\"{lx:.1}\" y=\"{ly:.1}\" style=\"fill:var(--muted,#4a5565)\" font-size=\"12\"{} aria-hidden=\"true\">{}</text>\n",
        if end { " text-anchor=\"end\"" } else { "" },
        esc(&label)
    ));

    if !pts.is_empty() {
        s.push_str(&format!(
            "<polyline points=\"{}\" fill=\"none\" style=\"stroke:var(--blue,#0065FD)\" stroke-width=\"2\" stroke-linejoin=\"round\" aria-hidden=\"true\"></polyline>\n",
            fmt_points(&data_line)
        ));
        s.push_str("<g style=\"stroke:var(--ground,#ffffff)\" stroke-width=\"2\">\n");
        for p in &pts {
            let (fill, r) = if p.over() {
                ("var(--fail,#b42318)", 5.0)
            } else {
                ("var(--blue,#0065FD)", 4.5)
            };
            s.push_str(&format!(
                "<circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"{r}\" style=\"fill:{fill}\"><title>{}: {} instructions, limit {}</title></circle>\n",
                x(p.lines as f64),
                y(p.instructions as f64),
                lines_text(p.lines),
                group_int(p.instructions),
                group_int(p.limit)
            ));
        }
        s.push_str("</g>\n");

        // "First over" callout, only when a measured cart is over its limit.
        let first_over = pts.iter().find(|p| p.over());
        if let Some(p) = first_over {
            let (px, py) = (x(p.lines as f64), y(p.instructions as f64));
            let head = lines_text(p.lines);
            let body = format!("{}, first over", group_int(p.instructions));
            let w = head.chars().count().max(body.chars().count()) as f64 * CHAR_W;
            // (dx, dy, text to the left of the leader end)
            let spots = [
                (-44.0, -38.0, true),
                (44.0, 38.0, false),
                (44.0, -38.0, false),
                (-44.0, 38.0, true),
            ];
            let place = |(dx, dy, left): (f64, f64, bool)| {
                let tx = if left {
                    (px + dx).max(X0 + 8.0 + w)
                } else {
                    (px + dx).min(WIDTH - 8.0 - w)
                };
                let ty = (py + dy).clamp(Y1 + 30.0, Y0 - 8.0);
                let text_x = if left { tx - 4.0 } else { tx + 4.0 };
                let r = Rect::text(text_x, ty - 14.0, head.chars().count(), left)
                    .union(Rect::text(text_x, ty, body.chars().count(), left));
                (tx, ty, text_x, left, r)
            };
            let chosen = spots
                .iter()
                .map(|&sp| place(sp))
                .find(|c| obstacles.clear(&c.4, 6.0))
                .unwrap_or_else(|| place(spots[0]));
            let (tx, ty, text_x, left, r) = chosen;
            obstacles.boxes.push(r);
            let anchor = if left { "end" } else { "start" };
            s.push_str(&format!(
                "<line x1=\"{px:.1}\" y1=\"{py:.1}\" x2=\"{tx:.1}\" y2=\"{ty:.1}\" style=\"stroke:var(--ink,#0b1220)\" stroke-width=\"1\" aria-hidden=\"true\"></line>\n"
            ));
            s.push_str(&format!(
                "<text x=\"{text_x:.1}\" y=\"{:.1}\" style=\"fill:var(--ink,#0b1220)\" font-size=\"12\" text-anchor=\"{anchor}\" font-weight=\"600\" aria-hidden=\"true\">{head}</text>\n",
                ty - 14.0
            ));
            s.push_str(&format!(
                "<text x=\"{text_x:.1}\" y=\"{ty:.1}\" style=\"fill:var(--muted,#4a5565)\" font-size=\"12\" text-anchor=\"{anchor}\" aria-hidden=\"true\">{body}</text>\n"
            ));
        }

        // The largest cart's count, unless the callout already names it.
        if let Some(last) = pts.last()
            && first_over.map(|p| p.lines) != Some(last.lines)
        {
            let (px, py) = (x(last.lines as f64), y(last.instructions as f64));
            let text = format!("{} at {}", millions(last.instructions as f64), last.lines);
            let n = text.chars().count();
            let spots = [(px - 8.0, py - 6.0), (px, py - 12.0), (px - 8.0, py + 16.0)];
            let (ex, ey) = spots
                .iter()
                .copied()
                .map(|(a, b)| (a, b.clamp(Y1 + 12.0, Y0 - 6.0)))
                .find(|&(a, b)| obstacles.clear(&Rect::text(a, b, n, true), 4.0))
                .unwrap_or((px - 8.0, (py - 6.0).clamp(Y1 + 12.0, Y0 - 6.0)));
            s.push_str(&format!(
                "<text x=\"{ex:.1}\" y=\"{ey:.1}\" style=\"fill:var(--ink,#0b1220)\" font-size=\"12\" text-anchor=\"end\" aria-hidden=\"true\">{text}</text>\n"
            ));
        }
    }
    s.push_str("</svg>");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use promochaos_core::rules::limits::{scale_for_lines, scaled};

    fn p(lines: usize, instructions: u64) -> BudgetPoint {
        BudgetPoint {
            lines,
            instructions,
            limit: scaled(INSTRUCTIONS, scale_for_lines(lines)),
        }
    }

    /// The `points` attribute of the dashed limit polyline, parsed.
    fn limit_polyline(svg: &str) -> Vec<(f64, f64)> {
        let at = svg.find("stroke-dasharray").expect("limit line");
        let start = svg[..at].rfind("points=\"").expect("points") + 8;
        let end = start + svg[start..].find('"').expect("end quote");
        svg[start..end]
            .split(' ')
            .map(|xy| {
                let (a, b) = xy.split_once(',').expect("x,y");
                (a.parse().expect("x"), b.parse().expect("y"))
            })
            .collect()
    }

    #[test]
    fn summary_names_the_first_cart_over() {
        let pts = vec![
            p(1, 27_000),
            p(100, 3_500_000),
            p(178, 11_021_348),
            p(200, 14_000_000),
        ];
        assert_eq!(
            summary(&pts),
            "Instructions used per cart size. The count grows with the square of the line count, and the first cart over the limit has 178 lines."
        );
    }

    #[test]
    fn summary_says_so_when_no_cart_reaches_the_limit() {
        let pts = [
            p(1, 68_869),
            p(100, 1_989_238),
            p(200, 3_905_737),
            p(250, 4_868_157),
        ];
        assert_eq!(
            summary(&pts),
            "Instructions used per cart size. The count grows in line with the line count. Every cart size measured, up to 250 lines, is within the limit."
        );
        let s = svg(&pts);
        assert!(!s.contains("first over"), "no callout when nothing is over");
        assert!(
            !s.contains("#b42318"),
            "no failure colour when nothing is over"
        );
    }

    #[test]
    fn limit_line_follows_the_measured_limits() {
        let pts = [
            p(1, 68_869),
            p(100, 1_989_238),
            p(200, 3_905_737),
            p(201, 3_926_739),
            p(250, 4_868_157),
        ];
        let s = svg(&pts);
        let line = limit_polyline(&s);
        // From 0 lines, then one vertex per measured point.
        assert_eq!(line.len(), pts.len() + 1);
        // Flat up to 200 lines...
        let flat_y = line[0].1;
        for v in &line[..=3] {
            assert!((v.1 - flat_y).abs() < 0.05, "flat to 200 lines: {line:?}");
        }
        // ...then rising: 11,055,000 at 201 and 13,750,000 at 250 lines.
        assert!(line[4].1 < flat_y && line[5].1 < line[4].1, "{line:?}");
        let per_m = (line[0].1 - line[5].1) / 2.75; // px per million
        assert!(((flat_y - line[4].1) / per_m - 0.055).abs() < 0.02);
        assert!(s.contains("Shopify limit: 11 M up to 200 lines, then scales"));
    }

    #[test]
    fn limit_line_adds_the_knee_between_distant_points() {
        let s = svg(&[p(1, 1_000), p(150, 2_000_000), p(250, 3_000_000)]);
        let line = limit_polyline(&s);
        assert_eq!(line.len(), 5, "0, 1, 150, knee at 200, 250: {line:?}");
        assert!((line[3].1 - line[0].1).abs() < 0.05);
        assert!(line[4].1 < line[3].1);
    }

    #[test]
    fn limit_label_says_flat_when_no_point_is_scaled() {
        let s = svg(&[p(1, 1_000), p(150, 2_000_000)]);
        assert!(s.contains(">Shopify limit: 11 M</text>"));
    }

    #[test]
    fn labels_keep_clear_of_a_steep_curve() {
        // The tiered example: the 11 M line sits near the bottom of a
        // 160 M axis and the data crosses the left of it.
        let pts = [
            p(1, 69_473),
            p(10, 438_696),
            p(50, 6_045_004),
            p(69, 11_005_013),
            p(75, 12_876_444),
            p(100, 22_279_068),
            p(200, 86_994_613),
            p(250, 135_289_769),
        ];
        let s = svg(&pts);
        assert!(s.contains(">160 M</text>"), "tight axis");
        assert!(s.contains(">69 lines</text>"));
        assert!(s.contains("11,005,013, first over"));
        // The limit label moved off the left, where the curve crosses it.
        let label = s
            .lines()
            .find(|l| l.starts_with("<text") && l.contains("Shopify limit"))
            .expect("label");
        assert!(label.contains("text-anchor=\"end\""), "{label}");
    }

    #[test]
    fn svg_is_well_formed_enough() {
        let s = svg(&[p(1, 27_000), p(250, 21_000_000)]);
        assert!(s.starts_with("<svg"));
        assert!(s.ends_with("</svg>"));
        assert!(s.contains("#b42318"));
        assert_eq!(s.matches("<circle").count(), 2);
        assert!(svg(&[]).contains("Shopify limit"));
        assert!(!s.contains("http"), "no external references");
    }

    #[test]
    fn axis_steps_are_round() {
        assert_eq!(nice_step(5.7e6), 6e6);
        assert_eq!(nice_step(35.5e6), 40e6);
        assert_eq!(nice_step(3.6e6), 4e6);
        assert_eq!(nice_step(2.0e6), 2e6);
    }

    #[test]
    fn millions_format() {
        assert_eq!(millions(6_000_000.0), "6 M");
        assert_eq!(millions(21_714_500.0), "21.7 M");
    }

    /// Regression (QA 2026-11-15): the tuned Rust example (4.3 M at 100
    /// lines, 13.5 M at 200) was described as growing "in line with the
    /// line count".
    #[test]
    fn mixed_linear_and_quadratic_growth_is_not_called_linear() {
        let p = |lines: usize, instructions: u64| BudgetPoint {
            lines,
            instructions,
            limit: 11_000_000,
        };
        let s = summary(&[p(100, 4_319_968), p(200, 13_516_413)]);
        assert!(s.contains("faster than the line count"), "{s}");
        let s = summary(&[p(100, 1_989_238), p(200, 3_905_737)]);
        assert!(s.contains("in line with the line count"), "{s}");
    }
}
