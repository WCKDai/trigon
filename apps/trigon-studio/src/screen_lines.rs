//! Screen-only pen widths. Paper exports and semantic picking keep their original geometry.
//!
//! `zoom` means logical window points per original SVG pixel, independently of Retina density.
//! Fine and cut pens retain their order while approaching different high-zoom limits. Dash/gap
//! cadence follows the pen's screen width; the original paper cadence stays in the source SVG.
//! This is a bounded tokenizer for Studio-generated SVG, not a sanitizer for imported SVG files.

const MIN_ZOOM: f32 = 0.02;
const MAX_ZOOM: f32 = 200.0;
const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_DEPTH: usize = 128;
const MAX_ATTRIBUTES: usize = 64;
const MAX_DASHES: usize = 32;

/// Adapt explicit drawn path strokes for the canvas, leaving the supplied source untouched.
///
/// At 1× the result is byte-identical. Defs, patterns, text and its halos, fill-coloured seam
/// covers, CSS-styled paths and inherited strokes are not changed. Coordinates, transforms,
/// clipping, colour, opacity, line caps and joins are copied exactly. Target attributes must be
/// finite unitless numbers, as emitted by the drawing writer; malformed tags or target numbers
/// fail atomically. The subsequent SVG parser remains responsible for full SVG validation.
pub(crate) fn svg_at_zoom(source: &str, zoom: f32) -> Result<String, String> {
    if !zoom.is_finite() || !(MIN_ZOOM..=MAX_ZOOM).contains(&zoom) {
        return Err("screen line zoom must be finite and within 0.02..200".into());
    }
    if source.len() > MAX_BYTES {
        return Err("screen drawing exceeds the 64 MiB source limit".into());
    }
    let mut out = String::with_capacity(source.len());
    let mut stack: Vec<(&str, bool)> = Vec::new();
    let mut cursor = 0;
    let mut root_seen = false;
    let mut root_closed = false;
    while let Some(relative) = source[cursor..].find('<') {
        let start = cursor + relative;
        let text = &source[cursor..start];
        check_text(text, stack.is_empty())?;
        append(&mut out, text)?;
        let rest = &source[start..];
        if let Some(comment) = rest.strip_prefix("<!--") {
            let Some(end) = comment.find("-->") else {
                return Err("unterminated SVG comment".into());
            };
            if comment[..end].contains("--") || comment[..end].ends_with('-') {
                return Err("malformed SVG comment".into());
            }
            cursor = start + 4 + end + 3;
            append(&mut out, &source[start..cursor])?;
            continue;
        }
        if let Some(cdata) = rest.strip_prefix("<![CDATA[") {
            if stack.is_empty() {
                return Err("SVG CDATA must be inside the root".into());
            }
            let Some(end) = cdata.find("]]>") else {
                return Err("unterminated SVG CDATA".into());
            };
            cursor = start + 9 + end + 3;
            append(&mut out, &source[start..cursor])?;
            continue;
        }
        if rest.starts_with("<?") {
            if root_seen || !rest.starts_with("<?xml ") {
                return Err("unsupported SVG processing instruction".into());
            }
            let Some(end) = rest.find("?>") else {
                return Err("unterminated SVG declaration".into());
            };
            cursor = start + end + 2;
            append(&mut out, &source[start..cursor])?;
            continue;
        }
        if rest.starts_with("<!") {
            return Err("SVG declarations and external entities are not supported".into());
        }
        let tag = tag_at(source, start)?;
        if tag.closing {
            let Some((name, _)) = stack.pop() else {
                return Err("unexpected SVG closing tag".into());
            };
            if name != tag.name {
                return Err("mismatched SVG closing tag".into());
            }
            if stack.is_empty() {
                root_closed = true;
            }
            append(&mut out, &source[start..tag.end])?;
        } else {
            if stack.is_empty() {
                if root_seen || root_closed || tag.name != "svg" {
                    return Err("screen drawing must have one SVG root".into());
                }
                root_seen = true;
            }
            let blocked = stack.last().is_some_and(|(_, blocked)| *blocked);
            if tag.name == "path" && !blocked {
                append_path(&mut out, source, start, &tag, f64::from(zoom))?;
            } else {
                append(&mut out, &source[start..tag.end])?;
            }
            if !tag.self_closing {
                if stack.len() >= MAX_DEPTH {
                    return Err("screen drawing exceeds the SVG nesting limit".into());
                }
                stack.push((tag.name, blocked || !matches!(tag.name, "svg" | "g")));
            } else if stack.is_empty() {
                root_closed = true;
            }
        }
        cursor = tag.end;
    }
    check_text(&source[cursor..], stack.is_empty())?;
    append(&mut out, &source[cursor..])?;
    if !root_seen || !root_closed || !stack.is_empty() {
        return Err("screen drawing has an incomplete SVG root".into());
    }
    Ok(out)
}

