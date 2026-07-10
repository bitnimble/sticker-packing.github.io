use clap::Parser;
use sticker_packer::engine::{run_pack, Params};
use std::time::Instant;

#[derive(Parser)]
#[command(name = "pack_stickers")]
struct Args {
    /// Border SVG: its path drives all packing math (grown by --spacing).
    #[arg(long)]
    border: String,
    /// Image SVG or raster (png/jpg/bmp/...): the artwork, clipped to the border shape.
    /// SVGs must share the border's viewBox. Defaults to the border.
    #[arg(long)]
    image: Option<String>,
    /// Output basename (default: border filename without extension).
    #[arg(long)]
    out: Option<String>,
    #[arg(long = "sticker-width")]
    sticker_width: Option<f64>,
    #[arg(long, default_value_t = 5.0)]
    margin: f64,
    #[arg(long, default_value_t = 1.5)]
    spacing: f64,
    #[arg(long, default_value = "both")]
    method: String,
    /// Page preset: a3|a4|a5|a6|letter|legal|tabloid. Overridden by --page-width/--page-height.
    #[arg(long, default_value = "a4")]
    page: String,
    /// Custom page width in mm (portrait); overrides --page.
    #[arg(long = "page-width")]
    page_width: Option<f64>,
    /// Custom page height in mm (portrait); overrides --page.
    #[arg(long = "page-height")]
    page_height: Option<f64>,
    #[arg(long, default_value_t = 4)]
    rotations: usize,
    #[arg(long)]
    landscape: bool,
    #[arg(long = "max-count")]
    max_count: Option<usize>,
    #[arg(long, default_value_t = 0.4)]
    simplify: f64,
    #[arg(long = "greedy-attempts", default_value_t = 16)]
    greedy_attempts: usize,
    #[arg(long, default_value_t = 0.1)]
    stroke: f64,
    #[arg(long = "svg-only")]
    svg_only: bool,
    #[arg(long = "reg-marks")]
    reg_marks: bool,
    #[arg(long = "reg-draw")]
    reg_draw: bool,
    #[arg(long = "reg-length", default_value_t = 0.4)]
    reg_length: f64,
    #[arg(long = "reg-thickness", default_value_t = 0.02)]
    reg_thickness: f64,
    #[arg(long = "reg-inset", default_value_t = 0.4)]
    reg_inset: f64,
}

fn write(path: &str, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|e| format!("write {path}: {e}"))?;
    println!("  {path}");
    Ok(())
}

fn run(args: Args) -> Result<(), String> {
    let base = args.out.clone().unwrap_or_else(|| {
        args.border.rsplit_once('.').map_or(args.border.clone(), |(b, _)| b.to_string())
    });
    let border_svg = std::fs::read_to_string(&args.border).map_err(|e| format!("read {}: {e}", args.border))?;
    let (image_bytes, image_ext) = match &args.image {
        Some(p) => (
            std::fs::read(p).map_err(|e| format!("read {p}: {e}"))?,
            p.rsplit_once('.').map_or(String::new(), |(_, e)| e.to_string()),
        ),
        None => (Vec::new(), String::new()),
    };
    let (preset_w, preset_h) = sticker_packer::engine::page_preset(&args.page)
        .ok_or_else(|| format!("unknown --page '{}' (a3|a4|a5|a6|letter|legal|tabloid)", args.page))?;
    let params = Params {
        sticker_width: args.sticker_width,
        page_w: args.page_width.unwrap_or(preset_w),
        page_h: args.page_height.unwrap_or(preset_h),
        margin: args.margin,
        spacing: args.spacing,
        method: args.method,
        rotations: args.rotations,
        landscape: args.landscape,
        max_count: args.max_count,
        simplify: args.simplify,
        greedy_attempts: args.greedy_attempts,
        stroke: args.stroke,
        want_pdf: !args.svg_only,
        reg_marks: args.reg_marks,
        reg_draw: args.reg_draw,
        reg_length_in: args.reg_length,
        reg_thickness_in: args.reg_thickness,
        reg_inset_l_in: args.reg_inset,
        reg_inset_t_in: args.reg_inset,
        reg_inset_r_in: args.reg_inset,
        reg_inset_b_in: args.reg_inset,
        ..Default::default()
    };

    let t0 = Instant::now();
    let out = run_pack(&border_svg, &image_bytes, &image_ext, &params, &|stage, _| eprintln!("  {stage}..."))?;
    eprintln!("packed {} in {:.2}s", out.count, t0.elapsed().as_secs_f64());

    println!("outputs:");
    write(&format!("{base}_content.svg"), out.content_svg.as_bytes())?;
    write(&format!("{base}_outline.svg"), out.outline_svg.as_bytes())?;
    if !args.svg_only {
        write(&format!("{base}_content.pdf"), &out.content_pdf)?;
        write(&format!("{base}_outline.pdf"), &out.outline_pdf)?;
    }
    Ok(())
}

fn main() {
    if let Err(e) = run(Args::parse()) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
