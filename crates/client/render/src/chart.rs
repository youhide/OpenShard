//! A curve over time, as an SVG string.
//!
//! Every camera question in this repository is answered by looking at two
//! traces on one axis — a rig against another rig, a drawn body against the
//! oracle it is supposed to be — and a number in a table is what that looks
//! like *after* somebody has decided which number to take. So the picture is
//! part of the instrument rather than a nicety, and it lives here, beside
//! [`crate::bench`], so that the offline runner and the walk harness in
//! `client/app` draw with one drawer.
//!
//! # Strings, not files, and no plotting crate
//!
//! This builds a `String` and opens nothing: `client/render` reads no clocks and
//! writes no files, and the tests that dump these are the ones that know where
//! a dump belongs. Drawn by hand because it is a handful of `<polyline>`s and a
//! plotting dependency is for ever — the same argument the bench's own
//! SplitMix64 is here on.

/// One named curve: `(x, y)` in whatever units the panel is about, `x` being
/// seconds from the start of the run.
#[derive(Clone, Debug)]
pub struct Series {
    /// What to call it in the legend.
    pub name: String,
    /// The points, in order.
    pub points: Vec<(f64, f64)>,
}

/// One set of axes, and every curve drawn on it.
#[derive(Clone, Debug)]
pub struct Panel {
    /// What the vertical axis is, in words and units.
    pub title: String,
    /// The curves. Two or more on purpose: one curve on its own says nothing.
    pub series: Vec<Series>,
    /// A horizontal line to draw across the panel, if the quantity has an
    /// expected value — the oracle's constant speed, a lag of zero.
    ///
    /// [`Option`] in its proper sense: most quantities have no such value, and a
    /// default of `0.0` would draw a line claiming they did.
    pub baseline: Option<f64>,
}

/// The colours, in the order panels take them.
const COLOURS: [&str; 4] = ["#c0392b", "#2471a3", "#1e8449", "#8e44ad"];

const WIDTH: f64 = 900.0;
const PANEL_HEIGHT: f64 = 260.0;

/// Every panel stacked, titled, over `seconds` of run.
pub fn svg(title: &str, seconds: f64, panels: &[Panel]) -> String {
    let seconds = seconds.max(0.001);
    let mut out = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{WIDTH}\" height=\"{}\" \
         font-family=\"sans-serif\" font-size=\"12\">\n\
         <rect width=\"100%\" height=\"100%\" fill=\"#ffffff\"/>\n\
         <text x=\"12\" y=\"22\" font-size=\"15\">{title}</text>\n",
        PANEL_HEIGHT * panels.len() as f64 + (panels.len() as f64 * 20.0) + 40.0,
    );
    for (index, panel) in panels.iter().enumerate() {
        let top = 40.0 + index as f64 * (PANEL_HEIGHT + 20.0);
        out.push_str(&draw(panel, top, seconds));
    }
    out.push_str("</svg>\n");
    out
}

/// One panel, scaled to whatever it holds.
///
/// The vertical scale spans the data rather than starting at zero, because what
/// is being looked at is a departure from a flat line and a scale that included
/// the origin would flatten every one of them. A negative floor is kept for the
/// same reason: an eye that goes *backwards* is the whole subject in one of
/// these panels, and clamping it at zero would hide it.
fn draw(panel: &Panel, top: f64, seconds: f64) -> String {
    let left = 70.0;
    let plot = WIDTH - left - 20.0;
    let values = || {
        panel
            .series
            .iter()
            .flat_map(|series| series.points.iter().map(|(_, y)| *y))
            .chain(panel.baseline)
    };
    let low = values().fold(f64::INFINITY, f64::min).min(0.0);
    let high = values().fold(f64::NEG_INFINITY, f64::max);
    // A run where nothing varied is a flat line and not a division by zero.
    let span = (high - low).max(1e-6);
    let x = |t: f64| left + plot * (t / seconds);
    let y = |value: f64| top + PANEL_HEIGHT - PANEL_HEIGHT * ((value - low) / span);

    let mut out = format!(
        "<text x=\"{left}\" y=\"{}\">{}</text>\n\
         <line x1=\"{left}\" y1=\"{top}\" x2=\"{left}\" y2=\"{}\" stroke=\"#888\"/>\n\
         <line x1=\"{left}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"#888\"/>\n\
         <text x=\"8\" y=\"{}\" fill=\"#555\">{high:.1}</text>\n\
         <text x=\"8\" y=\"{}\" fill=\"#555\">{low:.1}</text>\n",
        top - 8.0,
        panel.title,
        top + PANEL_HEIGHT,
        top + PANEL_HEIGHT,
        left + plot,
        top + PANEL_HEIGHT,
        top + 10.0,
        top + PANEL_HEIGHT,
    );
    if let Some(baseline) = panel.baseline {
        out.push_str(&format!(
            "<line x1=\"{left}\" y1=\"{0:.1}\" x2=\"{1}\" y2=\"{0:.1}\" \
             stroke=\"#999\" stroke-dasharray=\"4 4\"/>\n",
            y(baseline),
            left + plot,
        ));
    }
    for (index, series) in panel.series.iter().enumerate() {
        let colour = COLOURS[index % COLOURS.len()];
        let path: String = series
            .points
            .iter()
            .map(|(t, value)| format!("{:.1},{:.1}", x(*t), y(*value)))
            .collect::<Vec<_>>()
            .join(" ");
        out.push_str(&format!(
            "<polyline fill=\"none\" stroke=\"{colour}\" stroke-width=\"1.2\" points=\"{path}\"/>\n\
             <text x=\"{}\" y=\"{}\" fill=\"{colour}\">{}</text>\n",
            left + plot - 90.0,
            top + 14.0 + index as f64 * 16.0,
            series.name,
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel() -> Panel {
        Panel {
            title: "speed".to_string(),
            series: vec![Series {
                name: "body".to_string(),
                points: vec![(0.0, 10.0), (1.0, 20.0), (2.0, -5.0)],
            }],
            baseline: Some(10.0),
        }
    }

    /// The one property a chart has to have to be worth looking at: every point
    /// lands inside the axes, including a negative one.
    ///
    /// The failure this guards is the one that makes a picture lie rather than
    /// break — a curve clipped at the floor reads as a quantity that stopped
    /// falling, which is exactly the reading these panels exist to give.
    #[test]
    fn every_point_lands_inside_the_panel() {
        let drawn = svg("run", 2.0, &[panel()]);
        let points = drawn
            .split("points=\"")
            .nth(1)
            .expect("a polyline")
            .split('"')
            .next()
            .expect("its points");
        for pair in points.split(' ') {
            let (x, y) = pair.split_once(',').expect("an x,y pair");
            let (x, y): (f64, f64) = (x.parse().unwrap(), y.parse().unwrap());
            assert!((0.0..=WIDTH).contains(&x), "{x} is off the page");
            assert!((40.0..=40.0 + PANEL_HEIGHT).contains(&y), "{y} is off the panel");
        }
    }

    /// A run where every sample is the same number is a flat line rather than a
    /// division by a span of nothing.
    #[test]
    fn a_constant_series_does_not_divide_by_zero() {
        let flat = Panel {
            title: "still".to_string(),
            series: vec![Series {
                name: "eye".to_string(),
                points: vec![(0.0, 7.0), (1.0, 7.0)],
            }],
            baseline: None,
        };
        let drawn = svg("still", 1.0, &[flat]);
        assert!(!drawn.contains("NaN"), "{drawn}");
    }
}
