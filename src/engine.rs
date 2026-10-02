use crate::geom::*;
use crate::{greedy, lattice, output, svgio};
use base64::Engine as _;

pub struct Params {
    /// Inclusive (min, max) sticker width in mm, swept in 1mm steps; None = viewBox units are mm.
    pub sticker_width: Option<(f64, f64)>,
    /// Portrait page dimensions in mm (A4 = 210 x 297); `landscape` swaps them.
    pub page_w: f64,
    pub page_h: f64,
    pub margin: f64,
    pub spacing: f64,
    pub method: String,
    pub rotations: usize,
    pub landscape: bool,
    pub max_count: Option<usize>,
    pub simplify: f64,
    pub greedy_attempts: usize,
    pub stroke: f64,
    pub want_pdf: bool,
    /// Add a full-page white background to the PDFs so Silhouette imports them at document bounds.
    pub pdf_background: bool,
    /// Reserve keep-out zones for Silhouette-style registration marks (inputs in inches).
    pub reg_marks: bool,
    /// Also draw the Cameo marks into the content sheet.
    pub reg_draw: bool,
    /// Also draw the Cameo marks into the outline (cut) sheet.
    pub reg_draw_outline: bool,
    pub reg_length_in: f64,
    pub reg_thickness_in: f64,
    pub reg_inset_l_in: f64,
    pub reg_inset_t_in: f64,
    pub reg_inset_r_in: f64,
    pub reg_inset_b_in: f64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            sticker_width: None,
            page_w: 210.0,
            page_h: 297.0,
            margin: 5.0,
            spacing: 1.5,
            method: "both".into(),
            rotations: 4,
            landscape: false,
            max_count: None,
            simplify: 0.4,
            greedy_attempts: 16,
            stroke: 0.1,
            want_pdf: true,
            pdf_background: true,
            reg_marks: false,
            reg_draw: false,
            reg_draw_outline: false,
            reg_length_in: 0.4,
            reg_thickness_in: 0.02,
            reg_inset_l_in: 0.4,
            reg_inset_t_in: 0.4,
            reg_inset_r_in: 0.4,
            reg_inset_b_in: 0.4,
        }
    }
}

/// Build the page keep-out reservation for the (already landscape-swapped) page. With marks off
/// this is just the uniform page margin. With marks on there are two no-pack zones, matching how
/// Silhouette shows them: (1) everything outside the cut border -- the registration inset defines
/// that border (superseding the page margin), so content stays inside the inset rectangle; and
/// (2) the three Cameo corner marks (solid top-left square + L-brackets top-right and bottom-left),
/// each reserving a `length x length` square just inside that corner.
fn build_reserve(p: &Params, pw: f64, ph: f64) -> Reserve {
    let m = p.margin;
    if !p.reg_marks {
        return Reserve { left: m, top: m, right: m, bottom: m, rects: Vec::new() };
    }
    const MM_PER_IN: f64 = 25.4;
    let (il, it, ir, ib) = (
        p.reg_inset_l_in * MM_PER_IN,
        p.reg_inset_t_in * MM_PER_IN,
        p.reg_inset_r_in * MM_PER_IN,
        p.reg_inset_b_in * MM_PER_IN,
    );
    let len = p.reg_length_in * MM_PER_IN;
    let rects = vec![
        [il, it, il + len, it + len],           // top-left solid square
        [pw - ir - len, it, pw - ir, it + len], // top-right bracket
        [il, ph - ib - len, il + len, ph - ib], // bottom-left bracket
    ];
    Reserve { left: il, top: it, right: ir, bottom: ib, rects }
}

/// True when the reservation maps onto itself under a 180° turn about the page centre -- the
/// condition for `orient_upright`'s sheet flip to be safe. Registration marks (three corners, and
/// possibly asymmetric insets) break this, so the flip must be skipped or it could turn a sticker
/// into a mark or past a border.
fn reserve_symmetric(r: &Reserve) -> bool {
    r.rects.is_empty() && (r.left - r.right).abs() < 1e-9 && (r.top - r.bottom).abs() < 1e-9
}

/// Common page presets in mm (portrait). Returns None for unknown names.
pub fn page_preset(name: &str) -> Option<(f64, f64)> {
    Some(match name.trim().to_ascii_lowercase().as_str() {
        "a3" => (297.0, 420.0),
        "a4" => (210.0, 297.0),
        "a5" => (148.0, 210.0),
        "a6" => (105.0, 148.0),
        "letter" => (215.9, 279.4),
        "legal" => (215.9, 355.6),
        "tabloid" | "ledger" => (279.4, 431.8),
        _ => return None,
    })
}

