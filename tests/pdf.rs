#![cfg(feature = "pdf")]
// The content PDF tessellates one raster across many placements. svg2pdf expands every <use> into
// its own image XObject, so without dedup the raster is embedded once per sticker; these guard that
// identical image streams collapse back to a single shared object.
use sticker_packer::output::svg_to_pdf;

// A 4x4 opaque RGB PNG (no alpha -> no soft mask), embedded as a data URI.
const TINY_PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAQAAAAECAIAAAAmkwkpAAAAKUlEQVR4nA3HMQEAAAzCMIRVGGdFIXDLlyQSGxcTBIvjU6mt62cyOzcPp2MTQTYdST8AAAAASUVORK5CYII=";

fn count(haystack: &[u8], needle: &[u8]) -> usize {
    haystack.windows(needle.len()).filter(|w| *w == needle).count()
}

fn svg_with_images(n: usize) -> String {
    let imgs: String = (0..n)
        .map(|i| format!("<image x=\"{}\" y=\"0\" width=\"20\" height=\"20\" xlink:href=\"data:image/png;base64,{TINY_PNG_B64}\"/>", i * 25))
        .collect();
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"500\" height=\"20\" viewBox=\"0 0 500 20\">{imgs}</svg>"
    )
}

#[test]
fn identical_rasters_collapse_to_one_image() {
    let pdf = svg_to_pdf(&svg_with_images(6)).expect("pdf");
    let images = count(&pdf, b"/Subtype/Image") + count(&pdf, b"/Subtype /Image");
    assert_eq!(images, 1, "6 identical rasters should share one image XObject, found {images}");
}

#[test]
fn background_is_full_page_behind_content() {
    use sticker_packer::output::add_background;
    let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"210mm\" height=\"297mm\" viewBox=\"0 0 210 297\"><path d=\"M0,0 L1,1\"/></svg>";
    let out = add_background(svg, 210.0, 297.0);
    let rect_at = out.find("<rect").expect("rect present");
    let path_at = out.find("<path").expect("path present");
    assert!(rect_at < path_at, "background must be the first (bottom) child");
    assert!(out.contains("width=\"210\" height=\"297\"") && out.contains("fill=\"#ffffff\""));
    assert!(svg_to_pdf(&out).is_ok(), "backgrounded svg must still convert");
}

#[test]
fn dedup_shrinks_the_file() {
    // Ten copies must not cost ten images' worth of bytes.
    let one = svg_to_pdf(&svg_with_images(1)).unwrap().len();
    let ten = svg_to_pdf(&svg_with_images(10)).unwrap().len();
    assert!(ten < one * 3, "10 copies ({ten} B) ballooned vs 1 copy ({one} B) -- dedup not applied");
}