fn check_text(text: &str, outside: bool) -> Result<(), String> {
    if text.contains("]]>") || text.as_bytes().contains(&0) || (outside && !text.trim().is_empty()) {
        return Err("malformed SVG text".into());
    }
    Ok(())
}

fn append(out: &mut String, value: &str) -> Result<(), String> {
    if value.len() > MAX_BYTES.saturating_sub(out.len()) {
        return Err("adapted screen drawing exceeds the 64 MiB limit".into());
    }
    out.push_str(value);
    Ok(())
}

struct Attribute<'a> {
    name: &'a str,
    start: usize,
    end: usize,
}

struct Tag<'a> {
    name: &'a str,
    attributes: Vec<Attribute<'a>>,
    end: usize,
    closing: bool,
    self_closing: bool,
}

fn name_end(bytes: &[u8], start: usize) -> Result<usize, String> {
    if !bytes.get(start).is_some_and(|b| b.is_ascii_alphabetic() || matches!(*b, b'_' | b':')) {
        return Err("malformed SVG element or attribute name".into());
    }
    let mut end = start + 1;
    while bytes.get(end).is_some_and(|b| b.is_ascii_alphanumeric() || matches!(*b, b'_' | b':' | b'.' | b'-')) {
        end += 1;
    }
    Ok(end)
}

fn whitespace(bytes: &[u8], cursor: &mut usize) {
    while bytes.get(*cursor).is_some_and(u8::is_ascii_whitespace) {
        *cursor += 1;
    }
}

fn tag_at(source: &str, start: usize) -> Result<Tag<'_>, String> {
    let bytes = source.as_bytes();
    let closing = bytes.get(start + 1) == Some(&b'/');
    let name_start = start + if closing { 2 } else { 1 };
    let mut cursor = name_end(bytes, name_start)?;
    let name = &source[name_start..cursor];
    let mut attributes: Vec<Attribute<'_>> = Vec::new();
    loop {
        let before = cursor;
        whitespace(bytes, &mut cursor);
        let separated = cursor != before;
        if bytes.get(cursor) == Some(&b'>') {
            return Ok(Tag { name, attributes, end: cursor + 1, closing, self_closing: false });
        }
        if !closing && bytes.get(cursor..cursor + 2) == Some(b"/>") {
            return Ok(Tag { name, attributes, end: cursor + 2, closing, self_closing: true });
        }
        if closing || !separated || attributes.len() >= MAX_ATTRIBUTES {
            return Err("malformed SVG tag or too many attributes".into());
        }
        let attr_start = cursor;
        cursor = name_end(bytes, cursor)?;
        let attr_name = &source[attr_start..cursor];
        if attributes.iter().any(|attr| attr.name == attr_name) {
            return Err("duplicate SVG attribute".into());
        }
        whitespace(bytes, &mut cursor);
        if bytes.get(cursor) != Some(&b'=') {
            return Err("SVG attribute has no value".into());
        }
        cursor += 1;
        whitespace(bytes, &mut cursor);
        let Some(&quote @ (b'\'' | b'"')) = bytes.get(cursor) else {
            return Err("SVG attribute value must be quoted".into());
        };
        cursor += 1;
        let value_start = cursor;
        while let Some(&b) = bytes.get(cursor) {
            if b == quote {
                break;
            }
            if matches!(b, b'<' | 0) {
                return Err("malformed SVG attribute value".into());
            }
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&quote) {
            return Err("unterminated SVG attribute value".into());
        }
        attributes.push(Attribute { name: attr_name, start: value_start, end: cursor });
        cursor += 1;
    }
}

