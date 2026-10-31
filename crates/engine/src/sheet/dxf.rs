//! DXF from a scene, written from Autodesk's public DXF reference for AutoCAD 2000 (AC1015,
//! ASCII). Model space holds the drawing at 1:1 in millimetres ($INSUNITS 4, $MEASUREMENT 1); every
//! scene layer is a DXF layer with its colour, lineweight and linetype. Line work is LINE and
//! LWPOLYLINE, poche and fills are HATCH (written first, so lines draw over them), labels are TEXT
//! and the title block under the drawing is MTEXT. The file is in its declared code page,
//! Windows-1252 (æ, ø, å, ×, – as single bytes); anything outside it is written `\U+XXXX`.

use std::fmt::Write as _;

use super::{Anchor, Fill, Item, Layer, Path, Rgb, Scene, Stroke, Text, scale_text};

/// The lettering on top of the drawing in the PDF, under it in the DXF.
pub(crate) struct TitleBlock<'a> {
    pub(crate) title: &'a str,
    pub(crate) project: &'a str,
    pub(crate) caption: &'a str,
    pub(crate) note: Option<String>,
}

/// Cap height of Liberation Sans / Arial per em: DXF text heights are cap heights.
const CAP: f64 = 0.7;
/// ANSI31's line spacing in its pattern definition, drawing units.
const ANSI31: f64 = 3.175;
/// Masonry hatch spacing, paper mm (as the SVG pattern: 7 px along x at 45°).
const MASONRY_SPACING: f64 = 7.0 / std::f64::consts::SQRT_2 * super::MM_PER_PX;
/// Lineweights DXF allows, 1/100 mm.
const LINEWEIGHTS: [i32; 24] = [0, 5, 9, 13, 15, 18, 20, 25, 30, 35, 40, 50, 53, 60, 70, 80, 90, 100, 106, 120, 140, 158, 200, 211];

// Fixed handles for the objects the tables and blocks point at; everything else counts up from
// FIRST.
const H_BLOCK_RECORDS: u32 = 0x1;
const H_LAYERS: u32 = 0x2;
const H_STYLES: u32 = 0x3;
const H_LTYPES: u32 = 0x5;
const H_VIEWS: u32 = 0x6;
const H_UCS: u32 = 0x7;
const H_VPORTS: u32 = 0x8;
const H_APPIDS: u32 = 0x9;
const H_DIMSTYLES: u32 = 0xA;
const H_ROOT: u32 = 0xC;
const H_GROUPS: u32 = 0xD;
const H_PLOTSTYLES: u32 = 0xE;
const H_PLACEHOLDER: u32 = 0xF;
const H_MLINESTYLES: u32 = 0x17;
const H_MLINESTYLE: u32 = 0x18;
const H_LAYOUTS: u32 = 0x1A;
const H_PS_RECORD: u32 = 0x1B;
const H_PS_BLOCK: u32 = 0x1C;
const H_PS_END: u32 = 0x1D;
const H_PS_LAYOUT: u32 = 0x1E;
const H_MS_RECORD: u32 = 0x1F;
const H_MS_BLOCK: u32 = 0x20;
const H_MS_END: u32 = 0x21;
const H_MS_LAYOUT: u32 = 0x22;
const FIRST: u32 = 0x100;

struct W {
    out: String,
    next: u32,
}

impl W {
    fn pair(&mut self, code: u16, value: impl std::fmt::Display) {
        let _ = write!(self.out, "{code}\n{value}\n");
    }

    fn num(&mut self, code: u16, v: f64) {
        let v = if v.is_finite() { v } else { 0.0 };
        let _ = write!(self.out, "{code}\n{v:.4}\n");
    }

    fn handle(&mut self) -> u32 {
        let h = self.next;
        self.next += 1;
        h
    }

    fn table(&mut self, name: &str, handle: u32, count: usize) {
        self.pair(0, "TABLE");
        self.pair(2, name);
        self.pair(5, format_args!("{handle:X}"));
        self.pair(330, 0);
        self.pair(100, "AcDbSymbolTable");
        self.pair(70, count);
    }

    fn record(&mut self, kind: &str, table: u32, subclass: &str) {
        let h = self.handle();
        self.pair(0, kind);
        self.pair(if kind == "DIMSTYLE" { 105 } else { 5 }, format_args!("{h:X}"));
        self.pair(330, format_args!("{table:X}"));
        self.pair(100, "AcDbSymbolTableRecord");
        self.pair(100, subclass);
    }

