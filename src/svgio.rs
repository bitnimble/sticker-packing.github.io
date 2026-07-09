use crate::geom::*;
use usvg::tiny_skia_path::PathSegment;
use usvg::{Node, Transform};

const CURVE_STEPS: usize = 16;

fn map(t: &Transform, x: f32, y: f32) -> (f64, f64) {
    (
        (t.sx * x + t.kx * y + t.tx) as f64,
        (t.ky * x + t.sy * y + t.ty) as f64,
    )
}

fn flatten(path: &usvg::Path, rings: &mut Vec<Vec<(f64, f64)>>) {
    let t = path.abs_transform();
    let mut cur: Vec<(f64, f64)> = Vec::new();
    let mut last = (0.0f64, 0.0f64);
    let flush = |cur: &mut Vec<(f64, f64)>, rings: &mut Vec<Vec<(f64, f64)>>| {
        if cur.len() >= 3 {
            rings.push(std::mem::take(cur));
        } else {
            cur.clear();
        }
    };
    for seg in path.data().segments() {
        match seg {
            PathSegment::MoveTo(p) => {
                flush(&mut cur, rings);
                last = map(&t, p.x, p.y);
                cur.push(last);
            }
            PathSegment::LineTo(p) => {
                last = map(&t, p.x, p.y);
                cur.push(last);
            }
            PathSegment::QuadTo(a, b) => {
                let a = map(&t, a.x, a.y);
                let b = map(&t, b.x, b.y);
                for s in 1..=CURVE_STEPS {
                    let u = s as f64 / CURVE_STEPS as f64;
                    let m = 1.0 - u;
                    cur.push((
                        m * m * last.0 + 2.0 * m * u * a.0 + u * u * b.0,
                        m * m * last.1 + 2.0 * m * u * a.1 + u * u * b.1,
                    ));
                }
                last = b;
            }
            PathSegment::CubicTo(a, b, c) => {
                let a = map(&t, a.x, a.y);
                let b = map(&t, b.x, b.y);
                let c = map(&t, c.x, c.y);
                for s in 1..=CURVE_STEPS {
                    let u = s as f64 / CURVE_STEPS as f64;
                    let m = 1.0 - u;
                    cur.push((
                        m * m * m * last.0 + 3.0 * m * m * u * a.0 + 3.0 * m * u * u * b.0 + u * u * u * c.0,
                        m * m * m * last.1 + 3.0 * m * m * u * a.1 + 3.0 * m * u * u * b.1 + u * u * u * c.1,
                    ));
                }
                last = c;
            }
            PathSegment::Close => flush(&mut cur, rings),
        }
    }
    flush(&mut cur, rings);
}

fn walk(group: &usvg::Group, rings: &mut Vec<Vec<(f64, f64)>>) {
    for node in group.children() {
        match node {
            Node::Group(g) => walk(g, rings),
            Node::Path(p) => flatten(p, rings),
            _ => {}
        }
    }
}

/// The `viewBox="minx miny w h"` of an SVG's root element (the shared coordinate frame).
/// usvg normalizes coordinates after parsing, so read it straight from the XML.
pub fn read_viewbox_str(s: &str) -> Result<[f64; 4], String> {
    let lower = s.to_ascii_lowercase();
    let start = lower.find("<svg").ok_or("no <svg> element")?;
    let end = s[start..].find('>').map(|i| start + i).ok_or("malformed <svg>")?;
    let tag = &s[start..end];
    let vb = tag.to_ascii_lowercase().find("viewbox").ok_or("SVG has no viewBox attribute (both SVGs need one to align)")?;
    let after = tag[vb..].splitn(2, '=').nth(1).ok_or("malformed viewBox")?;
    let q = after.trim_start();
    let quote = q.chars().next().filter(|c| *c == '"' || *c == '\'').ok_or("malformed viewBox")?;
    let body = &q[1..];
    let inner = &body[..body.find(quote).ok_or("unterminated viewBox")?];
    let nums: Result<Vec<f64>, _> = inner
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
        .map(str::parse::<f64>)
        .collect();
    match nums.map_err(|_| "non-numeric viewBox".to_string())?.as_slice() {
        [a, b, c, d] => Ok([*a, *b, *c, *d]),
        _ => Err("viewBox must have 4 numbers".into()),
    }
}

/// Border and image must share a viewBox (same coordinate space) or their relative alignment
/// is undefined. Returns the shared viewBox; errors on mismatch.
pub fn require_same_viewbox_str(border: &str, image: &str) -> Result<[f64; 4], String> {
    let a = read_viewbox_str(border).map_err(|e| format!("border: {e}"))?;
    let b = read_viewbox_str(image).map_err(|e| format!("image: {e}"))?;
    if a.iter().zip(&b).all(|(x, y)| (x - y).abs() <= 1e-4 * x.abs().max(y.abs()).max(1.0)) {
        Ok(a)
    } else {
        Err(format!(
            "viewBox mismatch: border [{}], image [{}] -- both SVGs must share the same viewBox/coordinate space so the image aligns to the border",
            a.map(|v| v.to_string()).join(" "),
            b.map(|v| v.to_string()).join(" "),
        ))
    }
}

/// Load an SVG's filled outline as a single polygon in viewBox USER-UNITS: flatten all
/// subpaths, undo any usvg size-scaling (so raw-XML content in the same viewBox aligns), union,
/// take the largest component. Used for the BORDER (drives packing).
pub fn load_outline_str(svg: &str) -> Result<Poly, String> {
    let tree = usvg::Tree::from_data(svg.as_bytes(), &usvg::Options::default())
        .map_err(|e| format!("parse SVG: {e}"))?;
    let vb = read_viewbox_str(svg)?;
    // usvg maps viewBox -> its size box; recover viewBox units: vb = usvg/scale + vb_min.
    let scale = tree.size().width() as f64 / vb[2];
    let to_vb = |(x, y): (f64, f64)| (x / scale + vb[0], y / scale + vb[1]);

    let mut rings: Vec<Vec<(f64, f64)>> = Vec::new();
    walk(tree.root(), &mut rings);
    let polys: Vec<Multi> = rings
        .iter()
        .filter(|r| r.len() >= 3)
        .map(|r| Multi::new(vec![poly_from(&r.iter().map(|&p| to_vb(p)).collect::<Vec<_>>())]))
        .collect();
    if polys.is_empty() {
        return Err("no closed paths in border SVG".into());
    }
    Ok(largest(&union_all(polys)))
}

/// The raw inner markup of an SVG (everything between the root `<svg ...>` and `</svg>`), in
/// viewBox user-units. Inlined verbatim into an output group so all artwork fidelity
/// (gradients, clips, rasters) is preserved.
pub fn load_inner_svg_str(s: &str) -> Result<String, String> {
    let lower = s.to_ascii_lowercase();
    let start = lower.find("<svg").ok_or("no <svg>")?;
    let open_end = s[start..].find('>').map(|i| start + i + 1).ok_or("malformed <svg>")?;
    let close = lower.rfind("</svg>").ok_or("no </svg>")?;
    Ok(s[open_end..close].trim().to_string())
}

/// File-reading wrappers (native CLI).
pub fn load_outline(path: &str) -> Result<Poly, String> {
    load_outline_str(&std::fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?)
}
