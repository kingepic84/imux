//! iTerm2 inline image protocol: `OSC 1337 ; File=args : base64 ST`. Parses the arguments and
//! works out how many cells the image covers.

use super::base64;

/// A size argument: `N` cells, `Npx` pixels, `N%` of the pane, or `auto`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dim {
    Auto,
    Cells(usize),
    Px(usize),
    Percent(usize),
}

impl Dim {
    /// Numbers are capped so that later arithmetic on them cannot overflow.
    fn parse(s: &str) -> Dim {
        let num = |n: &str| n.parse::<usize>().ok().map(|v| v.min(1 << 20));
        if let Some(n) = s.strip_suffix("px") {
            num(n).map_or(Dim::Auto, Dim::Px)
        } else if let Some(n) = s.strip_suffix('%') {
            num(n).map_or(Dim::Auto, Dim::Percent)
        } else {
            num(s).map_or(Dim::Auto, Dim::Cells)
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct File {
    /// `inline=1`: show it; otherwise the sender wants a download, which imux ignores.
    pub inline: bool,
    pub width: Dim,
    pub height: Dim,
    pub preserve_aspect: bool,
    /// The file as sent (PNG, JPEG, GIF), already base64-decoded.
    pub data: Vec<u8>,
}

/// `payload` is what follows `File=`: `key=value;...:base64`.
pub fn parse(payload: &[u8]) -> Result<File, String> {
    let colon = payload.iter().position(|&b| b == b':').ok_or("no image data")?;
    let args = std::str::from_utf8(&payload[..colon]).map_err(|_| "arguments are not text")?;
    let mut file = File { inline: false, width: Dim::Auto, height: Dim::Auto, preserve_aspect: true, data: Vec::new() };
    for arg in args.split(';') {
        let Some((key, value)) = arg.split_once('=') else { continue };
        match key {
            "inline" => file.inline = value == "1",
            "width" => file.width = Dim::parse(value),
            "height" => file.height = Dim::parse(value),
            "preserveAspectRatio" => file.preserve_aspect = value != "0",
            _ => {}
        }
    }
    file.data = base64::decode(&payload[colon + 1..]).ok_or("image data is not base64")?;
    Ok(file)
}

/// What the image is placed in: the cell size in pixels and the pane size in cells.
#[derive(Clone, Copy, Debug)]
pub struct Metrics {
    pub cell_w: usize,
    pub cell_h: usize,
    pub cols: usize,
    pub rows: usize,
}

/// Rows an image may cover at most; keeps a hostile size argument from allocating the world.
const MAX_ROWS: usize = 4096;

fn div_ceil(a: usize, b: usize) -> usize {
    a.div_ceil(b.max(1))
}

/// Cells covered by an image of `img_w` x `img_h` pixels. Resolved once at placement: the stored
/// bitmap is never rescaled, only sampled to this footprint when drawn.
pub fn footprint(img_w: usize, img_h: usize, width: Dim, height: Dim, preserve: bool, m: Metrics) -> (usize, usize) {
    let (img_w, img_h) = (img_w.max(1), img_h.max(1));
    let cells = |d: Dim, cell: usize, total: usize| match d {
        Dim::Auto => None,
        Dim::Cells(n) => Some(n),
        Dim::Px(p) => Some(div_ceil(p, cell)),
        Dim::Percent(p) => Some(total * p / 100),
    };
    let want_c = cells(width, m.cell_w, m.cols).map(|n| n.max(1));
    let want_r = cells(height, m.cell_h, m.rows).map(|n| n.max(1));
    let natural = (div_ceil(img_w, m.cell_w), div_ceil(img_h, m.cell_h));
    let (mut c, mut r) = match (want_c, want_r) {
        (None, None) => natural,
        (Some(c), None) if preserve => (c, div_ceil(c * m.cell_w * img_h, img_w * m.cell_h)),
        (Some(c), None) => (c, natural.1),
        (None, Some(r)) if preserve => (div_ceil(r * m.cell_h * img_w, img_h * m.cell_w), r),
        (None, Some(r)) => (natural.0, r),
        (Some(c), Some(r)) if preserve => {
            let (bw, bh) = (c * m.cell_w, r * m.cell_h);
            if bw * img_h <= bh * img_w {
                (c, div_ceil(bw * img_h, img_w * m.cell_h))
            } else {
                (div_ceil(bh * img_w, img_h * m.cell_w), r)
            }
        }
        (Some(c), Some(r)) => (c, r),
    };
    if c > m.cols && m.cols > 0 {
        if preserve {
            r = div_ceil(r * m.cols, c);
        }
        c = m.cols;
    }
    (c.max(1), r.clamp(1, MAX_ROWS))
}

#[cfg(test)]
mod tests {
    use super::*;

    const M: Metrics = Metrics { cell_w: 10, cell_h: 20, cols: 80, rows: 24 };

    #[test]
    fn arguments_and_data_are_split_at_the_colon() {
        let f = parse(b"name=YS5wbmc=;inline=1;width=20;height=50%;preserveAspectRatio=0:Zm9v").unwrap();
        assert!(f.inline);
        assert_eq!((f.width, f.height), (Dim::Cells(20), Dim::Percent(50)));
        assert!(!f.preserve_aspect);
        assert_eq!(f.data, b"foo");
    }

    #[test]
    fn defaults_are_a_download_with_auto_size_and_kept_aspect() {
        let f = parse(b":Zm9v").unwrap();
        assert!(!f.inline);
        assert_eq!((f.width, f.height), (Dim::Auto, Dim::Auto));
        assert!(f.preserve_aspect);
    }

    #[test]
    fn dimension_units() {
        assert_eq!(Dim::parse("12"), Dim::Cells(12));
        assert_eq!(Dim::parse("120px"), Dim::Px(120));
        assert_eq!(Dim::parse("30%"), Dim::Percent(30));
        assert_eq!(Dim::parse("auto"), Dim::Auto);
        assert_eq!(Dim::parse("x"), Dim::Auto);
    }

    #[test]
    fn missing_data_or_bad_base64_is_an_error() {
        assert!(parse(b"inline=1").is_err());
        assert!(parse(b"inline=1:%%%").is_err());
    }

    #[test]
    fn natural_size_rounds_up_to_whole_cells() {
        assert_eq!(footprint(95, 41, Dim::Auto, Dim::Auto, true, M), (10, 3));
    }

    #[test]
    fn one_given_side_keeps_the_aspect() {
        assert_eq!(footprint(100, 100, Dim::Cells(10), Dim::Auto, true, M), (10, 5));
        assert_eq!(footprint(100, 100, Dim::Auto, Dim::Cells(5), true, M), (10, 5));
        assert_eq!(footprint(100, 100, Dim::Cells(10), Dim::Auto, false, M), (10, 5));
    }

    #[test]
    fn both_sides_fit_inside_the_box_when_the_aspect_is_kept() {
        assert_eq!(footprint(100, 100, Dim::Cells(20), Dim::Cells(5), true, M), (10, 5));
        assert_eq!(footprint(100, 100, Dim::Cells(20), Dim::Cells(5), false, M), (20, 5));
    }

    #[test]
    fn pixel_and_percent_units() {
        assert_eq!(footprint(100, 100, Dim::Px(55), Dim::Px(41), false, M), (6, 3));
        assert_eq!(footprint(100, 100, Dim::Percent(50), Dim::Auto, false, M), (40, 5));
    }

    #[test]
    fn wider_than_the_pane_is_scaled_down_to_fit() {
        assert_eq!(footprint(2000, 1000, Dim::Auto, Dim::Auto, true, M), (80, 20));
        assert_eq!(footprint(2000, 1000, Dim::Auto, Dim::Auto, false, M), (80, 50));
    }

    #[test]
    fn absurd_sizes_are_clamped() {
        let (c, r) = footprint(10, 10, Dim::Cells(1_000_000), Dim::Cells(1_000_000), false, M);
        assert_eq!(c, 80);
        assert_eq!(r, MAX_ROWS);
    }
}