    /// The start of a model-space entity.
    fn entity(&mut self, kind: &str, layer: Layer, colour: Option<u8>, stroke: Option<&Stroke>) {
        let h = self.handle();
        self.pair(0, kind);
        self.pair(5, format_args!("{h:X}"));
        self.pair(330, format_args!("{H_MS_RECORD:X}"));
        self.pair(100, "AcDbEntity");
        self.pair(8, layer.name());
        if let Some(s) = stroke
            && s.dashed != layer.dashed()
        {
            self.pair(6, if s.dashed { "DASHED" } else { "Continuous" });
        }
        if let Some(c) = colour {
            self.pair(62, c);
        }
        if let Some(s) = stroke {
            let lw = lineweight(s.pen);
            if lw != lineweight(layer.pen()) {
                self.pair(370, lw);
            }
        }
    }
}

/// The nearest lineweight DXF allows, 1/100 mm.
pub(crate) fn lineweight(pen_mm: f64) -> i32 {
    let want = pen_mm * 100.0;
    LINEWEIGHTS.iter().copied().min_by(|a, b| (f64::from(*a) - want).abs().total_cmp(&(f64::from(*b) - want).abs())).unwrap_or(25)
}

/// The AutoCAD colour index closest to `c`, or `None` for black (the layer's colour 7).
pub(crate) fn aci(c: Rgb) -> Option<u8> {
    let (r, g, b) = (i32::from(c[0]), i32::from(c[1]), i32::from(c[2]));
    let (hi, lo) = (r.max(g).max(b), r.min(g).min(b));
    if hi < 60 {
        return None;
    }
    if hi - lo < 28 {
        const GREYS: [(u8, i32); 8] = [(250, 51), (251, 80), (252, 105), (8, 128), (253, 130), (254, 190), (9, 192), (255, 255)];
        let v = (r + g + b) / 3;
        return GREYS.iter().min_by_key(|(_, l)| (l - v).abs()).map(|(k, _)| *k);
    }
    let hue = |x: i32| x > hi - 60;
    Some(match (hue(r), hue(g), hue(b)) {
        (true, true, false) => 2,
        (false, true, true) => 4,
        (true, false, true) => 6,
        (_, true, _) => 3,
        (_, _, true) => 5,
        _ => 1,
    })
}

/// The Windows-1252 byte for `c`, if it has one.
fn cp1252(c: char) -> Option<u8> {
    const HIGH: [(char, u8); 27] = [
        ('€', 0x80),
        ('‚', 0x82),
        ('ƒ', 0x83),
        ('„', 0x84),
        ('…', 0x85),
        ('†', 0x86),
        ('‡', 0x87),
        ('ˆ', 0x88),
        ('‰', 0x89),
        ('Š', 0x8A),
        ('‹', 0x8B),
        ('Œ', 0x8C),
        ('Ž', 0x8E),
        ('‘', 0x91),
        ('’', 0x92),
        ('“', 0x93),
        ('”', 0x94),
        ('•', 0x95),
        ('–', 0x96),
        ('—', 0x97),
        ('˜', 0x98),
        ('™', 0x99),
        ('š', 0x9A),
        ('›', 0x9B),
        ('œ', 0x9C),
        ('ž', 0x9E),
        ('Ÿ', 0x9F),
    ];
    match c as u32 {
        v @ (0x0A | 0x20..=0x7E | 0xA0..=0xFF) => u8::try_from(v).ok(),
        _ => HIGH.iter().find(|(h, _)| *h == c).map(|(_, b)| *b),
    }
}

/// The file's bytes: `text` holds only characters [`encode`] lets through.
pub(crate) fn bytes(text: &str) -> Vec<u8> {
    text.chars().map(|c| cp1252(c).unwrap_or(b'?')).collect()
}

/// DXF text: Windows-1252 characters as they are, anything else as `\U+XXXX`. MTEXT also
/// escapes its formatting characters. At most `limit` output characters, never cutting an escape.
fn encode(s: &str, mtext: bool, limit: usize) -> String {
    let mut out = String::new();
    let mut count = 0;
    for ch in s.chars() {
        let piece = match ch {
            '\\' | '{' | '}' if mtext => format!("\\{ch}"),
            '\n' if mtext => "\\P".into(),
            // The typographic minus (levels) as the hyphen every font has.
            '−' => "-".into(),
            c if c.is_control() => " ".into(),
            c if cp1252(c).is_some() => c.to_string(),
            c if (c as u32) <= 0xFFFF => format!("\\U+{:04X}", c as u32),
            _ => "?".into(),
        };
        let n = piece.chars().count();
        if count + n > limit {
            break;
        }
        count += n;
        out.push_str(&piece);
    }
    // "%%" starts a TEXT control code (%%c, %%d, …); "%%%" is a literal percent sign.
    if mtext { out } else { out.replace('%', "%%%") }
}