pub struct Outputs {
    pub count: usize,
    /// Width the outputs were packed at: the largest swept width with the highest count.
    pub width: Option<f64>,
    /// (width, count) for every swept width, ascending.
    pub sweep: Vec<(f64, usize)>,
    pub content_svg: String,
    pub outline_svg: String,
    pub content_pdf: Vec<u8>,
    pub outline_pdf: Vec<u8>,
}

/// Placeholder href the preview emits for a raster instead of a multi-MB base64 data-URI; the
/// web UI swaps it for the art's already-decoded blob URL so the preview renders instantly.
pub const PREVIEW_ART_HREF: &str = "__ART_HREF__";

/// Content-sheet artwork (viewBox-unit inner markup): SVG image inlined (shared viewBox
/// required), or a raster `<image>` covering the whole artboard (its resolution matches the
/// viewBox, so it maps 1:1 onto it and the border clip masks it to the sticker). Empty ext = no
/// separate image, so the border itself is the art. `raster_href` overrides the embedded
/// base64 data-URI (used by the preview to reference a blob URL instead).
pub fn build_inner(
    border_svg: &str,
    image_bytes: &[u8],
    image_ext: &str,
    vb: &[f64; 4],
    raster_href: Option<&str>,
) -> Result<String, String> {
    let ext = image_ext.trim().trim_start_matches('.').to_ascii_lowercase();
    let mime = match ext.as_str() {
        "" => return svgio::load_inner_svg_str(border_svg),
        "svg" => {
            let img = std::str::from_utf8(image_bytes).map_err(|_| "image SVG is not valid UTF-8".to_string())?;
            svgio::require_same_viewbox_str(border_svg, img)?;
            return svgio::load_inner_svg_str(img);
        }
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "bmp" => "image/bmp",
        "gif" => "image/gif",
        "webp" => "image/webp",
        other => return Err(format!("unsupported image type: .{other}")),
    };
    let href = match raster_href {
        Some(h) => h.to_string(),
        None => format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(image_bytes)),
    };
    Ok(format!(
        "<image xlink:href=\"{href}\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" preserveAspectRatio=\"none\"/>",
        vb[0], vb[1], vb[2], vb[3]
    ))
}

/// A standalone SVG of ONE sticker: the art clipped to the border shape, in the border's
/// viewBox. Uses the same outline + clip + art logic as the packed output so the preview
/// matches a placed sticker; rasters reference PREVIEW_ART_HREF rather than embedding base64.
pub fn preview_svg(border_svg: &str, image_bytes: &[u8], image_ext: &str) -> Result<String, String> {
    let outline = svgio::load_outline_str(border_svg)?;
    let art_vb = svgio::read_art_region(border_svg)?;
    let inner = build_inner(border_svg, image_bytes, image_ext, &art_vb, Some(PREVIEW_ART_HREF))?;
    let clip = output::poly_d(&outline);
    // Size the canvas to the OUTLINE, not the art: an auto-outline margin can extend past the art,
    // which would otherwise be clipped by an art-sized viewBox. Pad a little so the outline stroke
    // isn't flush against the edge.
    let (minx, miny, maxx, maxy) = poly_bbox(&outline);
    let pad = (maxx - minx).max(maxy - miny) * 0.03;
    let (vx, vy, vw, vh) = (minx - pad, miny - pad, maxx - minx + 2.0 * pad, maxy - miny + 2.0 * pad);
    Ok(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" \
         width=\"{vw}\" height=\"{vh}\" viewBox=\"{vx} {vy} {vw} {vh}\"><defs><clipPath id=\"c\" clipPathUnits=\"userSpaceOnUse\">\
         <path d=\"{clip}\"/></clipPath></defs><g clip-path=\"url(#c)\">{inner}</g></svg>"
    ))
}

pub fn parse_join_style(s: &str) -> Result<JoinStyle, String> {
    Ok(match s {
        "external" => JoinStyle::RoundExternal,
        "all" => JoinStyle::RoundAll,
        "sharp" => JoinStyle::SharpAll,
        o => return Err(format!("unknown outline style '{o}' (external|all|sharp)")),
    })
}

