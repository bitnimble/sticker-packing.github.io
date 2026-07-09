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
    let vb = svgio::read_viewbox_str(border_svg)?;
    let inner = build_inner(border_svg, image_bytes, image_ext, &vb, Some(PREVIEW_ART_HREF))?;
    let clip = output::poly_d(&outline);
    Ok(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" \
         width=\"{}\" height=\"{}\" viewBox=\"{} {} {} {}\"><defs><clipPath id=\"c\" clipPathUnits=\"userSpaceOnUse\">\
         <path d=\"{}\"/></clipPath></defs><g clip-path=\"url(#c)\">{}</g></svg>",
        vb[2], vb[3], vb[0], vb[1], vb[2], vb[3], clip, inner
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
/// x,y pairs) outward by `margin` with the given corner style. The result shares the art's
/// viewBox, so it drops straight into the pipeline as the border.
pub fn auto_outline_svg(points: &[f64], lengths: &[u32], vb: &[f64; 4], margin: f64, round_radius: f64, style: JoinStyle, stroke: f64) -> Result<String, String> {
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
    let outline = offset_outline_multi(&rings, margin, round_radius, style);
    if outline.0.is_empty() {
        return Err("could not build an outline from the art".into());
    }
    let paths: String = outline
        .0
        .iter()
        .map(|p| format!("<path d=\"{}\" fill=\"none\" stroke=\"#000000\" stroke-width=\"{}\"/>", output::poly_d(p), stroke))
        .collect();
    Ok(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\" viewBox=\"{} {} {} {}\">{}</svg>",
        vb[2], vb[3], vb[0], vb[1], vb[2], vb[3], paths
    ))
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
    let vb = svgio::read_viewbox_str(border_svg)?;
    let inner = build_inner(border_svg, image_bytes, image_ext, &vb, None)?;
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
    let placements = orient_upright(placements, pw, ph);

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
