use crate::engine::{run_pack, Params};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct PackResult {
    count: usize,
    content_svg: String,
    outline_svg: String,
    content_pdf: Vec<u8>,
    outline_pdf: Vec<u8>,
}

#[wasm_bindgen]
impl PackResult {
    #[wasm_bindgen(getter)]
    pub fn count(&self) -> usize {
        self.count
    }
    #[wasm_bindgen(getter)]
    pub fn content_svg(&self) -> String {
        self.content_svg.clone()
    }
    #[wasm_bindgen(getter)]
    pub fn outline_svg(&self) -> String {
        self.outline_svg.clone()
    }
    #[wasm_bindgen(getter)]
    pub fn content_pdf(&self) -> Vec<u8> {
        self.content_pdf.clone()
    }
    #[wasm_bindgen(getter)]
    pub fn outline_pdf(&self) -> Vec<u8> {
        self.outline_pdf.clone()
    }
}

/// Run the full pack. `image_bytes` empty + `image_ext` "" => single-SVG mode (border is art).
/// `sticker_width <= 0` => none; `max_count < 0` => none.
#[allow(clippy::too_many_arguments)]
#[wasm_bindgen]
pub fn pack(
    border_svg: String,
    image_bytes: Vec<u8>,
    image_ext: String,
    sticker_width: f64,
    page_w: f64,
    page_h: f64,
    margin: f64,
    spacing: f64,
    method: String,
    rotations: u32,
    landscape: bool,
    max_count: i32,
    simplify: f64,
    greedy_attempts: u32,
    stroke: f64,
    want_pdf: bool,
) -> Result<PackResult, JsValue> {
    let p = Params {
        sticker_width: (sticker_width > 0.0).then_some(sticker_width),
        page_w,
        page_h,
        margin,
        spacing,
        method,
        rotations: rotations as usize,
        landscape,
        max_count: (max_count >= 0).then_some(max_count as usize),
        simplify,
        greedy_attempts: greedy_attempts as usize,
        stroke,
        want_pdf,
    };
    let out = run_pack(&border_svg, &image_bytes, &image_ext, &p).map_err(|e| JsValue::from_str(&e))?;
    Ok(PackResult {
        count: out.count,
        content_svg: out.content_svg,
        outline_svg: out.outline_svg,
        content_pdf: out.content_pdf,
        outline_pdf: out.outline_pdf,
    })
}