/// Build an outline SVG by offsetting a traced art silhouette (viewBox-unit `points`, flattened
/// x,y pairs split into rings by `lengths`) outward with the given corner style. The margin on
/// each side is `margin_frac` of the whole outline's width, so it looks the same at any sticker
/// size. The result shares the art's viewBox, so it drops straight into the pipeline as the border.
pub fn auto_outline_svg(
    points: &[f64], lengths: &[u32], vb: &[f64; 4], margin_frac: f64, round_radius: f64, style: JoinStyle, stroke: f64,
) -> Result<String, String> {
    if !(0.0..=MAX_MARGIN_FRAC).contains(&margin_frac) {
        return Err(format!("margin must be 0-{}% of the sticker width", MAX_MARGIN_FRAC * 100.0));
    }
    let mut rings: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut idx = 0usize;
    for &len in lengths {
        let l = len as usize;
        if idx + 2 * l > points.len() {
            break;
        }
        if l >= 3 {
            rings.push((0..l).map(|k| [points[idx + 2 * k], points[idx + 2 * k + 1]]).collect());
        }
        idx += 2 * l;
    }
    if rings.is_empty() {
        return Err("need at least one silhouette contour".into());
    }
    let xs = rings.iter().flatten().map(|p| p[0]);
    let silhouette_w = xs.clone().fold(f64::MIN, f64::max) - xs.fold(f64::MAX, f64::min);
    let outline_w = |o: &Multi| {
        let (minx, _, maxx, _) = multi_bbox(o);
        maxx - minx
    };
    let margin = margin_frac * silhouette_w / (1.0 - 2.0 * margin_frac);
    let mut outline = offset_outline_multi(&rings, margin, round_radius, style);
    if outline.0.is_empty() {
        return Err("could not build an outline from the art".into());
    }
    if margin > 0.0 {
        // width grows by k per unit of margin: 2 for flat or round sides, more at a mitred tip
        let k = (outline_w(&outline) - silhouette_w) / margin;
        if (k - 2.0).abs() > 1e-6 && k * margin_frac < 1.0 {
            outline = offset_outline_multi(&rings, margin_frac * silhouette_w / (1.0 - k * margin_frac), round_radius, style);
        }
    }
    let paths: String = outline
        .0
        .iter()
        .map(|p| format!("<path d=\"{}\" fill=\"none\" stroke=\"#000000\" stroke-width=\"{}\"/>", output::poly_d(p), stroke))
        .collect();
    // The outline is the art dilated by the margin, so it extends past the art's viewBox. Size the
    // canvas to the outline (bbox of all rings), and record the original art region in `data-art`
    // so the art still lands 1:1 in its own box, not stretched to the enlarged viewBox.
    let (minx, miny, maxx, maxy) = multi_bbox(&outline);
    let (w, h) = (maxx - minx, maxy - miny);
    Ok(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"{minx} {miny} {w} {h}\" \
         data-art=\"{} {} {} {}\">{paths}</svg>",
        vb[0], vb[1], vb[2], vb[3]
    ))
}

const MAX_MARGIN_FRAC: f64 = 0.45;

fn multi_bbox(m: &Multi) -> (f64, f64, f64, f64) {
    m.0.iter().map(poly_bbox).fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |(a, b, c, d), (e, f, g, h)| {
        (a.min(e), b.min(f), c.max(g), d.max(h))
    })
}

/// Build the content sheet. Raster art (pdf build) is baked one sticker at a time into a single
/// pre-clipped image so Silhouette imports each as one raster; SVG art keeps the vector clip.
#[cfg(feature = "pdf")]
fn build_content_svg(
    border_svg: &str, image_bytes: &[u8], image_ext: &str,
    outline: &Poly, vb: &[f64; 4], norm: &Mat, placements: &[greedy::Placement], pw: f64, ph: f64,
) -> Result<String, String> {
    let ext = image_ext.trim().trim_start_matches('.').to_ascii_lowercase();
    if matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "bmp" | "gif" | "webp") {
        let png = output::bake_clipped_png(image_bytes, outline, vb)?;
        let href = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(&png));
        return Ok(output::content_svg_baked(&href, vb, norm, placements, pw, ph));
    }
    let inner = build_inner(border_svg, image_bytes, image_ext, vb, None)?;
    Ok(output::content_svg(&inner, outline, norm, placements, pw, ph))
}

#[cfg(not(feature = "pdf"))]
fn build_content_svg(
    border_svg: &str, image_bytes: &[u8], image_ext: &str,
    outline: &Poly, vb: &[f64; 4], norm: &Mat, placements: &[greedy::Placement], pw: f64, ph: f64,
) -> Result<String, String> {
    let inner = build_inner(border_svg, image_bytes, image_ext, vb, None)?;
    Ok(output::content_svg(&inner, outline, norm, placements, pw, ph))
}

