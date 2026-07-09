use geo::{
    Area, AffineOps, AffineTransform, BooleanOps, BoundingRect, Contains, ConvexHull, LineString,
    MultiPoint, MultiPolygon, Point, Polygon,
};
use geo_types::{coord, Coord};
use std::f64::consts::PI;

pub type Poly = Polygon<f64>;
pub type Multi = MultiPolygon<f64>;

pub fn poly_from(pts: &[(f64, f64)]) -> Poly {
    Polygon::new(
        LineString::new(pts.iter().map(|&(x, y)| coord! {x: x, y: y}).collect()),
        vec![],
    )
}

pub fn poly_bbox(p: &Poly) -> (f64, f64, f64, f64) {
    let r = p.bounding_rect().unwrap();
    (r.min().x, r.min().y, r.max().x, r.max().y)
}

/// Largest-area polygon of a (possibly multi) result. Panics on empty.
pub fn largest(m: &Multi) -> Poly {
    m.0.iter()
        .max_by(|a, b| a.unsigned_area().partial_cmp(&b.unsigned_area()).unwrap())
        .expect("empty multipolygon")
        .clone()
}

// --- affine ---------------------------------------------------------------

/// SVG-convention rotation about the origin: matrix [[cos,-sin],[sin,cos]] (positive = the
/// same visual direction as SVG rotate() in a y-down space), so output transforms match.
pub fn rot_origin(deg: f64) -> AffineTransform<f64> {
    let (s, c) = deg.to_radians().sin_cos();
    AffineTransform::new(c, -s, 0.0, s, c, 0.0)
}

pub fn rotate_m(m: &Multi, deg: f64) -> Multi {
    m.affine_transform(&rot_origin(deg))
}

pub fn rotate_p(p: &Poly, deg: f64) -> Poly {
    p.affine_transform(&rot_origin(deg))
}

pub fn translate_m(m: &Multi, dx: f64, dy: f64) -> Multi {
    m.affine_transform(&AffineTransform::translate(dx, dy))
}

// --- triangulation & Minkowski --------------------------------------------

/// A convex polygon piece (CCW vertices) -- the unit operand of the Minkowski sum.
pub type Piece = Vec<Coord<f64>>;

fn push_ring(data: &mut Vec<f64>, ls: &LineString<f64>) {
    let cs = &ls.0;
    let n = cs.len().saturating_sub(1); // drop closing duplicate
    for cpt in &cs[..n] {
        data.push(cpt.x);
        data.push(cpt.y);
    }
}

fn face_signed_area(verts: &[Coord<f64>], face: &[usize]) -> f64 {
    let n = face.len();
    (0..n).map(|i| {
        let (p, q) = (verts[face[i]], verts[face[(i + 1) % n]]);
        p.x * q.y - q.x * p.y
    }).sum::<f64>() * 0.5
}

fn is_convex(verts: &[Coord<f64>], face: &[usize]) -> bool {
    let n = face.len();
    n >= 3 && (0..n).all(|i| {
        let (a, b, c) = (verts[face[i]], verts[face[(i + 1) % n]], verts[face[(i + 2) % n]]);
        (b.x - a.x) * (c.y - b.y) - (b.y - a.y) * (c.x - b.x) >= -1e-9 // CCW: no right turns
    })
}

/// Merge two CCW faces sharing an edge into one face, iff the result stays convex.
fn try_merge(verts: &[Coord<f64>], fi: &[usize], fj: &[usize]) -> Option<Vec<usize>> {
    let (ni, nj) = (fi.len(), fj.len());
    for a in 0..ni {
        let (u, v) = (fi[a], fi[(a + 1) % ni]);
        for b in 0..nj {
            if fj[b] == v && fj[(b + 1) % nj] == u {
                // new boundary: fi from v around to u, then fj's interior (excluding u, v)
                let mut merged = Vec::with_capacity(ni + nj - 2);
                let mut k = (a + 1) % ni;
                loop {
                    merged.push(fi[k]);
                    if k == a { break; }
                    k = (k + 1) % ni;
                }
                let mut k = (b + 2) % nj;
                while k != b {
                    merged.push(fj[k]);
                    k = (k + 1) % nj;
                }
                return is_convex(verts, &merged).then_some(merged);
            }
        }
    }
    None
}