fn value<'a>(source: &'a str, tag: &Tag<'_>, name: &str) -> Option<&'a str> {
    tag.attributes.iter().find(|attr| attr.name == name).map(|attr| &source[attr.start..attr.end])
}

fn finite_number(value: &str, minimum: f64, maximum: f64) -> Result<f64, String> {
    if value.len() > 64 {
        return Err("screen stroke number exceeds the token limit".into());
    }
    let number = value.trim().parse::<f64>().map_err(|_| "screen stroke requires a unitless number".to_owned())?;
    if !number.is_finite() || !(minimum..=maximum).contains(&number) {
        return Err("screen stroke number is outside its finite range".into());
    }
    Ok(number)
}

/// Logical point width, with a distinct monotone asymptote for each source pen.
fn screen_width(width: f64, zoom: f64) -> f64 {
    if width == 0.0 {
        0.0
    } else if zoom < 1.0 {
        (width * zoom.powf(0.55)).max(width.min(0.14 + 0.2 * width.sqrt()))
    } else {
        let ceiling = width.max(1.0 + 1.4 * width.sqrt());
        width + (ceiling - width) * (1.0 - zoom.powf(-0.35))
    }
}

fn dash_values(value: &str, factor: f64) -> Result<String, String> {
    if value.trim() == "none" {
        return Ok(value.into());
    }
    let bytes = value.as_bytes();
    let mut cursor = 0;
    let mut out = String::new();
    let mut count = 0;
    whitespace(bytes, &mut cursor);
    while cursor < bytes.len() {
        let start = cursor;
        while bytes.get(cursor).is_some_and(|b| !b.is_ascii_whitespace() && *b != b',') {
            cursor += 1;
        }
        if cursor == start || count >= MAX_DASHES {
            return Err("malformed or excessive SVG dash array".into());
        }
        let number = finite_number(&value[start..cursor], 0.0, 1_000_000.0)?;
        if count > 0 {
            out.push(' ');
        }
        let scaled = number * factor;
        if !scaled.is_finite() {
            return Err("adapted SVG dash exceeds the finite range".into());
        }
        out.push_str(&scaled.to_string());
        count += 1;
        whitespace(bytes, &mut cursor);
        if bytes.get(cursor) == Some(&b',') {
            cursor += 1;
            whitespace(bytes, &mut cursor);
            if cursor == bytes.len() {
                return Err("trailing SVG dash separator".into());
            }
        }
    }
    if count == 0 {
        return Err("empty SVG dash array".into());
    }
    Ok(out)
}