/// Angular distance of `deg` from the artwork's original orientation (0°), in [0, 180].
fn upright_deviation(deg: f64) -> f64 {
    let a = deg.rem_euclid(360.0);
    a.min(360.0 - a)
}

/// If a rigid 180° turn of the whole sheet leaves the stickers nearer their original (0°)
/// orientation, apply it. The turn is about the page centre, so content and outline stay
/// registered and every sticker stays inside the centre-symmetric margin box -- it just fixes
/// packings that otherwise come out predominantly upside-down.
fn orient_upright(placements: Vec<greedy::Placement>, pw: f64, ph: f64) -> Vec<greedy::Placement> {
    let as_is: f64 = placements.iter().map(|p| upright_deviation(p.angle)).sum();
    let flipped: f64 = placements.iter().map(|p| upright_deviation(p.angle + 180.0)).sum();
    if flipped >= as_is {
        return placements;
    }
    placements
        .into_iter()
        .map(|p| greedy::Placement { angle: (p.angle + 180.0).rem_euclid(360.0), x: pw - p.x, y: ph - p.y })
        .collect()
}

struct ParsedBorder {
    outline: Poly,
    art_vb: [f64; 4],
}

impl ParsedBorder {
    fn parse(svg: &str) -> Result<Self, String> {
        let outline = svgio::load_outline_str(svg)?;
        // The art raster maps into its own box (which the auto-outline records separately from the
        // enlarged border viewBox), not the border's viewBox.
        let art_vb = svgio::read_art_region(svg)?;
        // A degenerate outline (collinear / zero-area) triangulates to nothing, which would panic in
        // buffer()/largest() and disable collision detection; reject it with a clear error instead.
        let (ominx, ominy, omaxx, omaxy) = poly_bbox(&outline);
        if omaxx - ominx < 1e-9 || omaxy - ominy < 1e-9 || area_poly(&outline) < 1e-9 {
            return Err("border outline is degenerate (near-zero area or size); check the border SVG".into());
        }
        Ok(Self { outline, art_vb })
    }
}

struct Layout {
    norm: Poly,
    norm_mat: Mat,
    placements: Vec<greedy::Placement>,
}

/// Widths to try: `a`, `a + 1`, ... up to and including `b` (either order).
pub fn sweep_widths(a: f64, b: f64) -> Vec<f64> {
    let (lo, hi) = (a.min(b), a.max(b));
    let steps = (hi - lo + 1e-9).floor() as usize;
    let mut widths: Vec<f64> = (0..=steps).map(|i| lo + i as f64).collect();
    if lo + (steps as f64) < hi - 1e-9 {
        widths.push(hi);
    }
    widths
}

/// Index of the width to keep from ascending sweep `counts`: the largest width with the highest
/// nonzero count.
pub fn best_sweep_index(counts: &[usize]) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (i, &count) in counts.iter().enumerate() {
        if beats(count, best.map(|b| counts[b])) {
            best = Some(i);
        }
    }
    best
}

fn beats(count: usize, best: Option<usize>) -> bool {
    count > 0 && best.is_none_or(|b| count >= b)
}

struct Sheet {
    pw: f64,
    ph: f64,
    rots: Vec<f64>,
    reserve: Reserve,
}

impl Sheet {
    fn new(p: &Params) -> Self {
        let (mut pw, mut ph) = (p.page_w, p.page_h);
        if p.landscape {
            std::mem::swap(&mut pw, &mut ph);
        }
        let n = p.rotations.max(1);
        let rots = (0..n).map(|i| (i as f64 * 360.0 / n as f64 * 1e6).round() / 1e6).collect();
        Self { pw, ph, rots, reserve: build_reserve(p, pw, ph) }
    }
}

/// Stickers that fit at one width (`p.sticker_width` is ignored), without rendering any output.
pub fn pack_count(border_svg: &str, width: Option<f64>, p: &Params) -> Result<usize, String> {
    if width.is_some_and(|w| w <= 0.0) {
        return Err("sticker width must be positive".into());
    }
    let border = ParsedBorder::parse(border_svg)?;
    Ok(pack_layout(&border.outline, width, p, &Sheet::new(p), &|_, _| {})?.placements.len())
}

