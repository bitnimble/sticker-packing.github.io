use sticker_packer::geom::{poly_bbox, poly_from, rotate_p, Reserve};
use sticker_packer::{greedy, lattice};

// A placed part's bounding box, given the reference-point placement (angle in degrees, x, y).
fn placed_bbox(part: &sticker_packer::geom::Poly, angle: f64, x: f64, y: f64) -> (f64, f64, f64, f64) {
    let (minx, miny, maxx, maxy) = poly_bbox(&rotate_p(part, angle));
    (minx + x, miny + y, maxx + x, maxy + y)
}

// Positive-area intersection beyond a micron tolerance (edge-touching placements are fine).
fn rects_overlap(a: (f64, f64, f64, f64), b: [f64; 4]) -> bool {
    let eps = 1e-3;
    (a.2.min(b[2]) - a.0.max(b[0])) > eps && (a.3.min(b[3]) - a.1.max(b[1])) > eps
}

fn assert_clear(part: &sticker_packer::geom::Poly, placements: &[greedy::Placement], pw: f64, ph: f64, r: &Reserve) {
    assert!(!placements.is_empty(), "expected some placements");
    for p in placements {
        let bb = placed_bbox(part, p.angle, p.x, p.y);
        let eps = 1e-6;
        assert!(
            bb.0 >= r.left - eps && bb.1 >= r.top - eps && bb.2 <= pw - r.right + eps && bb.3 <= ph - r.bottom + eps,
            "placement {p:?} bbox {bb:?} escapes the per-side borders"
        );
        for k in &r.rects {
            assert!(!rects_overlap(bb, *k), "placement {p:?} bbox {bb:?} overlaps keep-out {k:?}");
        }
    }
}

fn cameo_reserve(pw: f64, ph: f64) -> Reserve {
    let inset = 0.625 * 25.4;
    let len = 0.787 * 25.4;
    Reserve {
        left: inset,
        top: inset,
        right: inset,
        bottom: inset,
        rects: vec![
            [inset, inset, inset + len, inset + len],
            [pw - inset - len, inset, pw - inset, inset + len],
            [inset, ph - inset - len, inset + len, ph - inset],
        ],
    }
}

#[test]
fn greedy_avoids_registration_zones() {
    let (pw, ph) = (210.0, 297.0);
    let part = poly_from(&[(0.0, 0.0), (24.0, 0.0), (24.0, 16.0), (0.0, 16.0)]);
    let rots = [0.0, 30.0, 90.0];
    let reserve = cameo_reserve(pw, ph);
    let placements = greedy::pack(&part, &rots, pw, ph, &reserve, None, 8);
    assert_clear(&part, &placements, pw, ph, &reserve);
}

#[test]
fn lattice_avoids_registration_zones() {
    let (pw, ph) = (210.0, 297.0);
    let part = poly_from(&[(0.0, 0.0), (24.0, 0.0), (24.0, 16.0), (0.0, 16.0)]);
    let rots = [0.0, 90.0, 180.0, 270.0];
    let reserve = cameo_reserve(pw, ph);
    let placements = lattice::pack(&part, &rots, pw, ph, &reserve, None);
    assert_clear(&part, &placements, pw, ph, &reserve);
}

#[test]
fn more_marks_means_no_more_stickers() {
    // Reserving corners can only reduce (never increase) the greedy count vs a plain margin.
    let (pw, ph) = (210.0, 297.0);
    let part = poly_from(&[(0.0, 0.0), (24.0, 0.0), (24.0, 16.0), (0.0, 16.0)]);
    let rots = [0.0, 45.0, 90.0];
    let plain = Reserve { left: 5.0, top: 5.0, right: 5.0, bottom: 5.0, rects: vec![] };
    let with_marks = cameo_reserve(pw, ph);
    let n_plain = greedy::pack(&part, &rots, pw, ph, &plain, None, 8).len();
    let n_marks = greedy::pack(&part, &rots, pw, ph, &with_marks, None, 8).len();
    assert!(n_marks <= n_plain, "marks reserved space but packed more ({n_marks} > {n_plain})");
}