/// Earcut triangulation merged into a small set of convex polygons (Hertel-Mehlhorn). Identical
/// geometry to the triangulation, but far fewer pieces -- and the Minkowski sum is O(pieces_a *
/// pieces_b), so this is the biggest lever on packing precompute time.
pub fn convex_pieces(poly: &Poly) -> Vec<Piece> {
    let mut data: Vec<f64> = Vec::new();
    let mut holes: Vec<usize> = Vec::new();
    push_ring(&mut data, poly.exterior());
    for hole in poly.interiors() {
        holes.push(data.len() / 2);
        push_ring(&mut data, hole);
    }
    let idx = match earcutr::earcut(&data, &holes, 2) {
        Ok(v) => v,
        Err(_) => return vec![],
    };
    let verts: Vec<Coord<f64>> = (0..data.len() / 2).map(|k| coord! {x: data[2 * k], y: data[2 * k + 1]}).collect();
    let mut faces: Vec<Vec<usize>> = idx
        .chunks_exact(3)
        .map(|t| {
            let mut f = t.to_vec();
            if face_signed_area(&verts, &f) < 0.0 {
                f.reverse();
            }
            f
        })
        .collect();
    loop {
        let mut did_merge = false;
        'outer: for i in 0..faces.len() {
            for j in (i + 1)..faces.len() {
                if let Some(m) = try_merge(&verts, &faces[i], &faces[j]) {
                    faces[i] = m;
                    faces.remove(j);
                    did_merge = true;
                    break 'outer;
                }
            }
        }
        if !did_merge {
            break;
        }
    }
    faces.into_iter().map(|f| f.into_iter().map(|k| verts[k]).collect()).collect()
}

pub fn neg_pieces(pieces: &[Piece]) -> Vec<Piece> {
    pieces.iter().map(|p| p.iter().map(|c| coord! {x: -c.x, y: -c.y}).collect()).collect()
}

/// Union of many polygons via divide-and-conquer (only geo pairwise union needed).
pub fn union_all(mut polys: Vec<Multi>) -> Multi {
    if polys.is_empty() {
        return MultiPolygon::new(vec![]);
    }
    while polys.len() > 1 {
        let mut next = Vec::with_capacity(polys.len().div_ceil(2));
        let mut it = polys.into_iter();
        while let Some(a) = it.next() {
            match it.next() {
                Some(b) => next.push(a.union(&b)),
                None => next.push(a),
            }
        }
        polys = next;
    }
    polys.pop().unwrap()
}

/// Exact Minkowski sum A (+) B from triangulations: each triangle-pair contributes the convex
/// hull of the 9 vertex sums; union them all. Handles concave A and B.
pub fn minkowski(pieces_a: &[Piece], pieces_b: &[Piece]) -> Multi {
    let mut hulls: Vec<Multi> = Vec::with_capacity(pieces_a.len() * pieces_b.len());
    for pa in pieces_a {
        for pb in pieces_b {
            let mut pts = Vec::with_capacity(pa.len() * pb.len());
            for a in pa {
                for b in pb {
                    pts.push(Point::new(a.x + b.x, a.y + b.y));
                }
            }
            hulls.push(MultiPolygon::new(vec![MultiPoint::new(pts).convex_hull()]));
        }
    }
    union_all(hulls)
}

fn disk(r: f64, nseg: usize) -> Poly {
    let pts = (0..nseg)
        .map(|i| {
            let a = 2.0 * PI * i as f64 / nseg as f64;
            coord! {x: r * a.cos(), y: r * a.sin()}
        })
        .collect();
    Polygon::new(LineString::new(pts), vec![])
}

/// Round-join outward buffer as a Minkowski sum with a disk polygon (reuses the NFP path,
/// avoiding a separate offset library).
pub fn buffer(poly: &Poly, r: f64, nseg: usize) -> Poly {
    let m = minkowski(&convex_pieces(poly), &convex_pieces(&disk(r, nseg)));
    largest(&m)
}

/// Collision body K = P (+) (-P): two copies overlap iff their offset is in K's interior.
pub fn collision_body(p: &Poly) -> Multi {
    let t = convex_pieces(p);
    let nt = neg_pieces(&t);
    minkowski(&t, &nt)
}

// --- queries --------------------------------------------------------------

type Bbox = (f64, f64, f64, f64);

/// A Multi with cached bounding boxes for fast point-containment. The lattice search runs tens
/// of millions of contains queries; a bbox reject skips geo's full ray-cast for the majority
/// that fall outside the body (or a given part), for identical results.
pub struct Body {
    parts: Vec<(Poly, Bbox)>,
    bbox: Bbox,
}