/// The whole pipeline, filesystem-free: border+image content in, four outputs out. A sticker
/// width range is swept, and the outputs are rendered once for the winning width.
/// `progress(stage, fraction)` is called at phase boundaries (0..1) for UI feedback.
pub fn run_pack(
    border_svg: &str,
    image_bytes: &[u8],
    image_ext: &str,
    p: &Params,
    progress: &dyn Fn(&str, f64),
) -> Result<Outputs, String> {
    if p.reg_marks && (p.reg_draw || p.reg_draw_outline) {
        if p.reg_thickness_in <= 0.0 || p.reg_thickness_in * IN >= 5.0 {
            return Err("registration mark thickness must be positive and under 5 mm".into());
        }
        if p.reg_length_in <= 0.0 {
            return Err("registration mark length must be positive".into());
        }
    }
    progress("Preparing", 0.05);
    let ParsedBorder { outline, art_vb: vb } = ParsedBorder::parse(border_svg)?;
    let sheet = Sheet::new(p);
    let (pw, ph) = (sheet.pw, sheet.ph);

    let widths: Vec<Option<f64>> = match p.sticker_width {
        Some((a, b)) => sweep_widths(a, b).into_iter().map(Some).collect(),
        None => vec![None],
    };
    let mut sweep: Vec<(f64, usize)> = Vec::new();
    let mut best: Option<(Option<f64>, Layout)> = None;
    let mut last_err: Option<String> = None;
    for (i, &w) in widths.iter().enumerate() {
        let (lo, hi) = (0.15 + 0.65 * i as f64 / widths.len() as f64, 0.15 + 0.65 * (i + 1) as f64 / widths.len() as f64);
        let step_progress = |stage: &str, f: f64| match w {
            Some(w) if widths.len() > 1 => progress(&format!("{stage}, {w} mm ({}/{})", i + 1, widths.len()), lo + (hi - lo) * f),
            _ => progress(stage, lo + (hi - lo) * f),
        };
        let count = match pack_layout(&outline, w, p, &sheet, &step_progress) {
            Ok(layout) => {
                let count = layout.placements.len();
                if beats(count, best.as_ref().map(|(_, b)| b.placements.len())) {
                    best = Some((w, layout));
                }
                count
            }
            Err(e) => {
                last_err = Some(e);
                0
            }
        };
        if let Some(w) = w {
            sweep.push((w, count));
        }
    }
    let Some((width, layout)) = best else {
        return Err(last_err.unwrap_or_else(|| "sticker does not fit on the page (check margin / sticker width)".into()));
    };
    let Layout { norm, norm_mat, placements } = layout;

    const IN: f64 = 25.4;
    let marks = output::registration_marks(
        pw, ph, p.reg_length_in * IN, p.reg_thickness_in * IN,
        p.reg_inset_l_in * IN, p.reg_inset_t_in * IN, p.reg_inset_r_in * IN, p.reg_inset_b_in * IN,
    );
    let with_marks = |mut svg: String, draw: bool| {
        if p.reg_marks && draw {
            if let Some(close) = svg.rfind("</svg>") {
                svg.insert_str(close, &marks);
            }
        }
        svg
    };

    progress("Content sheet", 0.82);
    let content_svg = build_content_svg(border_svg, image_bytes, image_ext, &outline, &vb, &norm_mat, &placements, pw, ph)?;
    let content_svg = with_marks(content_svg, p.reg_draw);
    progress("Outline sheet", 0.86);
    // Cut file from the ORIGINAL border geometry (curves preserved), not the flattened packing
    // polygon; fall back to the polygon if the SVG has no extractable path.
    let segs = svgio::outline_path_segs(border_svg, &norm_mat).unwrap_or_else(|_| output::poly_segs(&norm));
    // plottie's svgoutline truncates the page to whole px at 5 px/mm; any other size scales y alone,
    // skewing the bracket arms so plottie can't detect the marks
    let (opw, oph) = if p.reg_marks && p.reg_draw_outline { (pw.ceil(), ph.ceil()) } else { (pw, ph) };
    let outline_svg = with_marks(output::outline_svg(&segs, &placements, opw, oph, p.stroke), p.reg_draw_outline);
    let (content_pdf, outline_pdf) = if p.want_pdf {
        progress("Rendering PDF", 0.9);
        if p.pdf_background {
            (pdf_of(&output::add_background(&content_svg, pw, ph))?, pdf_of(&output::add_background(&outline_svg, opw, oph))?)
        } else {
            (pdf_of(&content_svg)?, pdf_of(&outline_svg)?)
        }
    } else {
        (Vec::new(), Vec::new())
    };
    progress("Done", 1.0);
    Ok(Outputs { count: placements.len(), width, sweep, content_svg, outline_svg, content_pdf, outline_pdf })
}

