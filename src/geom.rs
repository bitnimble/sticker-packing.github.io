use geo::{
    Area, AffineOps, AffineTransform, BooleanOps, BoundingRect, Contains, ConvexHull, LineString,
    MultiPoint, MultiPolygon, Point, Polygon,
};
use geo_types::{coord, Coord};
use i_overlay::core::fill_rule::FillRule;
use i_overlay::float::simplify::SimplifyShape;
use std::f64::consts::PI;

pub type Poly = Polygon<f64>;
pub type Multi = MultiPolygon<f64>;

/// Page placement constraints: per-side clear borders (mm from each edge, already the larger of
/// the page margin and any registration inset) plus keep-out rectangles the placed part must not
/// overlap (registration-mark corner zones). `rects` are `[x0, y0, x1, y1]` in page coordinates.
#[derive(Clone, Default)]
pub struct Reserve {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub rects: Vec<[f64; 4]>,
}

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

// --- outward offset (auto-outline) ----------------------------------------

/// Corner treatment for an outward offset. External = convex (bulges away from the shape),
/// internal = concave (notches toward it).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum JoinStyle {
    /// Round the external corners, keep internal corners sharp (natural disk dilation).
    RoundExternal,
    /// Round every corner.
    RoundAll,
    /// Miter every corner (no rounding).
    SharpAll,
}

fn unit(x: f64, y: f64) -> (f64, f64) {
    let l = (x * x + y * y).sqrt();
    if l < 1e-12 { (0.0, 0.0) } else { (x / l, y / l) }
}

/// Intersection of lines (p1 + t*d1) and (p2 + t*d2); None if parallel.
fn line_intersect(p1: [f64; 2], d1: (f64, f64), p2: [f64; 2], d2: (f64, f64)) -> Option<[f64; 2]> {
    let denom = d1.0 * d2.1 - d1.1 * d2.0;
    if denom.abs() < 1e-12 {
        return None;
    }
    let t = ((p2[0] - p1[0]) * d2.1 - (p2[1] - p1[1]) * d2.0) / denom;
    Some([p1[0] + t * d1.0, p1[1] + t * d1.1])
}

/// Outer contour of each shape only -- a cut outline is solid, so holes (gaps enclosed where two
/// offsets merge, letter counters, etc.) are not cut.
fn shapes_to_polys(shapes: Vec<Vec<Vec<[f64; 2]>>>) -> Vec<Poly> {
    shapes
        .into_iter()
        .filter_map(|shape| {
            let outer = shape.into_iter().next()?;
            Some(Polygon::new(LineString::new(outer.into_iter().map(|p| coord! {x: p[0], y: p[1]}).collect()), vec![]))
        })
        .collect()
}

fn poly_area2(c: &[[f64; 2]]) -> f64 {
    (0..c.len()).map(|k| c[k][0] * c[(k + 1) % c.len()][1] - c[(k + 1) % c.len()][0] * c[k][1]).sum()
}

/// Orient a contour counter-clockwise (positive area) so it *adds* under a NonZero fill.
fn ccw(mut c: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    if poly_area2(&c) < 0.0 {
        c.reverse();
    }
    c
}

