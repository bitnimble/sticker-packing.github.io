use crate::geom::*;
use crate::{greedy, lattice, output, svgio};
use base64::Engine as _;

pub struct Params {
    pub sticker_width: Option<f64>,
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
        }
    }
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
    pub content_svg: String,
    pub outline_svg: String,
    pub content_pdf: Vec<u8>,
    pub outline_pdf: Vec<u8>,
}

/// Content-sheet artwork (viewBox-unit inner markup): SVG image inlined (shared viewBox
/// required), or a raster embedded as a base64 <image> covering the border bbox. Empty ext =
/// no separate image, so the border itself is the art.
pub fn build_inner(border_svg: &str, image_bytes: &[u8], image_ext: &str, outline: &Poly) -> Result<String, String> {
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
    let b64 = base64::engine::general_purpose::STANDARD.encode(image_bytes);
    let (x0, y0, x1, y1) = poly_bbox(outline);
    Ok(format!(
        "<image xlink:href=\"data:{mime};base64,{b64}\" x=\"{x0}\" y=\"{y0}\" width=\"{}\" height=\"{}\" preserveAspectRatio=\"xMidYMid slice\"/>",
        x1 - x0,
        y1 - y0
    ))
}

/// A standalone SVG of ONE sticker: the art clipped to the border shape, in the border's
/// viewBox. Uses the exact same outline + clip + raster-embed logic as the packed output, so
/// the preview matches what a placed sticker will look like.
pub fn preview_svg(border_svg: &str, image_bytes: &[u8], image_ext: &str) -> Result<String, String> {
    let outline = svgio::load_outline_str(border_svg)?;
    let inner = build_inner(border_svg, image_bytes, image_ext, &outline)?;
    let vb = svgio::read_viewbox_str(border_svg)?;
    let clip = output::poly_d(&outline);
    Ok(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" \
         viewBox=\"{} {} {} {}\"><defs><clipPath id=\"c\" clipPathUnits=\"userSpaceOnUse\">\
         <path d=\"{}\"/></clipPath></defs><g clip-path=\"url(#c)\">{}</g></svg>",
        vb[0], vb[1], vb[2], vb[3], clip, inner
    ))
}

/// The whole pipeline, filesystem-free: border+image content in, four outputs out.
/// `progress(stage, fraction)` is called at phase boundaries (0..1) for UI feedback.
pub fn run_pack(
    border_svg: &str,
    image_bytes: &[u8],
    image_ext: &str,
    p: &Params,
    progress: &dyn Fn(&str, f64),
) -> Result<Outputs, String> {
    let (mut pw, mut ph) = (p.page_w, p.page_h);
    if p.landscape {
        std::mem::swap(&mut pw, &mut ph);
    }
    progress("Preparing", 0.05);
    let outline = svgio::load_outline_str(border_svg)?;
    let inner = build_inner(border_svg, image_bytes, image_ext, &outline)?;
    let (norm, norm_mat) = normalize(&outline, p.sticker_width);
    let packing = simplify_poly(&norm, p.simplify);
    let grown = simplify_poly(&buffer(&packing, p.spacing / 2.0 + 1e-4, 16), p.simplify);

    let n = p.rotations.max(1);
    let rots: Vec<f64> = (0..n).map(|i| (i as f64 * 360.0 / n as f64 * 1e6).round() / 1e6).collect();

    let placements = match p.method.as_str() {
        "greedy" => {
            progress("Packing (greedy)", 0.15);
            greedy::pack(&grown, &rots, pw, ph, p.margin, p.max_count, p.greedy_attempts)
        }
        "lattice" => {
            progress("Packing (lattice)", 0.15);
            lattice::pack(&grown, &rots, pw, ph, p.margin, p.max_count)
        }
        "both" => {
            progress("Packing (greedy)", 0.15);
            let g = greedy::pack(&grown, &rots, pw, ph, p.margin, p.max_count, p.greedy_attempts);
            progress("Packing (lattice)", 0.5);
            let l = lattice::pack(&grown, &rots, pw, ph, p.margin, p.max_count);
            if g.len() >= l.len() { g } else { l }
        }
        m => return Err(format!("unknown method '{m}' (both|greedy|lattice)")),
    };
    if placements.is_empty() {
        return Err("sticker does not fit on the page (check margin / sticker width)".into());
    }

    progress("Content sheet", 0.82);
    let content_svg = output::content_svg(&inner, &outline, &norm_mat, &placements, pw, ph);
    progress("Outline sheet", 0.86);
    let outline_svg = output::outline_svg(&norm, &placements, pw, ph, p.stroke);
    let (content_pdf, outline_pdf) = if p.want_pdf {
        progress("Rendering PDF", 0.9);
        (pdf_of(&content_svg)?, pdf_of(&outline_svg)?)
    } else {
        (Vec::new(), Vec::new())
    };
    progress("Done", 1.0);
    Ok(Outputs { count: placements.len(), content_svg, outline_svg, content_pdf, outline_pdf })
}

#[cfg(feature = "pdf")]
fn pdf_of(svg: &str) -> Result<Vec<u8>, String> {
    output::svg_to_pdf(svg)
}
#[cfg(not(feature = "pdf"))]
fn pdf_of(_svg: &str) -> Result<Vec<u8>, String> {
    Err("PDF output is not available in this build".into())
}