/// `progress` fractions are 0..1 within this step.
fn pack_layout(
    outline: &Poly,
    width: Option<f64>,
    p: &Params,
    sheet: &Sheet,
    progress: &dyn Fn(&str, f64),
) -> Result<Layout, String> {
    let Sheet { pw, ph, rots, reserve } = sheet;
    let (pw, ph) = (*pw, *ph);
    let (norm, norm_mat) = normalize(outline, width);
    let packing = simplify_poly(&norm, p.simplify);
    let grown = simplify_poly(&buffer(&packing, p.spacing / 2.0 + 1e-4, 16), p.simplify);
    let placements = match p.method.as_str() {
        "greedy" => {
            progress("Packing (greedy)", 0.0);
            greedy::pack(&grown, rots, pw, ph, reserve, p.max_count, p.greedy_attempts)
        }
        "lattice" => {
            progress("Packing (lattice)", 0.0);
            lattice::pack(&grown, rots, pw, ph, reserve, p.max_count)
        }
        "both" => {
            progress("Packing (greedy)", 0.0);
            let g = greedy::pack(&grown, rots, pw, ph, reserve, p.max_count, p.greedy_attempts);
            progress("Packing (lattice)", 0.5);
            let l = lattice::pack(&grown, rots, pw, ph, reserve, p.max_count);
            if g.len() >= l.len() { g } else { l }
        }
        m => return Err(format!("unknown method '{m}' (both|greedy|lattice)")),
    };
    // The upright flip turns the whole sheet 180°; only safe when the reservation is symmetric under
    // that turn, else it could move a sticker onto a registration mark or past an asymmetric border.
    let placements = if reserve_symmetric(reserve) { orient_upright(placements, pw, ph) } else { placements };
    Ok(Layout { norm, norm_mat, placements })
}