fn append_path(out: &mut String, source: &str, start: usize, tag: &Tag<'_>, zoom: f64) -> Result<(), String> {
    let Some(stroke) = value(source, tag, "stroke") else {
        return append(out, &source[start..tag.end]);
    };
    let Some(raw_width) = value(source, tag, "stroke-width") else {
        return append(out, &source[start..tag.end]);
    };
    // Solid fill seams deliberately retain the fill's physical cover, not the outline policy.
    if stroke.trim() == "none"
        || value(source, tag, "style").is_some()
        || value(source, tag, "fill").is_some_and(|fill| fill.trim().eq_ignore_ascii_case(stroke.trim()))
    {
        return append(out, &source[start..tag.end]);
    }
    let width = finite_number(raw_width, 0.0, 4096.0)?;
    let factor = if width > 0.0 { screen_width(width, zoom) / width / zoom } else { 1.0 };
    if !factor.is_finite() {
        return Err("screen stroke ratio exceeds the finite range".into());
    }
    // Validate targeted attributes even at 1×, but retain their exact source formatting there.
    let mut replacements: Vec<(usize, usize, String)> = Vec::new();
    for attr in &tag.attributes {
        let raw = &source[attr.start..attr.end];
        let replacement = match attr.name {
            "stroke-width" => Some((width * factor).to_string()),
            "stroke-dasharray" => Some(dash_values(raw, factor)?),
            "stroke-dashoffset" => {
                let offset = finite_number(raw, -1_000_000.0, 1_000_000.0)? * factor;
                if !offset.is_finite() {
                    return Err("adapted SVG dash offset exceeds the finite range".into());
                }
                Some(offset.to_string())
            }
            _ => None,
        };
        if let Some(replacement) = replacement
            && zoom != 1.0
            && width > 0.0
        {
            replacements.push((attr.start, attr.end, replacement));
        }
    }
    let mut cursor = start;
    for (from, to, replacement) in replacements {
        append(out, &source[cursor..from])?;
        append(out, &replacement)?;
        cursor = to;
    }
    append(out, &source[cursor..tag.end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use trigon_engine::resvg::{self, tiny_skia};

    const PROTECTED: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="256" height="256">
<defs><pattern id="masonry"><path d="M0 0L7 7" fill="none" stroke="#777" stroke-width="0.65"/></pattern></defs>
<path d="M0 0L1 0L1 1Z" fill="#bbaa99" stroke="#bbaa99" stroke-width="0.3"/>
<path d="M2 0L3 0L3 1Z" fill="#aabbcc" stroke="#AABBCC" stroke-width="0.6"/>
<path d="M1 2L3 4" fill="none" stroke="#555" stroke-width="0.5" style="stroke-width:0.8"/>
<g><text x="4" y="20" stroke="white" stroke-width="4">stroke-width="2.4" &lt;path stroke-width='99'/&gt;<![CDATA[<path stroke-width="77"/>]]></text></g>
<!-- <path d="M1 2" stroke="black" stroke-width="99"/> -->
</svg>"##;

    fn drawing(body: &str) -> String {
        format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="256" height="256" viewBox="0 0 256 256">{body}</svg>"#)
    }

    fn line(width: f64) -> String {
        drawing(&format!(r#"<path d="M128.5 0L128.5 256" fill="none" stroke="black" stroke-width="{width}"/>"#))
    }

    fn render_at_phase(tree: &trigon_engine::resvg::usvg::Tree, zoom: f32, ppp: u32, physical_phase: f32) -> tiny_skia::Pixmap {
        let mut pixels = tiny_skia::Pixmap::new(128 * ppp, 128 * ppp).unwrap();
        pixels.fill(tiny_skia::Color::WHITE);
        let density = ppp as f32;
        let scale = zoom * density;
        let transform =
            tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, 64.0 * density - 128.5 * scale + physical_phase, 64.0 * density - 128.0 * scale);
        resvg::render(tree, transform, &mut pixels.as_mut());
        pixels
    }

    fn render(source: &str, zoom: f32, ppp: u32) -> tiny_skia::Pixmap {
        let tree = trigon_engine::parse_svg(source).unwrap();
        render_at_phase(&tree, zoom, ppp, 0.0)
    }

    fn phase_ink(source: &str, zoom: f32, ppp: u32) -> [f64; 32] {
        let tree = trigon_engine::parse_svg(source).unwrap();
        // Tiny-skia's filled-stroke AA uses a 4x subpixel lattice. Cover a whole physical
        // pixel using midpoint samples, so edge rounding cannot select the measured width.
        std::array::from_fn(|phase| {
            let pixels = render_at_phase(&tree, zoom, ppp, (phase as f32 + 0.5) / 32.0);
            ink_width(&pixels, ppp)
        })
    }

    fn mean_ink(source: &str, zoom: f32, ppp: u32) -> f64 {
        phase_ink(source, zoom, ppp).iter().sum::<f64>() / 32.0
    }

    fn pixel(pixels: &tiny_skia::Pixmap, x: u32, y: u32) -> [u8; 4] {
        let offset = (y as usize * pixels.width() as usize + x as usize) * 4;
        pixels.data()[offset..offset + 4].try_into().unwrap()
    }

    fn ink_width(pixels: &tiny_skia::Pixmap, ppp: u32) -> f64 {
        (0..pixels.width()).map(|x| 1.0 - f64::from(pixel(pixels, x, 64 * ppp)[0]) / 255.0).sum::<f64>() / f64::from(ppp)
    }

    #[test]
    fn source_is_exact_at_one_and_protected_content_never_changes() {
        let active = r##"<path data-note="literal stroke-width='100' > end" d="M1 2L3 4" transform="translate(5 6)" fill="none" stroke="#c02020" stroke-opacity="0.45" opacity="0.8" stroke-width="0.850" stroke-linecap="round" stroke-linejoin="round" stroke-dasharray="0 4.724" stroke-dashoffset="-2.0"/>"##;
        let mixed = PROTECTED.replacen("</svg>", &format!("{active}</svg>"), 1);
        assert_eq!(svg_at_zoom(&mixed, 1.0).unwrap(), mixed);
        for zoom in [0.02, 0.5, 4.0, 8.0, 200.0] {
            assert_eq!(svg_at_zoom(PROTECTED, zoom).unwrap(), PROTECTED);
            let styled = svg_at_zoom(&mixed, zoom).unwrap();
            assert!(styled.starts_with(PROTECTED.strip_suffix("</svg>").unwrap()));
            for untouched in [
                "data-note=\"literal stroke-width='100' > end\"",
                "d=\"M1 2L3 4\"",
                "transform=\"translate(5 6)\"",
                "stroke=\"#c02020\"",
                "stroke-opacity=\"0.45\"",
                "opacity=\"0.8\"",
                "stroke-linecap=\"round\"",
                "stroke-linejoin=\"round\"",
                "stroke-dasharray=\"0 ",
            ] {
                assert!(styled.contains(untouched), "lost {untouched} at {zoom}");
            }
            assert!(!styled.ends_with(&format!("{active}</svg>")), "active stroke should adapt at {zoom}");
        }
    }

    #[test]
    fn malformed_input_refuses_without_rewriting_text_as_markup() {
        for source in [
            "",
            "<path/>",
            "<svg><g></svg>",
            "<svg><path stroke='black' stroke-width='1></svg>",
            "<svg><path stroke='black' stroke-width='1' stroke-width='2'/></svg>",
            "<svg><path stroke='black'stroke-width='1'/></svg>",
            "<svg><path note='<path/>'/></svg>",
            "<svg><!-- unterminated </svg>",
            "<svg><!-- invalid -- middle --></svg>",
            "<svg><![CDATA[unterminated </svg>",
            "<!DOCTYPE svg SYSTEM 'file:///private'><svg/>",
            "<?unsafe arbitrary?><svg/>",
            "<svg/>trailing text",
            "<svg/> <svg/>",
            "<svg>bad\0text</svg>",
            "<svg>bad ]]>text</svg>",
        ] {
            assert!(svg_at_zoom(source, 4.0).is_err(), "accepted malformed fixture {source:?}");
        }
        for raw in ["-1", "NaN", "inf", "1e9999", "5000", "1px"] {
            let source = drawing(&format!("<path d='M0 0L1 1' stroke='black' stroke-width='{raw}'/>"));
            for zoom in [1.0, 4.0] {
                assert!(svg_at_zoom(&source, zoom).is_err(), "accepted width {raw} at {zoom}");
            }
        }
        for raw in ["", "1,,2", ",1", "1,", "1 -2", "NaN 2", "1px 2", "1 inf"] {
            let source = drawing(&format!("<path d='M0 0L1 1' stroke='black' stroke-width='1' stroke-dasharray='{raw}'/>"));
            assert!(svg_at_zoom(&source, 4.0).is_err(), "accepted dash {raw:?}");
        }
        for zoom in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 0.0, 0.019, 200.1] {
            assert!(svg_at_zoom(PROTECTED, zoom).is_err());
        }
        let nested = format!("<svg>{}<path/>{}</svg>", "<g>".repeat(MAX_DEPTH), "</g>".repeat(MAX_DEPTH));
        assert!(svg_at_zoom(&nested, 4.0).is_err());
        let attrs = (0..MAX_ATTRIBUTES + 1).map(|i| format!(" a{i}='0'")).collect::<String>();
        assert!(svg_at_zoom(&format!("<svg{attrs}/>"), 4.0).is_err());
        assert!(svg_at_zoom("<?xml version='1.0'?><svg/>", 1.0).is_ok());
    }

    #[test]
    fn actual_render_keeps_cut_detail_hierarchy_without_linear_stroke_ballooning() {
        // .13/.18 mm details, actual .35 mm plan cuts, and the .5 mm heavy-cut pen.
        let pens = [0.614, 0.850, 1.654, 2.362];
        for zoom in [0.02, 0.125, 0.5, 1.0, 2.0, 4.0, 8.0, 20.0, 200.0] {
            let mut measured = Vec::new();
            for pen in pens {
                let source = line(pen);
                let adapted = svg_at_zoom(&source, zoom).unwrap();
                let samples = phase_ink(&adapted, zoom, 2);
                let ink = samples.iter().sum::<f64>() / samples.len() as f64;
                let wanted = screen_width(pen, f64::from(zoom));
                for sample in samples {
                    // Two edges rounded on the 1/4 physical-pixel grid contribute at most
                    // 1/4 px of width error; add two alpha steps and 1/64 px float allowance.
                    let phase_envelope = (0.25 + 2.0 / 255.0 + 1.0 / 64.0) / 2.0;
                    assert!((sample - wanted).abs() < phase_envelope, "pen {pen}, zoom {zoom}: individual phase {sample}, wanted {wanted}");
                    assert!(sample < 3.3, "individual phase cut silhouette balloons at {zoom}");
                }
                assert!((ink - wanted).abs() < 0.09, "pen {pen}, zoom {zoom}: measured {ink}, wanted {wanted}");
                assert!(ink < 3.3, "cut silhouette must not consume an opening at {zoom}");
                measured.push(ink);
            }
            assert!(measured.windows(2).all(|pair| pair[0] < pair[1]), "lost fine/medium/cut order at {zoom}: {measured:?}");
        }
        let source = line(2.362);
        let physical = mean_ink(&source, 8.0, 2);
        let adaptive = mean_ink(&svg_at_zoom(&source, 8.0).unwrap(), 8.0, 2);
        assert!(physical > 18.0, "regression fixture must reproduce paper-pen ballooning");
        assert!(adaptive < 3.3 && physical > adaptive * 5.0);
    }

    #[test]
    fn closer_views_reveal_real_frame_gaps_instead_of_scaling_a_black_blob() {
        let source = drawing(r#"<path d="M128 0L128 256M129 0L129 256" fill="none" stroke="black" stroke-width="0.85"/>"#);
        let clear_gap = |zoom: f32, adapt: bool| {
            let svg = if adapt { svg_at_zoom(&source, zoom).unwrap() } else { source.clone() };
            let pixels = render(&svg, zoom, 2);
            let from = (128.0 - zoom) as u32;
            let to = (128.0 + zoom) as u32;
            (from..to).filter(|x| pixel(&pixels, *x, 128)[0] > 250).count()
        };
        let overview = clear_gap(1.0, true);
        let near = clear_gap(4.0, true);
        let close = clear_gap(8.0, true);
        assert!(near > overview && close > near, "real gap must become visible at 4× and 8×: {overview}, {near}, {close}");
        assert!(close >= clear_gap(8.0, false) + 6, "thin on-screen pens must expose more of the actual geometry gap");
    }

    #[test]
    fn logical_ink_is_independent_of_retina_pixel_density() {
        for zoom in [0.125, 0.5, 1.0, 4.0, 8.0, 200.0] {
            let adapted = svg_at_zoom(&line(2.362), zoom).unwrap();
            let one = mean_ink(&adapted, zoom, 1);
            for ppp in [2, 3] {
                let other = mean_ink(&adapted, zoom, ppp);
                assert!((one - other).abs() < 0.1, "logical width changed at zoom {zoom}, density {ppp}: {one} vs {other}");
            }
        }
    }

    #[test]
    fn round_zero_dash_red_guides_remain_visible_and_separated_in_real_pixels() {
        let source = drawing(
            r##"<path d="M0 128L256 128" fill="none" stroke="#c02020" stroke-width="0.85" stroke-linecap="round" stroke-dasharray="0 4.724"/>"##,
        );
        for zoom in [0.5, 2.0, 8.0] {
            let adapted = svg_at_zoom(&source, zoom).unwrap();
            assert!(adapted.contains("stroke-dasharray=\"0 "));
            let pixels = render(&adapted, zoom, 2);
            let mut dots = 0;
            let mut was_red = false;
            let mut white = 0;
            for x in 0..pixels.width() {
                let [r, g, b, _] = pixel(&pixels, x, 128);
                let red = i16::from(r) > i16::from(g) + 20 && i16::from(r) > i16::from(b) + 20;
                if red && !was_red {
                    dots += 1;
                }
                was_red = red;
                if r > 250 && g > 250 && b > 250 {
                    white += 1;
                }
            }
            assert!(dots >= 3 && white >= 10, "guide lost its visible red dots/gaps at {zoom}: {dots} dots, {white} white pixels");
        }
    }
}
