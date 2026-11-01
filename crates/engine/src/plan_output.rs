//! Portable plan rendering: the same SVG scene becomes an antialiased PNG or a vector PDF.
//! Fonts are bundled, and SVG image references may only use embedded data.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use krilla::geom::{Size, Transform};
use krilla::page::PageSettings;
use krilla_svg::{SurfaceExt, SvgSettings};
use resvg::{tiny_skia, usvg};

use crate::{EngineError, Result};

const FONT: &[u8] = include_bytes!("../assets/liberation-sans/LiberationSans-Regular.ttf");
const FONT_FAMILY: &str = "Liberation Sans";
const ADDITIONAL_FONTS: &[&[u8]] = &[
    include_bytes!("../assets/liberation/LiberationSans-Bold.ttf"),
    include_bytes!("../assets/liberation/LiberationSans-Italic.ttf"),
    include_bytes!("../assets/liberation/LiberationSans-BoldItalic.ttf"),
    include_bytes!("../assets/liberation/LiberationSerif-Regular.ttf"),
    include_bytes!("../assets/liberation/LiberationSerif-Bold.ttf"),
    include_bytes!("../assets/liberation/LiberationSerif-Italic.ttf"),
    include_bytes!("../assets/liberation/LiberationSerif-BoldItalic.ttf"),
    include_bytes!("../assets/liberation/LiberationMono-Regular.ttf"),
    include_bytes!("../assets/liberation/LiberationMono-Bold.ttf"),
    include_bytes!("../assets/liberation/LiberationMono-Italic.ttf"),
    include_bytes!("../assets/liberation/LiberationMono-BoldItalic.ttf"),
];
const MAX_DIMENSION: f32 = 6000.0;
const MAX_PIXELS: u64 = 36_000_000;
const MAX_PAGES: usize = 100;

fn options(blocked: Arc<AtomicBool>) -> usvg::Options<'static> {
    let mut opt = usvg::Options {
        font_family: FONT_FAMILY.into(),
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: usvg::ImageHrefResolver::default_data_resolver(),
            resolve_string: Box::new(move |_, _| {
                blocked.store(true, Ordering::Relaxed);
                None
            }),
        },
        ..usvg::Options::default()
    };
    let fonts = opt.fontdb_mut();
    fonts.load_font_data(FONT.to_vec());
    for font in ADDITIONAL_FONTS {
        fonts.load_font_data(font.to_vec());
    }
    fonts.set_sans_serif_family(FONT_FAMILY);
    fonts.set_serif_family("Liberation Serif");
    fonts.set_monospace_family("Liberation Mono");
    opt
}

fn parse(svg: &str, opt: &usvg::Options<'_>, blocked: &AtomicBool) -> Result<usvg::Tree> {
    let tree = usvg::Tree::from_str(svg, opt).map_err(|e| EngineError::Io(format!("plan SVG: {e}")))?;
    if blocked.load(Ordering::Relaxed) {
        return Err(EngineError::Io("plan SVG images must be embedded; external image references are not allowed".into()));
    }
    let size = tree.size();
    if ![size.width(), size.height()].into_iter().all(|v| (1.0..=MAX_DIMENSION).contains(&v)) {
        return Err(EngineError::Io("plan SVG dimensions must be between 1 and 6000 pixels".into()));
    }
    let pixels = u64::from(size.width().ceil() as u32) * u64::from(size.height().ceil() as u32);
    if pixels > MAX_PIXELS {
        return Err(EngineError::Io("plan SVG exceeds the 36 million pixel limit".into()));
    }
    Ok(tree)
}

/// Parse SVG text as the drawings are parsed (bundled font, embedded images only, bounded size),
/// for live views that render it themselves (Trigon Studio's plan view).
pub fn parse_svg(svg: &str) -> Result<usvg::Tree> {
    let blocked = Arc::new(AtomicBool::new(false));
    let opt = options(Arc::clone(&blocked));
    parse(svg, &opt, &blocked)
}

/// Render the generated SVG with the bundled font. The returned bytes form a complete PNG.
pub(crate) fn png(svg: &str) -> Result<Vec<u8>> {
    let blocked = Arc::new(AtomicBool::new(false));
    let opt = options(Arc::clone(&blocked));
    let tree = parse(svg, &opt, &blocked)?;
    let size = tree.size().to_int_size();
    let mut pixels = tiny_skia::Pixmap::new(size.width(), size.height()).ok_or_else(|| EngineError::Io("cannot allocate the plan image".into()))?;
    pixels.fill(tiny_skia::Color::WHITE);
    resvg::render(&tree, tiny_skia::Transform::identity(), &mut pixels.as_mut());
    pixels.encode_png().map_err(|e| EngineError::Io(format!("encode plan PNG: {e}")))
}