impl Body {
    pub fn new(m: &Multi) -> Body {
        let parts: Vec<(Poly, Bbox)> = m.0.iter().map(|p| (p.clone(), poly_bbox(p))).collect();
        let bbox = parts.iter().fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |acc, (_, b)| {
            (acc.0.min(b.0), acc.1.min(b.1), acc.2.max(b.2), acc.3.max(b.3))
        });
        Body { parts, bbox }
    }

    pub fn contains(&self, x: f64, y: f64) -> bool {
        if x < self.bbox.0 || x > self.bbox.2 || y < self.bbox.1 || y > self.bbox.3 {
            return false;
        }
        let pt = Point::new(x, y);
        self.parts.iter().any(|(poly, b)| {
            x >= b.0 && x <= b.2 && y >= b.1 && y <= b.3 && poly.contains(&pt)
        })
    }
}

/// Vertex of `m` minimizing `key` (the fill corner/axis order), lexicographic on the pair.
pub fn extreme_vertex<F: Fn(f64, f64) -> (f64, f64)>(m: &Multi, key: F) -> Option<(f64, f64)> {
    let mut best: Option<((f64, f64), (f64, f64))> = None;
    for poly in &m.0 {
        for ring in std::iter::once(poly.exterior()).chain(poly.interiors()) {
            for cc in &ring.0 {
                let k = key(cc.x, cc.y);
                if best.as_ref().map_or(true, |(bk, _)| k < *bk) {
                    best = Some((k, (cc.x, cc.y)));
                }
            }
        }
    }
    best.map(|(_, p)| p)
}

/// n points evenly spaced by arc length along the largest polygon's exterior ring.
pub fn sample_boundary(ring: &LineString<f64>, n: usize) -> Vec<(f64, f64)> {
    let cs = &ring.0;
    let mut seglen = Vec::with_capacity(cs.len());
    let mut total = 0.0;
    for w in cs.windows(2) {
        let d = ((w[1].x - w[0].x).powi(2) + (w[1].y - w[0].y).powi(2)).sqrt();
        seglen.push(d);
        total += d;
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let mut target = total * i as f64 / n as f64;
        let mut j = 0;
        while j < seglen.len() && target > seglen[j] {
            target -= seglen[j];
            j += 1;
        }
        if j >= seglen.len() {
            out.push((cs[0].x, cs[0].y));
            continue;
        }
        let f = if seglen[j] > 0.0 { target / seglen[j] } else { 0.0 };
        out.push((
            cs[j].x + f * (cs[j + 1].x - cs[j].x),
            cs[j].y + f * (cs[j + 1].y - cs[j].y),
        ));
    }
    out
}

pub fn area_poly(p: &Poly) -> f64 {
    p.unsigned_area()
}

pub fn simplify_poly(p: &Poly, eps: f64) -> Poly {
    use geo::Simplify;
    p.simplify(&eps)
}

/// A 2x3 affine as (a,b,c,d,e,f): x' = a*x + b*y + c, y' = d*x + e*y + f.
pub type Mat = [f64; 6];

/// Compose: (p ∘ q)(v) = p(q(v)), apply q first, then p.
pub fn mat_compose(p: &Mat, q: &Mat) -> Mat {
    [
        p[0] * q[0] + p[1] * q[3],
        p[0] * q[1] + p[1] * q[4],
        p[0] * q[2] + p[1] * q[5] + p[2],
        p[3] * q[0] + p[4] * q[3],
        p[3] * q[1] + p[4] * q[4],
        p[3] * q[2] + p[4] * q[5] + p[5],
    ]
}

/// Placement transform: rotate `deg` about origin (SVG convention) then translate (tx,ty).
pub fn place_mat(deg: f64, tx: f64, ty: f64) -> Mat {
    let (s, c) = deg.to_radians().sin_cos();
    [c, -s, tx, s, c, ty]
}

/// Scale so the polygon is `target_w` wide (if given) and shift its bbox-min to the origin.
/// Returns the normalized polygon and the matrix (original coords -> normalized), which the
/// caller also applies to the *image* content so it stays aligned with the border.
pub fn normalize(p: &Poly, target_w: Option<f64>) -> (Poly, Mat) {
    let (minx, miny, maxx, _) = poly_bbox(p);
    let s = target_w.map_or(1.0, |w| w / (maxx - minx));
    let m: Mat = [s, 0.0, -s * minx, 0.0, s, -s * miny];
    let t = AffineTransform::new(m[0], m[1], m[2], m[3], m[4], m[5]);
    (p.affine_transform(&t), m)
}

/// Apply a 2x3 matrix to a polygon.
pub fn transform_poly(p: &Poly, m: &Mat) -> Poly {
    p.affine_transform(&AffineTransform::new(m[0], m[1], m[2], m[3], m[4], m[5]))
}