#[cfg(feature = "pdf")]
fn pdf_of(svg: &str) -> Result<Vec<u8>, String> {
    output::svg_to_pdf(svg)
}
#[cfg(not(feature = "pdf"))]
fn pdf_of(_svg: &str) -> Result<Vec<u8>, String> {
    Err("PDF output is not available in this build".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserve_is_inset_border_plus_corner_marks() {
        let p = Params { reg_marks: true, margin: 5.0, ..Default::default() };
        let (pw, ph) = (210.0, 297.0);
        let r = build_reserve(&p, pw, ph);
        let inset = 0.4 * 25.4;
        // border: content inside the cut-border line (inset dominates the 5mm page margin)
        for side in [r.left, r.top, r.right, r.bottom] {
            assert!((side - inset).abs() < 1e-9, "border side {side} != inset {inset}");
        }
        // corners: three Cameo mark squares
        assert_eq!(r.rects.len(), 3);
    }

    #[test]
    fn upright_flip_disabled_with_marks() {
        // Plain margins are 180°-symmetric (flip runs); registration marks are not (flip skipped).
        let plain = build_reserve(&Params { reg_marks: false, ..Default::default() }, 210.0, 297.0);
        assert!(reserve_symmetric(&plain));
        let marks = build_reserve(&Params { reg_marks: true, ..Default::default() }, 210.0, 297.0);
        assert!(!reserve_symmetric(&marks));
    }

    #[test]
    fn marks_drawn_only_into_selected_sheets() {
        let border = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 10 10\"><path d=\"M0,0 L10,0 L10,10 L0,10 Z\"/></svg>";
        let marks = output::registration_marks(210.0, 297.0, 0.4 * 25.4, 0.02 * 25.4, 0.4 * 25.4, 0.4 * 25.4, 0.4 * 25.4, 0.4 * 25.4);
        let run = |reg_draw, reg_draw_outline| {
            let p = Params {
                sticker_width: Some((30.0, 30.0)),
                want_pdf: false,
                reg_marks: true,
                reg_draw,
                reg_draw_outline,
                ..Default::default()
            };
            let out = run_pack(border, border.as_bytes(), "svg", &p, &|_, _| {}).unwrap();
            (out.content_svg.contains(&marks), out.outline_svg.contains(&marks))
        };
        assert_eq!(run(false, false), (false, false));
        assert_eq!(run(true, false), (true, false));
        assert_eq!(run(false, true), (false, true));
        assert_eq!(run(true, true), (true, true));
    }

    #[test]
    fn marks_meet_plottie_detection_rules() {
        let attr = |tag: &str, name: &str| -> f64 {
            let v = &tag[tag.find(&format!(" {name}=\"")).unwrap() + name.len() + 3..];
            v[..v.find('"').unwrap()].parse().unwrap()
        };
        // Insets with more than 4 decimals, so endpoints round independently.
        for i in 0..20 {
            let inset = 5.0 + i as f64 * 0.0123457;
            let len = 10.4742 + inset / 7.0;
            let marks = output::registration_marks(203.123457, 291.987654, len, 0.44, inset, inset * 1.3, inset * 0.9, inset * 1.1);
            let stroked = marks.split("<rect").find(|r| r.contains("stroke=")).unwrap();
            assert!((attr(stroked, "width") + attr(stroked, "stroke-width") - 5.0).abs() < 1e-9);
            for path in marks.split("<path d=\"M").skip(1) {
                let d = &path[..path.find('"').unwrap()];
                let p: Vec<f64> = d.split([',', 'L', ' ']).filter(|t| !t.is_empty()).map(|t| t.parse().unwrap()).collect();
                let (h, v) = ((p[2] - p[0]).abs(), (p[5] - p[3]).abs());
                assert!((h - v).abs() < 1e-9, "inset {inset}: arms {h} vs {v}");
            }
        }
    }

    #[test]
    fn outline_sheet_with_marks_rounds_page_up_to_whole_mm() {
        let border = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 10 10\"><path d=\"M0,0 L10,0 L10,10 L0,10 Z\"/></svg>";
        let p = Params {
            sticker_width: Some((30.0, 30.0)),
            page_w: 215.9,
            page_h: 279.4,
            want_pdf: false,
            reg_marks: true,
            reg_draw: true,
            reg_draw_outline: true,
            ..Default::default()
        };
        let out = run_pack(border, border.as_bytes(), "svg", &p, &|_, _| {}).unwrap();
        assert!(out.outline_svg.contains("width=\"216mm\" height=\"280mm\""));
        assert!(out.content_svg.contains("width=\"215.9mm\" height=\"279.4mm\""));

        let thin = Params { reg_thickness_in: 0.0, ..p };
        assert!(run_pack(border, border.as_bytes(), "svg", &thin, &|_, _| {}).is_err());
    }

    #[test]
    fn svg_art_matches_border_art_region_not_viewbox() {
        // The auto-outline border's viewBox is enlarged past the art, but its data-art matches the
        // SVG art's viewBox, so the alignment check must pass.
        let border = "<svg viewBox=\"-15 -15 130 130\" data-art=\"0 0 100 100\"></svg>";
        let art = "<svg viewBox=\"0 0 100 100\"></svg>";
        assert!(crate::svgio::require_same_viewbox_str(border, art).is_ok());
    }

    #[test]
    fn art_region_from_data_attr_or_viewbox() {
        // Auto-outline: enlarged viewBox, art region carried in data-art.
        let with = "<svg viewBox=\"-15 -15 130 130\" data-art=\"0 0 100 100\"></svg>";
        assert_eq!(crate::svgio::read_art_region(with).unwrap(), [0.0, 0.0, 100.0, 100.0]);
        // Plain border: art fills the viewBox.
        let without = "<svg viewBox=\"0 0 50 60\"></svg>";
        assert_eq!(crate::svgio::read_art_region(without).unwrap(), [0.0, 0.0, 50.0, 60.0]);
    }

    #[test]
    fn preview_canvas_fits_outline_beyond_art() {
        // Border viewBox is 100x100 but the outline path extends to [-10,120]; the preview canvas
        // must be sized to the outline, not the art, or the outline goes out of view.
        let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 100 100\" width=\"100\" height=\"100\"><path d=\"M-10,-10 L120,-10 L120,120 L-10,120 Z\" fill=\"none\" stroke=\"#000\"/></svg>";
        let out = preview_svg(svg, &[], "").unwrap();
        let vb = out.split("viewBox=\"").nth(1).unwrap().split('"').next().unwrap();
        let nums: Vec<f64> = vb.split_whitespace().map(|s| s.parse().unwrap()).collect();
        assert!(nums[2] > 120.0, "preview width {} must include the 130-wide outline", nums[2]);
    }

    #[test]
    fn sweep_steps_1mm_inclusive() {
        assert_eq!(sweep_widths(40.0, 40.0), vec![40.0]);
        assert_eq!(sweep_widths(40.0, 43.0), vec![40.0, 41.0, 42.0, 43.0]);
        assert_eq!(sweep_widths(40.5, 42.0), vec![40.5, 41.5, 42.0]);
        assert_eq!(best_sweep_index(&[3, 5, 5, 4]), Some(2));
        assert_eq!(best_sweep_index(&[0, 0]), None);
    }

    #[test]
    fn sweep_keeps_largest_width_with_most_stickers() {
        // 2:1 rectangle, upright only, on a 100mm square page: floor(100/w) * floor(200/w)
        let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 100 50\" width=\"100\" height=\"50\"><path d=\"M0,0 L100,0 L100,50 L0,50 Z\"/></svg>";
        let p = Params {
            sticker_width: Some((31.0, 34.0)),
            page_w: 100.0,
            page_h: 100.0,
            margin: 0.0,
            spacing: 0.0,
            method: "greedy".into(),
            rotations: 1,
            want_pdf: false,
            ..Default::default()
        };
        let out = run_pack(svg, &[], "", &p, &|_, _| {}).unwrap();
        assert_eq!(out.sweep, vec![(31.0, 18), (32.0, 18), (33.0, 18), (34.0, 10)]);
        assert_eq!(out.width, Some(33.0));
        assert_eq!(out.count, 18);
        assert_eq!(pack_count(svg, Some(34.0), &p), Ok(10));
    }

    #[test]
    fn auto_outline_margin_is_a_fraction_of_outline_width() {
        let rect = [0.0, 0.0, 100.0, 0.0, 100.0, 50.0, 0.0, 50.0];
        let svg = auto_outline_svg(&rect, &[4], &[0.0, 0.0, 100.0, 50.0], 0.05, 0.0, JoinStyle::SharpAll, 0.1).unwrap();
        let vb = svg.split("viewBox=\"").nth(1).unwrap().split('"').next().unwrap();
        let nums: Vec<f64> = vb.split_whitespace().map(|s| s.parse().unwrap()).collect();
        // 100 wide art + 5% of the outline width each side: w = 100 / 0.9
        assert!((nums[2] - 100.0 / 0.9).abs() < 1e-6, "outline width {}", nums[2]);
        assert!(auto_outline_svg(&rect, &[4], &[0.0, 0.0, 100.0, 50.0], 0.46, 0.0, JoinStyle::SharpAll, 0.1).is_err());
        assert!(auto_outline_svg(&rect, &[4], &[0.0, 0.0, 100.0, 50.0], -0.01, 0.0, JoinStyle::SharpAll, 0.1).is_err());

        // mitred 53° tip on the right grows the outline by more than the margin; the flat left side
        // sits exactly one margin out, and must still be 5% of the full width
        let pencil = [0.0, 0.0, 70.0, 0.0, 110.0, 20.0, 70.0, 40.0, 0.0, 40.0];
        let svg = auto_outline_svg(&pencil, &[5], &[0.0, 0.0, 110.0, 40.0], 0.05, 0.0, JoinStyle::SharpAll, 0.1).unwrap();
        let vb = svg.split("viewBox=\"").nth(1).unwrap().split('"').next().unwrap();
        let nums: Vec<f64> = vb.split_whitespace().map(|s| s.parse().unwrap()).collect();
        assert!((-nums[0] / nums[2] - 0.05).abs() < 1e-6, "margin {} of width {}", -nums[0], nums[2]);
    }

    #[cfg(feature = "pdf")]
    #[test]
    fn foreign_namespace_art_renders_to_pdf() {
        let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:affinity=\"http://www.serif.com/\" viewBox=\"0 0 100 100\" width=\"100\" height=\"100\"><path affinity:id=\"a\" d=\"M0,0 L100,0 L100,100 L0,100 Z\"/></svg>";
        let p = Params { want_pdf: true, ..Default::default() };
        run_pack(svg, &[], "", &p, &|_, _| {}).unwrap();
    }

    #[test]
    fn degenerate_outline_errors_not_panics() {
        // Collinear border (zero area) used to triangulate to nothing and panic in buffer()/largest().
        let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 100 100\" width=\"100\" height=\"100\"><path d=\"M0,0 L100,0 L50,0 Z\"/></svg>";
        let p = Params { want_pdf: false, ..Default::default() };
        assert!(run_pack(svg, &[], "", &p, &|_, _| {}).is_err());
    }
}