/// Place each SVG on its own page, fitted and centred without changing its aspect ratio.
/// Page dimensions are physical millimetres; PDF coordinates are points (1/72 inch).
pub(crate) fn pdf(pages: &[String], page_mm: [f64; 2]) -> Result<Vec<u8>> {
    if pages.is_empty() || pages.len() > MAX_PAGES {
        return Err(EngineError::Io("a plan PDF needs between 1 and 100 pages".into()));
    }
    if !page_mm.into_iter().all(|v| v.is_finite() && (1.0..=2000.0).contains(&v)) {
        return Err(EngineError::Io("PDF page dimensions must be finite and between 1 and 2000 mm".into()));
    }
    let width = (page_mm[0] * 72.0 / 25.4) as f32;
    let height = (page_mm[1] * 72.0 / 25.4) as f32;
    let page_size = Size::from_wh(width, height).ok_or_else(|| EngineError::Io("invalid PDF page dimensions".into()))?;
    let blocked = Arc::new(AtomicBool::new(false));
    let opt = options(Arc::clone(&blocked));
    let mut document = krilla::Document::new();
    for svg in pages {
        let tree = parse(svg, &opt, &blocked)?;
        let scale = (width / tree.size().width()).min(height / tree.size().height());
        let fit = Size::from_wh(tree.size().width() * scale, tree.size().height() * scale)
            .ok_or_else(|| EngineError::Io("invalid SVG size on PDF page".into()))?;
        let mut page = document.start_page_with(PageSettings::new(page_size));
        let mut surface = page.surface();
        surface.push_transform(&Transform::from_translate((width - fit.width()) / 2.0, (height - fit.height()) / 2.0));
        let drawn = surface.draw_svg(&tree, fit, SvgSettings { embed_text: true, filter_scale: 1.0 });
        // Balance the surface before propagating errors: its destructor checks the stack.
        surface.pop();
        surface.finish();
        page.finish();
        drawn.ok_or_else(|| EngineError::Io("cannot render the SVG onto its PDF page".into()))?;
    }
    document.finish().map_err(|e| EngineError::Io(format!("encode plan PDF: {e}")))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn text_svg(text: &str) -> String {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="320" height="70"><rect width="320" height="70" fill="white"/><text x="8" y="44" font-family="Liberation Sans" font-size="32">{text}</text></svg>"#
        )
    }

    fn pixels(bytes: &[u8]) -> Vec<u8> {
        assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        let mut decoder = ::png::Decoder::new(Cursor::new(bytes)).read_info().unwrap();
        let mut pixels = vec![0; decoder.output_buffer_size().unwrap()];
        let info = decoder.next_frame(&mut pixels).unwrap();
        assert_eq!(info.color_type, ::png::ColorType::Rgba);
        pixels.truncate(info.buffer_size());
        pixels
    }

    #[test]
    fn bundled_font_preserves_case_and_renders_danish_characters() {
        let lower = pixels(&png(&text_svg("Stueetage")).unwrap());
        let upper = pixels(&png(&text_svg("STUEETAGE")).unwrap());
        assert_ne!(lower, upper, "the renderer must not uppercase labels");
        for ch in ["Æ", "Ø", "Å", "æ", "ø", "å", "²"] {
            let glyph = pixels(&png(&text_svg(ch)).unwrap());
            assert!(glyph.as_chunks::<4>().0.iter().any(|p| p[0] < 100 && p[3] == 255), "missing glyph {ch}");
            assert_ne!(glyph, pixels(&png(&text_svg("\u{fffd}")).unwrap()), "{ch} must not render as a replacement character");
        }
    }

    #[test]
    fn bundled_family_and_face_choices_render_distinct_pixels_and_embed_real_pdf_fonts() {
        let styled = |family: &str, weight: &str, style: &str| {
            format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="420" height="90"><rect width="420" height="90" fill="white"/><text x="8" y="60" font-family="{family}" font-weight="{weight}" font-style="{style}" font-size="38">Køkken ffi WWW 123</text></svg>"#
            )
        };
        let mut rendered = Vec::new();
        for family in ["Liberation Sans", "Liberation Serif", "Liberation Mono"] {
            for (weight, style) in [("normal", "normal"), ("bold", "normal"), ("normal", "italic"), ("bold", "italic")] {
                let image = pixels(&png(&styled(family, weight, style)).unwrap());
                assert!(image.as_chunks::<4>().0.iter().any(|pixel| pixel[0] < 100));
                assert!(rendered.iter().all(|previous| previous != &image), "{family}/{weight}/{style} must use its real face");
                rendered.push(image);
            }
        }
        let bytes = pdf(&[styled("Liberation Serif", "bold", "italic"), styled("Liberation Mono", "normal", "normal")], [148.0, 105.0]).unwrap();
        let document = lopdf::Document::load_mem(&bytes).unwrap();
        let names: Vec<_> = document
            .objects
            .values()
            .filter_map(|object| object.as_dict().ok())
            .filter_map(|dictionary| dictionary.get(b"BaseFont").ok()?.as_name().ok())
            .map(|name| String::from_utf8_lossy(name).into_owned())
            .collect();
        assert!(names.iter().any(|name| name.contains("LiberationSerif-BoldItalic")), "actual serif bold italic must be embedded: {names:?}");
        assert!(names.iter().any(|name| name.contains("LiberationMono")), "actual mono must be embedded: {names:?}");
        assert_eq!(document.extract_text(&[1]).unwrap().replace('\n', ""), "Køkken ffi WWW 123");
    }

    #[test]
    fn diagonal_lines_are_antialiased() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="30"><path d="M 3.2 4.1 L 36.4 23.7" fill="none" stroke="black" stroke-width="1"/></svg>"#;
        let image = pixels(&png(svg).unwrap());
        assert!(image.as_chunks::<4>().0.iter().any(|p| p[0] > 0 && p[0] < 255 && p[0] == p[1] && p[1] == p[2] && p[3] == 255));
    }

    #[test]
    fn embedded_underlay_is_kept_and_external_images_are_refused() {
        let mut underlay = tiny_skia::Pixmap::new(2, 2).unwrap();
        underlay.fill(tiny_skia::Color::from_rgba8(220, 30, 40, 255));
        let encoded = crate::drawings::base64(&underlay.encode_png().unwrap());
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"><image width="2" height="2" href="data:image/png;base64,{encoded}"/></svg>"#
        );
        assert!(pixels(&png(&svg).unwrap()).as_chunks::<4>().0.iter().all(|p| *p == [220, 30, 40, 255]));
        for href in ["/a/local/image.png", "https://example.invalid/image.png"] {
            let external =
                format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"><image width="2" height="2" href="{href}"/></svg>"#);
            assert!(png(&external).is_err());
        }
    }

    #[test]
    fn pdf_contains_two_vector_pages_and_searchable_danish_text() {
        let bytes = pdf(&[text_svg("Kælder — 12 m²"), text_svg("1. sal — Øst")], [297.0, 210.0]).unwrap();
        assert!(bytes.starts_with(b"%PDF-"));
        let doc = lopdf::Document::load_mem(&bytes).unwrap();
        let pages = doc.get_pages();
        assert_eq!(pages.len(), 2);
        for id in pages.values() {
            let page = doc.get_dictionary(*id).unwrap();
            let media = page.get(b"MediaBox").unwrap().as_array().unwrap();
            let width = media[2].as_float().unwrap();
            let height = media[3].as_float().unwrap();
            assert!((width - 297.0 * 72.0 / 25.4).abs() < 0.01);
            assert!((height - 210.0 * 72.0 / 25.4).abs() < 0.01);
            let content = doc.get_page_content(*id);
            assert!(!content.is_empty());
            let operations = lopdf::content::Content::decode(&content).unwrap().operations;
            assert!(operations.iter().any(|op| matches!(op.operator.as_str(), "Tj" | "TJ")), "text must remain PDF text");
            assert!(operations.iter().all(|op| !matches!(op.operator.as_str(), "T*" | "'" | "\"")), "these labels must not contain line moves");
        }
        let first = doc.extract_text(&[1]).unwrap();
        let second = doc.extract_text(&[2]).unwrap();
        // krilla-svg draws individually positioned glyphs. lopdf 0.46's extractor adds a
        // newline for every ET (end text object), without checking glyph positions.
        // Remove only those synthetic newlines: every character, space and accent must match.
        assert_eq!(first.replace('\n', ""), "Kælder — 12 m²");
        assert_eq!(second.replace('\n', ""), "1. sal — Øst");
        assert!(
            !doc.objects
                .values()
                .any(|o| { o.as_stream().is_ok_and(|s| s.dict.get(b"Subtype").is_ok_and(|v| v.as_name().is_ok_and(|name| name == b"Image"))) })
        );
    }

    #[test]
    fn rejects_oversized_images_and_invalid_pdf_limits() {
        for (width, height) in [(6001, 10), (10, 6001), (100_000, 100_000), (0, 10)] {
            let svg = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}"/>"#);
            assert!(png(&svg).is_err());
        }
        let pages = [text_svg("Plan")];
        for dimensions in [[0.0, 210.0], [-1.0, 210.0], [f64::NAN, 210.0], [297.0, f64::INFINITY], [2001.0, 210.0]] {
            assert!(pdf(&pages, dimensions).is_err());
        }
        assert!(pdf(&[], [297.0, 210.0]).is_err());
        assert!(pdf(&vec![pages[0].clone(); 101], [297.0, 210.0]).is_err());
    }
}