fn finite(p: &[f64; 2]) -> bool {
    p[0].is_finite() && p[1].is_finite()
}

fn hatch(w: &mut W, p: &Path, fill: Fill, scale: f64) {
    let rings: Vec<&Vec<[f64; 2]>> = p.rings.iter().filter(|r| r.len() >= 3 && r.iter().all(finite)).collect();
    if rings.is_empty() {
        return;
    }
    let layer = if p.layer == Layer::WallCut { Layer::WallPattern } else { p.layer };
    let (colour, pattern) = match fill {
        Fill::Solid(c) => {
            // Light tones only matter on paper; in CAD they would glare on a dark screen.
            if c.iter().all(|v| *v > 216) {
                return;
            }
            (aci(c).or(Some(250)), None)
        }
        Fill::Masonry => (Some(8), Some(MASONRY_SPACING * scale / ANSI31)),
        Fill::Dots => return,
    };
    w.entity("HATCH", layer, colour, None);
    w.pair(100, "AcDbHatch");
    for code in [10, 20, 30, 210, 220] {
        w.num(code, 0.0);
    }
    w.num(230, 1.0);
    w.pair(2, if pattern.is_some() { "ANSI31" } else { "SOLID" });
    w.pair(70, i32::from(pattern.is_none()));
    w.pair(71, 0);
    w.pair(91, rings.len());
    for (k, ring) in rings.iter().enumerate() {
        // Polyline boundary; the first one also marked external.
        w.pair(92, if k == 0 { 3 } else { 2 });
        w.pair(72, 0);
        w.pair(73, 1);
        w.pair(93, ring.len());
        for q in ring.iter() {
            w.num(10, q[0]);
            w.num(20, q[1]);
        }
        w.pair(97, 0);
    }
    // Odd parity: rings inside rings are holes.
    w.pair(75, 0);
    w.pair(76, 1);
    if let Some(s) = pattern {
        w.num(52, 0.0);
        w.num(41, s);
        w.pair(77, 0);
        w.pair(78, 1);
        w.num(53, 45.0);
        w.num(43, 0.0);
        w.num(44, 0.0);
        // The line offset, rotated with the line: (0, 3.175) at 45°.
        let d = ANSI31 * s / std::f64::consts::SQRT_2;
        w.num(45, -d);
        w.num(46, d);
        w.pair(79, 0);
    }
    w.pair(98, 0);
}

fn lines(w: &mut W, p: &Path) {
    let Some(stroke) = p.stroke else { return };
    let colour = aci(stroke.colour);
    for ring in &p.rings {
        let ring: Vec<[f64; 2]> = ring.iter().copied().filter(finite).collect();
        if ring.len() < 2 {
            continue;
        }
        if ring.len() == 2 && !p.closed {
            w.entity("LINE", p.layer, colour, Some(&stroke));
            w.pair(100, "AcDbLine");
            for (c, q) in [(10, ring[0]), (11, ring[1])] {
                w.num(c, q[0]);
                w.num(c + 10, q[1]);
                w.num(c + 20, 0.0);
            }
            continue;
        }
        w.entity("LWPOLYLINE", p.layer, colour, Some(&stroke));
        w.pair(100, "AcDbPolyline");
        w.pair(90, ring.len());
        w.pair(70, i32::from(p.closed));
        for q in &ring {
            w.num(10, q[0]);
            w.num(20, q[1]);
        }
    }
}

fn text(w: &mut W, t: &Text, scale: f64) {
    if !finite(&t.at) || !t.deg.is_finite() {
        return;
    }
    let value = encode(&t.text, false, 250);
    if value.trim().is_empty() {
        return;
    }
    w.entity("TEXT", t.layer, aci(t.colour), None);
    w.pair(100, "AcDbText");
    w.num(10, t.at[0]);
    w.num(20, t.at[1]);
    w.num(30, 0.0);
    w.num(40, t.size * scale * CAP);
    w.pair(1, value);
    w.num(50, t.deg.rem_euclid(360.0));
    w.pair(7, "Standard");
    let h = match t.anchor {
        Anchor::Start => 0,
        Anchor::Middle => 1,
        Anchor::End => 2,
    };
    let v = if t.middle { 2 } else { 0 };
    if h != 0 || v != 0 {
        w.pair(72, h);
        w.num(11, t.at[0]);
        w.num(21, t.at[1]);
        w.num(31, 0.0);
    }
    w.pair(100, "AcDbText");
    if v != 0 {
        w.pair(73, v);
    }
}