/// Round a ring's corners: external (convex) by `convex_r`, internal (concave) by `concave_r`
/// (each clamped per corner to half the adjacent edges). Applied to the silhouette before offsetting.
fn fillet_corners(pts: &[[f64; 2]], convex_r: f64, concave_r: f64) -> Vec<[f64; 2]> {
    let n = pts.len();
    if n < 3 || (convex_r <= 0.0 && concave_r <= 0.0) {
        return pts.to_vec();
    }
    let orient = if poly_area2(pts) >= 0.0 { 1.0 } else { -1.0 };
    let mut out: Vec<[f64; 2]> = Vec::with_capacity(n * 3);
    for i in 0..n {
        let p = pts[(i + n - 1) % n];
        let v = pts[i];
        let q = pts[(i + 1) % n];
        let din = unit(v[0] - p[0], v[1] - p[1]);
        let dout = unit(q[0] - v[0], q[1] - v[1]);
        let delta = (din.0 * dout.1 - din.1 * dout.0).atan2(din.0 * dout.0 + din.1 * dout.1);
        let r = if delta * orient > 0.0 { convex_r } else { concave_r };
        if delta.abs() < 1e-6 || r <= 0.0 {
            out.push(v); // straight, or not rounding this corner type
            continue;
        }
        let tan_half = (delta.abs() / 2.0).tan();
        let lin = ((v[0] - p[0]).powi(2) + (v[1] - p[1]).powi(2)).sqrt();
        let lout = ((q[0] - v[0]).powi(2) + (q[1] - v[1]).powi(2)).sqrt();
        let s = (r * tan_half).min(lin.min(lout) * 0.5);
        let r_eff = s / tan_half;
        let t_in = [v[0] - din.0 * s, v[1] - din.1 * s];
        let t_out = [v[0] + dout.0 * s, v[1] + dout.1 * s];
        let perp = (-din.1, din.0);
        let cand = |sgn: f64| [t_in[0] + sgn * perp.0 * r_eff, t_in[1] + sgn * perp.1 * r_eff];
        let dist_out = |c: [f64; 2]| ((c[0] - t_out[0]) * dout.1 - (c[1] - t_out[1]) * dout.0).abs();
        let (c1, c2) = (cand(1.0), cand(-1.0));
        let c = if (dist_out(c1) - r_eff).abs() <= (dist_out(c2) - r_eff).abs() { c1 } else { c2 };
        let a0 = (t_in[1] - c[1]).atan2(t_in[0] - c[0]);
        let a1 = (t_out[1] - c[1]).atan2(t_out[0] - c[0]);
        let d = a1 - a0;
        let sweep = d.sin().atan2(d.cos());
        let steps = ((delta.abs() / (PI / 16.0)).ceil() as usize).max(1);
        for k in 0..=steps {
            let ang = a0 + sweep * (k as f64 / steps as f64);
            out.push([c[0] + r_eff * ang.cos(), c[1] + r_eff * ang.sin()]);
        }
    }
    out
}


/// Outward offset of one or more silhouette rings by `m` with the given corner style. `RoundExternal`
/// is an exact disk dilation (Minkowski sum with a disk): round external corners, sharp internal
/// ones, no spikes or bevels at fine concave detail. `SharpAll`/`RoundAll` use a union of primitives
/// (the polygon, an outward quad per edge, a cap per external corner). Both union all rings, so
/// nearby elements merge where their offsets touch. Rounded corners use radius `m`.
pub fn offset_outline_multi(input: &[Vec<[f64; 2]>], m: f64, round_radius: f64, style: JoinStyle) -> Multi {
    if m <= 0.0 {
        let polys: Vec<Poly> = input.iter().filter(|r| r.len() >= 3)
            .map(|r| Polygon::new(LineString::new(r.iter().map(|p| coord! {x: p[0], y: p[1]}).collect()), vec![]))
            .collect();
        return MultiPolygon::new(polys);
    }
    if style == JoinStyle::SharpAll {
        return offset_miter(input, m);
    }
    // Round styles: pre-round the silhouette corners, then dilate with a disk (Minkowski). Convex
    // corners always round (radius >= m); internal corners stay sharp for RoundExternal, and round
    // for RoundAll (concave fillet radius chosen so the offset internal radius also comes out ~m).
    let concave_r = if style == JoinStyle::RoundAll { round_radius + 2.0 * m } else { 0.0 };
    let filleted: Vec<Vec<[f64; 2]>>;
    let rings: &[Vec<[f64; 2]>] = if round_radius > 0.0 || concave_r > 0.0 {
        filleted = input.iter().map(|r| fillet_corners(r, round_radius, concave_r)).collect();
        &filleted
    } else {
        input
    };
    let disk_pieces = convex_pieces(&disk(m, 48));
    let mut hulls: Vec<Vec<[f64; 2]>> = Vec::new();
    for r in rings {
        if r.len() < 3 {
            continue;
        }
        let poly = Polygon::new(LineString::new(r.iter().map(|p| coord! {x: p[0], y: p[1]}).collect()), vec![]);
        for pa in &convex_pieces(&poly) {
            for pb in &disk_pieces {
                let pts: Vec<Point<f64>> = pa.iter().flat_map(|a| pb.iter().map(move |b| Point::new(a.x + b.x, a.y + b.y))).collect();
                hulls.push(ccw(MultiPoint::new(pts).convex_hull().exterior().0.iter().map(|c| [c.x, c.y]).collect()));
            }
        }
    }
    if hulls.is_empty() {
        return MultiPolygon::new(vec![]);
    }
    MultiPolygon::new(shapes_to_polys(hulls.simplify_shape(FillRule::NonZero, 0.0)))
}

