use crate::geom::*;
use crate::greedy::Placement;
use geo::LineString;

fn header(pw: f64, ph: f64) -> String {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" \
         width=\"{pw}mm\" height=\"{ph}mm\" viewBox=\"0 0 {pw} {ph}\">\n"
    )
}

/// SVG `matrix(a b c d e f)` string from our (a,b,c,d,e,f) with x'=a*x+b*y+c: SVG orders it
/// column-major, so a,b = first column, etc.
fn svg_matrix(m: &Mat) -> String {
    format!("matrix({} {} {} {} {} {})", m[0], m[3], m[1], m[4], m[2], m[5])
}

fn ring_d(ring: &LineString<f64>, d: &mut String) {
    for (i, c) in ring.0.iter().enumerate() {
        d.push_str(if i == 0 { "M " } else { " L " });
        d.push_str(&format!("{:.4},{:.4}", c.x, c.y));
    }
    d.push_str(" Z");
}

pub fn poly_d(p: &Poly) -> String {
    let mut d = String::new();
    ring_d(p.exterior(), &mut d);
    for h in p.interiors() {
        d.push(' ');
        ring_d(h, &mut d);
    }
    d
}

/// Content sheet: the image artwork (viewBox-unit `inner`) tessellated and clipped to the
/// border shape (`border_vb`, the border outline in viewBox units), so oversized vector/raster
/// art is masked to the sticker. Defined once in <defs> and <use>d per placement so artwork
/// IDs never collide; each instance transform = place ∘ norm (viewBox units -> page mm). The
/// clip is userSpaceOnUse in the same VB-unit space as the art, so both transform together.
pub fn content_svg(inner: &str, border_vb: &Poly, norm: &Mat, placements: &[Placement], pw: f64, ph: f64) -> String {
    let mut s = header(pw, ph);
    s.push_str("<defs>");
    s.push_str(&format!(
        "<clipPath id=\"border\" clipPathUnits=\"userSpaceOnUse\"><path d=\"{}\"/></clipPath>",
        poly_d(border_vb)
    ));
    s.push_str("<g id=\"sticker\" clip-path=\"url(#border)\">");
    s.push_str(inner);
    s.push_str("</g></defs>\n");
    for p in placements {
        let m = mat_compose(&place_mat(p.angle, p.x, p.y), norm);
        s.push_str(&format!("<use xlink:href=\"#sticker\" transform=\"{}\"/>\n", svg_matrix(&m)));
    }
    s.push_str("</svg>\n");
    s
}

/// Outline sheet (cut file): the un-grown border outline at the same placements, as unfilled
/// stroked cut lines. `norm_border` is the normalized (packing-space) border polygon.
pub fn outline_svg(norm_border: &Poly, placements: &[Placement], pw: f64, ph: f64, stroke: f64) -> String {
    let mut s = header(pw, ph);
    for p in placements {
        let poly = transform_poly(norm_border, &place_mat(p.angle, p.x, p.y));
        s.push_str(&format!(
            "<path d=\"{}\" fill=\"none\" stroke=\"#000000\" stroke-width=\"{stroke}\"/>\n",
            poly_d(&poly)
        ));
    }
    s.push_str("</svg>\n");
    s
}

/// Render an SVG string to a PDF (dpi=96 so mm -> pt yields exact physical A4).
#[cfg(feature = "pdf")]
pub fn svg_to_pdf(svg: &str) -> Result<Vec<u8>, String> {
    // svg2pdf re-exports its own usvg version; use it so the Tree types match.
    let tree = svg2pdf::usvg::Tree::from_str(svg, &svg2pdf::usvg::Options::default())
        .map_err(|e| format!("pdf parse: {e}"))?;
    let page = svg2pdf::PageOptions { dpi: 96.0 };
    svg2pdf::to_pdf(&tree, svg2pdf::ConversionOptions::default(), page)
        .map_err(|e| format!("pdf convert: {e}"))
}