/// One line of the title block: MTEXT, top left at `at`.
fn mtext(w: &mut W, layer: Layer, at: [f64; 2], height: f64, s: &str) {
    let value = encode(s, true, 4000);
    if value.trim().is_empty() || !finite(&at) {
        return;
    }
    w.entity("MTEXT", layer, None, None);
    w.pair(100, "AcDbMText");
    w.num(10, at[0]);
    w.num(20, at[1]);
    w.num(30, 0.0);
    w.num(40, height);
    w.num(41, 0.0);
    w.pair(71, 1);
    w.pair(72, 1);
    let chars: Vec<char> = value.chars().collect();
    let chunks: Vec<String> = chars.chunks(250).map(|c| c.iter().collect()).collect();
    for (k, chunk) in chunks.iter().enumerate() {
        w.pair(if k + 1 == chunks.len() { 1 } else { 3 }, chunk);
    }
    w.pair(7, "Standard");
}

/// The whole DXF file for `scene` with `block` lettered under the drawing.
pub(crate) fn write(scene: &Scene, block: &TitleBlock<'_>) -> String {
    let scale = if scene.scale.is_finite() && scene.scale > 0.0 { scene.scale } else { 100.0 };
    let b = scene.bounds().unwrap_or([0.0, 0.0, 1000.0, 1000.0]);
    let mut w = W { out: String::new(), next: FIRST };

    // Tables.
    w.pair(0, "SECTION");
    w.pair(2, "TABLES");
    w.table("VPORT", H_VPORTS, 1);
    w.record("VPORT", H_VPORTS, "AcDbViewportTableRecord");
    w.pair(2, "*Active");
    w.pair(70, 0);
    let (cx, cy) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
    for (code, v) in [(10, 0.0), (20, 0.0), (11, 1.0), (21, 1.0), (12, cx), (22, cy), (13, 0.0), (23, 0.0), (14, 10.0), (24, 10.0)] {
        w.num(code, v);
    }
    for (code, v) in [(15, 10.0), (25, 10.0), (16, 0.0), (26, 0.0), (36, 1.0), (17, 0.0), (27, 0.0), (37, 0.0)] {
        w.num(code, v);
    }
    w.num(40, ((b[3] - b[1]) * 1.4).max((b[2] - b[0]) / 1.6).max(1000.0));
    w.num(41, 1.6);
    w.num(42, 50.0);
    for code in [43, 44, 50, 51] {
        w.num(code, 0.0);
    }
    for (code, v) in [(71, 0), (72, 1000), (73, 1), (74, 3), (75, 0), (76, 0), (77, 0), (78, 0), (281, 0), (65, 0)] {
        w.pair(code, v);
    }
    w.num(146, 0.0);
    w.pair(0, "ENDTAB");

    w.table("LTYPE", H_LTYPES, 4);
    for name in ["ByBlock", "ByLayer", "Continuous"] {
        w.record("LTYPE", H_LTYPES, "AcDbLinetypeTableRecord");
        w.pair(2, name);
        w.pair(70, 0);
        w.pair(3, if name == "Continuous" { "Solid line" } else { "" });
        w.pair(72, 65);
        w.pair(73, 0);
        w.num(40, 0.0);
    }
    // Dashes in paper mm; $LTSCALE scales them to the drawing.
    w.record("LTYPE", H_LTYPES, "AcDbLinetypeTableRecord");
    w.pair(2, "DASHED");
    w.pair(70, 0);
    w.pair(3, "Dashed __ __ __");
    w.pair(72, 65);
    w.pair(73, 2);
    w.num(40, 3.0);
    for (len, kind) in [(2.0, 0), (-1.0, 0)] {
        w.num(49, len);
        w.pair(74, kind);
    }
    w.pair(0, "ENDTAB");

    w.table("LAYER", H_LAYERS, Layer::ALL.len() + 1);
    let layer = |w: &mut W, name: &str, colour: u8, dashed: bool, lw: i32| {
        w.record("LAYER", H_LAYERS, "AcDbLayerTableRecord");
        w.pair(2, name);
        w.pair(70, 0);
        w.pair(62, colour);
        w.pair(6, if dashed { "DASHED" } else { "Continuous" });
        w.pair(370, lw);
        w.pair(390, format_args!("{H_PLACEHOLDER:X}"));
    };
    layer(&mut w, "0", 7, false, -3);
    for l in Layer::ALL {
        layer(&mut w, l.name(), l.aci(), l.dashed(), lineweight(l.pen()));
    }
    w.pair(0, "ENDTAB");

    w.table("STYLE", H_STYLES, 1);
    w.record("STYLE", H_STYLES, "AcDbTextStyleTableRecord");
    w.pair(2, "Standard");
    w.pair(70, 0);
    w.num(40, 0.0);
    w.num(41, 1.0);
    w.num(50, 0.0);
    w.pair(71, 0);
    w.num(42, 2.5);
    w.pair(3, "arial.ttf");
    w.pair(4, "");
    w.pair(0, "ENDTAB");

    for (name, handle) in [("VIEW", H_VIEWS), ("UCS", H_UCS)] {
        w.table(name, handle, 0);
        w.pair(0, "ENDTAB");
    }

    w.table("APPID", H_APPIDS, 1);
    w.record("APPID", H_APPIDS, "AcDbRegAppTableRecord");
    w.pair(2, "ACAD");
    w.pair(70, 0);
    w.pair(0, "ENDTAB");

    w.table("DIMSTYLE", H_DIMSTYLES, 1);
    w.pair(100, "AcDbDimStyleTable");
    w.record("DIMSTYLE", H_DIMSTYLES, "AcDbDimStyleTableRecord");
    w.pair(2, "Standard");
    w.pair(70, 0);
    w.pair(0, "ENDTAB");

    w.table("BLOCK_RECORD", H_BLOCK_RECORDS, 2);
    for (name, record, layout) in [("*Model_Space", H_MS_RECORD, H_MS_LAYOUT), ("*Paper_Space", H_PS_RECORD, H_PS_LAYOUT)] {
        w.pair(0, "BLOCK_RECORD");
        w.pair(5, format_args!("{record:X}"));
        w.pair(330, format_args!("{H_BLOCK_RECORDS:X}"));
        w.pair(100, "AcDbSymbolTableRecord");
        w.pair(100, "AcDbBlockTableRecord");
        w.pair(2, name);
        w.pair(340, format_args!("{layout:X}"));
    }
    w.pair(0, "ENDTAB");
    w.pair(0, "ENDSEC");

    // Blocks: the model and paper space definitions.
    w.pair(0, "SECTION");
    w.pair(2, "BLOCKS");
    for (name, record, begin, end) in [("*Model_Space", H_MS_RECORD, H_MS_BLOCK, H_MS_END), ("*Paper_Space", H_PS_RECORD, H_PS_BLOCK, H_PS_END)] {
        w.pair(0, "BLOCK");
        w.pair(5, format_args!("{begin:X}"));
        w.pair(330, format_args!("{record:X}"));
        w.pair(100, "AcDbEntity");
        w.pair(8, "0");
        w.pair(100, "AcDbBlockBegin");
        w.pair(2, name);
        w.pair(70, 0);
        for code in [10, 20, 30] {
            w.num(code, 0.0);
        }
        w.pair(3, name);
        w.pair(1, "");
        w.pair(0, "ENDBLK");
        w.pair(5, format_args!("{end:X}"));
        w.pair(330, format_args!("{record:X}"));
        w.pair(100, "AcDbEntity");
        w.pair(8, "0");
        w.pair(100, "AcDbBlockEnd");
    }
    w.pair(0, "ENDSEC");

    // Entities: fills first, so the line work draws over them, then lines, then lettering.
    w.pair(0, "SECTION");
    w.pair(2, "ENTITIES");
    for item in &scene.items {
        if let Item::Path(p) = item
            && let Some(f) = p.fill
        {
            hatch(&mut w, p, f, scale);
        }
    }
    for item in &scene.items {
        if let Item::Path(p) = item {
            lines(&mut w, p);
        }
    }
    for item in &scene.items {
        if let Item::Text(t) = item {
            text(&mut w, t, scale);
        }
    }
    // The title block, under the drawing at the sheet's lettering sizes.
    let mut y = b[1] - 12.0 * scale;
    let x = b[0];
    let mut lines_out: Vec<(f64, String)> = vec![(8.5, block.title.to_string()), (3.8, block.project.to_string())];
    if let Some(note) = &block.note {
        lines_out.push((3.2, note.clone()));
    }
    lines_out.push((5.9, block.caption.to_string()));
    lines_out.push((2.5, format!("{} · Mål og tegninger er vejledende · Trigon", scale_text(scale))));
    for (size, s) in &lines_out {
        mtext(&mut w, Layer::Title, [x, y], size * scale * CAP, s);
        y -= size * scale * 1.6;
    }
    w.pair(0, "ENDSEC");

    // Objects: the dictionaries and layouts AutoCAD expects.
    w.pair(0, "SECTION");
    w.pair(2, "OBJECTS");
    let dict = |w: &mut W, kind: &str, handle: u32, owner: u32, entries: &[(&str, u32)]| {
        w.pair(0, kind);
        w.pair(5, format_args!("{handle:X}"));
        w.pair(330, format_args!("{owner:X}"));
        w.pair(100, "AcDbDictionary");
        w.pair(281, 1);
        for (name, h) in entries {
            w.pair(3, name);
            w.pair(350, format_args!("{h:X}"));
        }
    };
    dict(
        &mut w,
        "DICTIONARY",
        H_ROOT,
        0,
        &[("ACAD_GROUP", H_GROUPS), ("ACAD_LAYOUT", H_LAYOUTS), ("ACAD_MLINESTYLE", H_MLINESTYLES), ("ACAD_PLOTSTYLENAME", H_PLOTSTYLES)],
    );
    dict(&mut w, "DICTIONARY", H_GROUPS, H_ROOT, &[]);
    dict(&mut w, "DICTIONARY", H_LAYOUTS, H_ROOT, &[("Layout1", H_PS_LAYOUT), ("Model", H_MS_LAYOUT)]);
    dict(&mut w, "DICTIONARY", H_MLINESTYLES, H_ROOT, &[("Standard", H_MLINESTYLE)]);
    dict(&mut w, "ACDBDICTIONARYWDFLT", H_PLOTSTYLES, H_ROOT, &[("Normal", H_PLACEHOLDER)]);
    w.pair(100, "AcDbDictionaryWithDefault");
    w.pair(340, format_args!("{H_PLACEHOLDER:X}"));
    w.pair(0, "ACDBPLACEHOLDER");
    w.pair(5, format_args!("{H_PLACEHOLDER:X}"));
    w.pair(330, format_args!("{H_PLOTSTYLES:X}"));
    for (name, handle, record, tab, flags) in [("Model", H_MS_LAYOUT, H_MS_RECORD, 0, 1024), ("Layout1", H_PS_LAYOUT, H_PS_RECORD, 1, 0)] {
        w.pair(0, "LAYOUT");
        w.pair(5, format_args!("{handle:X}"));
        w.pair(330, format_args!("{H_LAYOUTS:X}"));
        w.pair(100, "AcDbPlotSettings");
        w.pair(1, "");
        w.pair(4, "A3");
        w.pair(6, "");
        for (code, v) in [(40, 7.5), (41, 20.0), (42, 7.5), (43, 20.0), (44, 420.0), (45, 297.0), (46, 0.0), (47, 0.0), (48, 0.0), (49, 0.0)] {
            w.num(code, v);
        }
        for (code, v) in [(140, 0.0), (141, 0.0), (142, 1.0), (143, 1.0)] {
            w.num(code, v);
        }
        w.pair(70, flags);
        w.pair(72, 1);
        w.pair(73, 0);
        w.pair(74, 5);
        w.pair(7, "");
        w.pair(75, 16);
        w.pair(76, 0);
        w.pair(77, 2);
        w.pair(78, 300);
        for (code, v) in [(147, 1.0), (148, 0.0), (149, 0.0)] {
            w.num(code, v);
        }
        w.pair(100, "AcDbLayout");
        w.pair(1, name);
        w.pair(70, 1);
        w.pair(71, tab);
        for (code, v) in [(10, 0.0), (20, 0.0), (11, 420.0), (21, 297.0), (12, 0.0), (22, 0.0), (32, 0.0)] {
            w.num(code, v);
        }
        for (code, v) in [(14, "1e+20"), (24, "1e+20"), (34, "1e+20"), (15, "-1e+20"), (25, "-1e+20"), (35, "-1e+20")] {
            w.pair(code, v);
        }
        for (code, v) in [(146, 0.0), (13, 0.0), (23, 0.0), (33, 0.0), (16, 1.0), (26, 0.0), (36, 0.0), (17, 0.0), (27, 1.0), (37, 0.0)] {
            w.num(code, v);
        }
        w.pair(76, 0);
        w.pair(330, format_args!("{record:X}"));
    }
    w.pair(0, "MLINESTYLE");
    w.pair(5, format_args!("{H_MLINESTYLE:X}"));
    w.pair(330, format_args!("{H_MLINESTYLES:X}"));
    w.pair(100, "AcDbMlineStyle");
    w.pair(2, "Standard");
    w.pair(70, 0);
    w.pair(3, "");
    w.pair(62, 256);
    w.num(51, 90.0);
    w.num(52, 90.0);
    w.pair(71, 2);
    for offset in [0.5, -0.5] {
        w.num(49, offset);
        w.pair(62, 256);
        w.pair(6, "BYLAYER");
    }
    w.pair(0, "ENDSEC");
    w.pair(0, "EOF");

    // The header goes first but needs the next free handle and the extents.
    let mut h = W { out: String::new(), next: 0 };
    h.pair(0, "SECTION");
    h.pair(2, "HEADER");
    let var = |h: &mut W, name: &str| h.pair(9, name);
    var(&mut h, "$ACADVER");
    h.pair(1, "AC1015");
    var(&mut h, "$DWGCODEPAGE");
    h.pair(3, "ANSI_1252");
    var(&mut h, "$INSBASE");
    for code in [10, 20, 30] {
        h.num(code, 0.0);
    }
    let bottom = y.min(b[1]);
    for (name, p) in [("$EXTMIN", [b[0], bottom]), ("$EXTMAX", [b[2], b[3]])] {
        var(&mut h, name);
        h.num(10, p[0]);
        h.num(20, p[1]);
        h.num(30, 0.0);
    }
    for (name, p) in [("$LIMMIN", [b[0], bottom]), ("$LIMMAX", [b[2], b[3]])] {
        var(&mut h, name);
        h.num(10, p[0]);
        h.num(20, p[1]);
    }
    var(&mut h, "$LTSCALE");
    h.num(40, scale);
    var(&mut h, "$CELTSCALE");
    h.num(40, 1.0);
    var(&mut h, "$PSLTSCALE");
    h.pair(70, 1);
    var(&mut h, "$TEXTSTYLE");
    h.pair(7, "Standard");
    var(&mut h, "$CLAYER");
    h.pair(8, "0");
    var(&mut h, "$CELTYPE");
    h.pair(6, "ByLayer");
    var(&mut h, "$CECOLOR");
    h.pair(62, 256);
    var(&mut h, "$LUNITS");
    h.pair(70, 2);
    var(&mut h, "$LUPREC");
    h.pair(70, 1);
    var(&mut h, "$INSUNITS");
    h.pair(70, 4);
    var(&mut h, "$MEASUREMENT");
    h.pair(70, 1);
    var(&mut h, "$LWDISPLAY");
    h.pair(290, 1);
    var(&mut h, "$HANDSEED");
    h.pair(5, format_args!("{:X}", w.next));
    h.pair(0, "ENDSEC");
    h.pair(0, "SECTION");
    h.pair(2, "CLASSES");
    h.pair(0, "ENDSEC");
    h.out + &w.out
}

