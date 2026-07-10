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
    // Define the cut path ONCE (place_mat is rigid, so stroke width stays uniform) and <use> it
    // per placement -- otherwise the full-resolution border is re-formatted for every sticker,
    // which dominates runtime for high-vertex outlines.
    s.push_str(&format!("<defs><path id=\"cut\" d=\"{}\"/></defs>\n", poly_d(norm_border)));
    for p in placements {
        s.push_str(&format!(
            "<use xlink:href=\"#cut\" transform=\"{}\" fill=\"none\" stroke=\"#000000\" stroke-width=\"{stroke}\"/>\n",
            svg_matrix(&place_mat(p.angle, p.x, p.y))
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
    let pdf = svg2pdf::to_pdf(&tree, svg2pdf::ConversionOptions::default(), page)
        .map_err(|e| format!("pdf convert: {e}"))?;
    // svg2pdf emits a fresh image XObject per placement (it expands every <use>), so a tessellated
    // raster is embedded N times. Merge byte-identical streams back into one shared object.
    Ok(dedupe_streams(pdf))
}

/// Collapse duplicate stream objects (identical images / soft-masks that svg2pdf duplicated per
/// placement) into a single shared object and repoint every reference. Purely a size optimization:
/// merging value-identical objects cannot change what the PDF renders. On any parse/write failure
/// the original bytes are returned unchanged.
#[cfg(feature = "pdf")]
fn dedupe_streams(bytes: Vec<u8>) -> Vec<u8> {
    use lopdf::{Document, Object, ObjectId};
    use std::collections::HashMap;

    let mut doc = match Document::load_mem(&bytes) {
        Ok(d) => d,
        Err(_) => return bytes,
    };
    // Fixpoint: masks (no inner refs) merge first; once images' /SMask refs are repointed to the
    // canonical mask, the image streams become identical too and merge on the next pass.
    loop {
        let mut canonical: HashMap<u64, ObjectId> = HashMap::new();
        let mut remap: HashMap<ObjectId, ObjectId> = HashMap::new();
        for (&id, obj) in &doc.objects {
            if matches!(obj, Object::Stream(_)) {
                let h = hash_object(obj);
                match canonical.get(&h) {
                    Some(&keep) => { remap.insert(id, keep); }
                    None => { canonical.insert(h, id); }
                }
            }
        }
        if remap.is_empty() {
            break;
        }
        for id in remap.keys() {
            doc.objects.remove(id);
        }
        for obj in doc.objects.values_mut() {
            remap_refs(obj, &remap);
        }
        for (_, v) in doc.trailer.iter_mut() {
            remap_refs(v, &remap);
        }
    }
    doc.renumber_objects();
    let mut out = Vec::new();
    match doc.save_to(&mut out) {
        Ok(()) => out,
        Err(_) => bytes,
    }
}

#[cfg(feature = "pdf")]
fn hash_object(o: &lopdf::Object) -> u64 {
    use std::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    hash_into(o, &mut h);
    h.finish()
}

#[cfg(feature = "pdf")]
fn hash_into(o: &lopdf::Object, h: &mut impl std::hash::Hasher) {
    use lopdf::Object::*;
    match o {
        Null => h.write_u8(0),
        Boolean(b) => { h.write_u8(1); h.write_u8(*b as u8); }
        Integer(i) => { h.write_u8(2); h.write_i64(*i); }
        Real(r) => { h.write_u8(3); h.write_u32(r.to_bits()); }
        Name(n) => { h.write_u8(4); h.write(n); }
        String(s, _) => { h.write_u8(5); h.write(s); }
        Reference(id) => { h.write_u8(6); h.write_u32(id.0); h.write_u16(id.1); }
        Array(a) => { h.write_u8(7); for x in a { hash_into(x, h); } }
        Dictionary(d) => { h.write_u8(8); hash_dict(d, h); }
        Stream(s) => { h.write_u8(9); hash_dict(&s.dict, h); h.write(&s.content); }
    }
}

#[cfg(feature = "pdf")]
fn hash_dict(d: &lopdf::Dictionary, h: &mut impl std::hash::Hasher) {
    let mut entries: Vec<(&Vec<u8>, &lopdf::Object)> = d.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    for (k, v) in entries {
        h.write(k);
        hash_into(v, h);
    }
}

#[cfg(feature = "pdf")]
fn remap_refs(o: &mut lopdf::Object, m: &std::collections::HashMap<lopdf::ObjectId, lopdf::ObjectId>) {
    use lopdf::Object::*;
    match o {
        Reference(id) => { if let Some(&c) = m.get(id) { *id = c; } }
        Array(a) => { for x in a { remap_refs(x, m); } }
        Dictionary(d) => { for (_, v) in d.iter_mut() { remap_refs(v, m); } }
        Stream(s) => { for (_, v) in s.dict.iter_mut() { remap_refs(v, m); } }
        _ => {}
    }
}