/// SharpAll: mitred outward offset as a union of primitives (shape + an outward quad per edge + a
/// mitre wedge per external corner). Internal corners fill out to the offset crossing, or bevel when
/// that crossing is far (shallow corner), so they never spike.
fn offset_miter(input: &[Vec<[f64; 2]>], m: f64) -> Multi {
    const MITER_LIMIT: f64 = 4.0;
    let mut contours: Vec<Vec<[f64; 2]>> = Vec::new();
    for r in input {
        let mut p = r.clone();
        if p.len() < 3 {
            continue;
        }
        let area: f64 = (0..p.len()).map(|i| p[i][0] * p[(i + 1) % p.len()][1] - p[(i + 1) % p.len()][0] * p[i][1]).sum();
        if area < 0.0 {
            p.reverse();
        }
        let n = p.len();
        let dir: Vec<(f64, f64)> = (0..n).map(|i| unit(p[(i + 1) % n][0] - p[i][0], p[(i + 1) % n][1] - p[i][1])).collect();
        let nrm: Vec<(f64, f64)> = dir.iter().map(|d| (d.1, -d.0)).collect();
        contours.push(ccw(p.clone()));
        for i in 0..n {
            let (a, b, nb) = (p[i], p[(i + 1) % n], nrm[i]);
            contours.push(ccw(vec![a, b, [b[0] + nb.0 * m, b[1] + nb.1 * m], [a[0] + nb.0 * m, a[1] + nb.1 * m]]));
        }
        for i in 0..n {
            let pe = (i + n - 1) % n;
            let v = p[i];
            let cross = dir[pe].0 * dir[i].1 - dir[pe].1 * dir[i].0;
            let a_off = [v[0] + nrm[pe].0 * m, v[1] + nrm[pe].1 * m];
            let b_off = [v[0] + nrm[i].0 * m, v[1] + nrm[i].1 * m];
            let x = line_intersect(a_off, dir[pe], b_off, dir[i]);
            let near = |x: &[f64; 2], lim: f64| ((x[0] - v[0]).powi(2) + (x[1] - v[1]).powi(2)).sqrt() <= lim * m;
            if cross > 1e-9 {
                match x.filter(|x| near(x, MITER_LIMIT)) {
                    Some(x) => contours.push(ccw(vec![v, a_off, x, b_off])),
                    None => contours.push(ccw(vec![v, a_off, b_off])),
                }
            } else if cross < -1e-9 {
                let apex = match x {
                    Some(x) if near(&x, 1.5) => x,
                    _ => [v[0], v[1]],
                };
                contours.push(ccw(vec![a_off, apex, b_off]));
            }
        }
    }
    if contours.is_empty() {
        return MultiPolygon::new(vec![]);
    }
    MultiPolygon::new(shapes_to_polys(contours.simplify_shape(FillRule::NonZero, 0.0)))
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