/// A short human summary of a written DXF: entity counts by type and the layers used.
pub(crate) fn summary(dxf: &str) -> serde_json::Value {
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    let mut layers = std::collections::BTreeSet::<String>::new();
    let mut in_entities = false;
    let lines: Vec<&str> = dxf.lines().collect();
    for pair in lines.chunks(2) {
        let [code, value] = pair else { continue };
        match (code.trim(), value.trim()) {
            ("2", "ENTITIES") => in_entities = true,
            ("0", "ENDSEC") => in_entities = false,
            ("0", kind) if in_entities => *counts.entry(kind.to_string()).or_default() += 1,
            ("8", layer) if in_entities => {
                layers.insert(layer.to_string());
            }
            _ => {}
        }
    }
    serde_json::json!({"entities": counts, "layers": layers})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheet::{Halo, Stroke};

    fn pairs(dxf: &str) -> Vec<(i32, String)> {
        let lines: Vec<&str> = dxf.lines().collect();
        assert_eq!(lines.len() % 2, 0, "group codes and values come in pairs");
        lines.chunks(2).map(|p| (p[0].trim().parse::<i32>().unwrap(), p[1].to_string())).collect()
    }

    fn sample() -> Scene {
        let mut s = Scene::new(100.0);
        let square = vec![[0.0, 0.0], [4000.0, 0.0], [4000.0, 3000.0], [0.0, 3000.0]];
        let hole = vec![[1000.0, 1000.0], [2000.0, 1000.0], [2000.0, 2000.0], [1000.0, 2000.0]];
        s.polygon(Layer::WallCut, vec![square.clone(), hole], Some(Fill::Masonry), Some(Stroke::new(0.5)));
        s.polygon(Layer::WallCut, vec![square], Some(Fill::Solid([150, 150, 150])), None);
        s.line(Layer::Ground, [-1000.0, 0.0], [5000.0, 0.0], Stroke::new(0.7));
        s.polyline(Layer::Level, vec![[0.0, 3000.0], [2000.0, 3000.0], [4000.0, 3000.0]], Stroke { dashed: true, ..Stroke::new(0.18) });
        let mut t = Text::new(Layer::Annotation, [2000.0, 3500.0], "Kælder 2×3 m² 100%", 2.5);
        t.halo = Halo::Box;
        s.text(t);
        s
    }

    #[test]
    fn dxf_has_every_section_layer_and_entity() {
        let dxf = write(&sample(), &TitleBlock { title: "Snit A–A", project: "Vej 1", caption: "Gård {x} →", note: None });
        let p = pairs(&dxf);
        let sections: Vec<&str> = p.windows(2).filter(|w| w[0] == (0, "SECTION".into())).map(|w| w[1].1.as_str()).collect();
        assert_eq!(sections, ["HEADER", "CLASSES", "TABLES", "BLOCKS", "ENTITIES", "OBJECTS"]);
        assert_eq!(p.last().unwrap(), &(0, "EOF".to_string()));
        let value = |var: &str| p.iter().position(|(c, v)| *c == 9 && v == var).map(|i| p[i + 1].1.clone());
        assert_eq!(value("$ACADVER").as_deref(), Some("AC1015"));
        assert_eq!(value("$INSUNITS").as_deref(), Some("4"));
        assert_eq!(value("$MEASUREMENT").as_deref(), Some("1"));
        for l in Layer::ALL {
            assert!(p.iter().any(|(c, v)| *c == 2 && v == l.name()), "layer {} declared", l.name());
        }
        let summary = summary(&dxf);
        assert_eq!(summary["entities"]["HATCH"], 2);
        // The masonry polygon's outline is one closed polyline per ring.
        assert_eq!(summary["entities"]["LWPOLYLINE"], 3);
        assert_eq!(summary["entities"]["LINE"], 1);
        assert_eq!(summary["entities"]["TEXT"], 1);
        assert_eq!(summary["entities"]["MTEXT"], 4);
        assert!(summary["layers"].as_array().unwrap().iter().any(|l| l == "A-WALL-PATT"), "fills go on the pattern layer");
        // Text in Windows-1252: æ, ×, ², – are single bytes; a literal percent sign; MTEXT braces
        // escaped; a character outside the code page as an escape.
        assert!(dxf.contains("Kælder 2×3 m² 100%%%"));
        assert!(dxf.contains("Snit A–A") && dxf.contains("Gård \\{x\\} \\U+2192"));
        let file = bytes(&dxf);
        assert_eq!(file.len(), dxf.chars().count(), "one byte per character");
        assert!(file.windows(6).any(|w| w == b"K\xe6lder"));
        // Handles are unique and below $HANDSEED.
        let seed = u32::from_str_radix(&value("$HANDSEED").unwrap(), 16).unwrap();
        let body = p.iter().position(|pair| *pair == (0, "ENDSEC".to_string())).unwrap();
        let mut handles: Vec<u32> =
            p[body..].iter().filter(|(c, _)| *c == 5 || *c == 105).map(|(_, v)| u32::from_str_radix(v, 16).unwrap()).collect();
        let n = handles.len();
        handles.sort_unstable();
        handles.dedup();
        assert_eq!(handles.len(), n);
        assert!(handles.iter().all(|h| *h < seed));
    }

    #[test]
    fn colours_and_lineweights_snap_to_what_dxf_allows() {
        assert_eq!(lineweight(0.5), 50);
        assert_eq!(lineweight(0.7), 70);
        assert_eq!(lineweight(0.13), 13);
        assert_eq!(lineweight(0.24), 25);
        assert_eq!(aci([20, 20, 20]), None);
        assert_eq!(aci([128, 128, 128]), Some(8));
        assert_eq!(aci([220, 30, 30]), Some(1));
        assert_eq!(aci([40, 150, 30]), Some(3));
        assert_eq!(encode("a\\b", false, 250), "a\\b");
        assert_eq!(encode("−2,83", false, 250), "-2,83");
        assert_eq!(encode(&"→".repeat(100), false, 250).len(), 245, "never cuts an escape");
        assert_eq!(encode(&"ø".repeat(300), false, 250).chars().count(), 250);
    }

    #[test]
    fn hostile_numbers_never_reach_the_file() {
        let mut s = Scene::new(f64::NAN);
        s.line(Layer::Wall, [f64::NAN, 0.0], [1.0, f64::INFINITY], Stroke::new(0.25));
        s.polygon(Layer::WallCut, vec![vec![[0.0, 0.0], [f64::NAN, 1.0], [1.0, 1.0]]], Some(Fill::Masonry), None);
        s.text(Text::new(Layer::Annotation, [f64::NAN, 0.0], "x", 2.5));
        let dxf = write(&s, &TitleBlock { title: "", project: "", caption: "", note: None });
        assert!(!dxf.to_lowercase().contains("nan") && !dxf.contains("inf"));
        assert_eq!(summary(&dxf)["entities"].as_object().map(|o| o.len()), Some(1), "only the title block's scale line");
    }
}
