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

/// Content sheet where each sticker is a single pre-clipped raster (the outline baked into the
/// image's alpha), placed by transform with no vector clip path. Silhouette imports each placement
/// as one raster, instead of exploding a clip-path + image + soft-mask into stacked polygons.
pub fn content_svg_baked(href: &str, vb: &[f64; 4], norm: &Mat, placements: &[Placement], pw: f64, ph: f64) -> String {
    let mut s = header(pw, ph);
    s.push_str(&format!(
        "<defs><image id=\"sticker\" xlink:href=\"{}\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" preserveAspectRatio=\"none\"/></defs>\n",
        href, vb[0], vb[1], vb[2], vb[3]
    ));
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

/// Prepend a full-page white rectangle as the first (bottom) child of an output SVG. Silhouette
/// imports a PDF at the bounding box of its vector content, so without a page-sized shape the
/// content and outline PDFs import at different bounds and don't line up; a shared background makes
/// both import at the full document size and position. The user deletes it after importing.
pub fn add_background(svg: &str, pw: f64, ph: f64) -> String {
    let rect = format!("<rect x=\"0\" y=\"0\" width=\"{pw}\" height=\"{ph}\" fill=\"#ffffff\"/>");
    // The opening `<svg ...>` tag has no '>' inside its quoted attributes, so the first '>' ends it.
    match svg.find('>') {
        Some(i) => format!("{}{}{}", &svg[..=i], rect, &svg[i + 1..]),
        None => svg.to_string(),
    }
}

/// Bake the border outline into the art raster's alpha: decode `image_bytes`, zero the alpha of
/// every pixel outside the outline (mapped from viewBox units to the art's pixel grid), re-encode
/// as PNG. The art is assumed to span the whole viewBox 1:1. Result is a single sticker-shaped
/// image that needs no clip path.
#[cfg(feature = "pdf")]
pub fn bake_clipped_png(image_bytes: &[u8], outline: &Poly, vb: &[f64; 4]) -> Result<Vec<u8>, String> {
    let src = image::load_from_memory(image_bytes).map_err(|e| format!("decode art: {e}"))?;
    let (w, h) = (src.width(), src.height());
    let mut buf = src.to_rgba8().into_raw();
    let mask = rasterize_mask(outline, vb, w as usize, h as usize);
    for i in 0..(w * h) as usize {
        buf[i * 4 + 3] = (buf[i * 4 + 3] as u32 * mask[i] as u32 / 255) as u8;
    }
    // Bleed opaque colours into the transparent margin so a downscaling renderer blends the edge
    // colour across the sticker boundary rather than a fringe of the arbitrary transparent-pixel RGB.
    bleed_edges(&mut buf, w as usize, h as usize);
    let img = image::RgbaImage::from_raw(w, h, buf).ok_or("rebuild sticker image")?;
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| format!("encode sticker png: {e}"))?;
    Ok(out.into_inner())
}

/// Copy the RGB of the nearest opaque pixel outward into transparent pixels (bounded flood; alpha
/// stays 0). With unpremultiplied alpha, a downscaling renderer averages neighbouring RGB, so the
/// transparent margin must carry the edge colour or it fringes the sticker outline.
#[cfg(feature = "pdf")]
fn bleed_edges(buf: &mut [u8], w: usize, h: usize) {
    let n = w * h;
    let mut known: Vec<bool> = (0..n).map(|i| buf[i * 4 + 3] > 0).collect();
    let mut frontier: Vec<usize> = (0..n)
        .filter(|&i| {
            known[i]
                && ((i % w > 0 && !known[i - 1]) || (i % w < w - 1 && !known[i + 1])
                    || (i >= w && !known[i - w]) || (i + w < n && !known[i + w]))
        })
        .collect();
    for _ in 0..32 {
        if frontier.is_empty() {
            break;
        }
        let mut next = Vec::new();
        for &i in &frontier {
            let (r, g, b) = (buf[i * 4], buf[i * 4 + 1], buf[i * 4 + 2]);
            let (x, y) = (i % w, i / w);
            let mut nbrs = [0usize; 4];
            let mut c = 0;
            if x > 0 { nbrs[c] = i - 1; c += 1; }
            if x < w - 1 { nbrs[c] = i + 1; c += 1; }
            if y > 0 { nbrs[c] = i - w; c += 1; }
            if y < h - 1 { nbrs[c] = i + w; c += 1; }
            for &j in &nbrs[..c] {
                if !known[j] {
                    known[j] = true;
                    buf[j * 4] = r;
                    buf[j * 4 + 1] = g;
                    buf[j * 4 + 2] = b;
                    next.push(j);
                }
            }
        }
        frontier = next;
    }
}

/// Even-odd scanline fill of a polygon (exterior + holes) into a `w*h` 0/255 mask.
#[cfg(feature = "pdf")]
fn rasterize_mask(outline: &Poly, vb: &[f64; 4], w: usize, h: usize) -> Vec<u8> {
    let mut mask = vec![0u8; w * h];
    let (sx, sy) = (w as f64 / vb[2], h as f64 / vb[3]);
    let mut edges: Vec<(f64, f64, f64, f64)> = Vec::new();
    for ring in std::iter::once(outline.exterior()).chain(outline.interiors()) {
        let pts: Vec<(f64, f64)> = ring.0.iter().map(|c| ((c.x - vb[0]) * sx, (c.y - vb[1]) * sy)).collect();
        for e in pts.windows(2) {
            edges.push((e[0].0, e[0].1, e[1].0, e[1].1));
        }
    }
    let mut xs: Vec<f64> = Vec::new();
    for y in 0..h {
        let yc = y as f64 + 0.5;
        xs.clear();
        for &(x0, y0, x1, y1) in &edges {
            let (lo, hi) = if y0 < y1 { (y0, y1) } else { (y1, y0) };
            if yc >= lo && yc < hi {
                xs.push(x0 + (yc - y0) / (y1 - y0) * (x1 - x0));
            }
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mut k = 0;
        while k + 1 < xs.len() {
            let a = (xs[k] - 0.5).ceil().max(0.0) as usize;
            let bf = (xs[k + 1] - 0.5).floor();
            if bf >= 0.0 {
                let b = (bf as usize).min(w - 1);
                for x in a..=b {
                    mask[y * w + x] = 255;
                }
            }
            k += 2;
        }
    }
    mask
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
                    // Confirm true equality, not just a hash collision, before merging.
                    Some(&keep) if doc.objects.get(&keep) == Some(obj) => { remap.insert(id, keep); }
                    Some(_) => {}
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
