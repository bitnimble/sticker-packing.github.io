// Debug helper: render an SVG file to PNG at a given pixel width. `render <in.svg> <out.png> [w]`.
use resvg::tiny_skia;
use resvg::usvg;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let inp = &args[1];
    let outp = &args[2];
    let target_w: f32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1000.0);

    let data = std::fs::read(inp).expect("read svg");
    let tree = usvg::Tree::from_data(&data, &usvg::Options::default()).expect("parse svg");
    let size = tree.size();
    let scale = target_w / size.width();
    let (pw, ph) = ((size.width() * scale).ceil() as u32, (size.height() * scale).ceil() as u32);

    let mut pixmap = tiny_skia::Pixmap::new(pw, ph).expect("pixmap");
    pixmap.fill(tiny_skia::Color::WHITE);
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    pixmap.save_png(outp).expect("save png");
    println!("rendered {inp} -> {outp} ({pw}x{ph})");
}
