//! Shared content model and the branded PDF renderer for meeting minutes.
//!
//! The Word renderer in [`crate::report_docx`] consumes the same [`ReportDoc`],
//! so the printed document and the editable one cannot drift apart.

use crate::{AgendaItem, LedgerEvent, Minutes};

// ---------------------------------------------------------------------------
// Brand palette
// ---------------------------------------------------------------------------

/// A colour carried as bytes so the PDF writer and the Word writer can never
/// disagree about what "Bea teal" is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl Color {
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    /// `#RRGGBB`, for Word.
    pub fn hex(self) -> String {
        format!("{:02X}{:02X}{:02X}", self.red, self.green, self.blue)
    }

    /// PDF non-stroking colour operator, at the precision PDF operators keep.
    fn fill_operator(self) -> String {
        format!(
            "{:.3} {:.3} {:.3} rg\n",
            f64::from(self.red) / 255.0,
            f64::from(self.green) / 255.0,
            f64::from(self.blue) / 255.0
        )
    }
}

pub const INK: Color = Color::new(0x0C, 0x11, 0x17);
pub const BODY: Color = Color::new(0x3D, 0x4A, 0x57);
pub const MUTED: Color = Color::new(0x76, 0x87, 0x98);
pub const QUIET: Color = Color::new(0xA9, 0xBA, 0xC8);
pub const RULE: Color = Color::new(0xE0, 0xE6, 0xEA);
pub const PAPER: Color = Color::new(0xFF, 0xFF, 0xFF);
pub const TEAL: Color = Color::new(0x5B, 0xD0, 0xAC);
pub const TEAL_INK: Color = Color::new(0x17, 0x80, 0x67);
pub const AMBER: Color = Color::new(0xE0, 0xB7, 0x74);
pub const AMBER_INK: Color = Color::new(0xA5, 0x73, 0x1F);
pub const AMBER_WASH: Color = Color::new(0xFC, 0xF4, 0xE6);

pub const BRAND_WORDMARK: &str = "BEA";
pub const DOCUMENT_KIND: &str = "MEETING MINUTES";
pub const FOOTER_KIND: &str = "Meeting minutes";
pub const EMPTY_NOTE: &str =
    "No summary, agenda, or recorded items were captured for this meeting.";

// ---------------------------------------------------------------------------
// Shared content model
// ---------------------------------------------------------------------------

/// Which accent a section wears. Open items are deliberately amber so a reader
/// can tell "settled" from "still open" without reading a word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accent {
    Settled,
    Open,
}

impl Accent {
    pub fn mark(self) -> Color {
        match self {
            Accent::Settled => TEAL,
            Accent::Open => AMBER,
        }
    }

    pub fn ink(self) -> Color {
        match self {
            Accent::Settled => TEAL_INK,
            Accent::Open => AMBER_INK,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgendaEntry {
    pub heading: String,
    /// Pre-formatted as `mm:ss–mm:ss`; absent when the model had no timing.
    pub time_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LedgerEntry {
    pub summary: String,
    pub owner: Option<String>,
    pub due: Option<String>,
    /// Pre-formatted as a human timestamp range.
    pub evidence: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReportSection {
    pub label: String,
    pub accent: Accent,
    pub entries: Vec<LedgerEntry>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReportDoc {
    pub title: String,
    pub summary: String,
    pub agenda: Vec<AgendaEntry>,
    pub sections: Vec<ReportSection>,
}

/// `mm:ss` — the shape every timestamp in a meeting document wants.
fn clock(seconds: u64) -> String {
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

/// Model output arrives with hard line breaks and stray tabs. Both show up as
/// gaps mid-paragraph in PDF and as phantom paragraphs in Word, so every string
/// that reaches a layout engine is flattened first.
fn flatten(value: &str) -> String {
    value
        .replace(['\r', '\n', '\t'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn optional_flat(value: &Option<String>) -> Option<String> {
    let cleaned = value.as_deref().map(flatten)?;
    (!cleaned.is_empty()).then_some(cleaned)
}

impl ReportDoc {
    pub fn from_minutes(minutes: &Minutes) -> Self {
        let title = flatten(&minutes.title);
        Self {
            // A generated document with a blank title reads like a bug report.
            title: if title.is_empty() {
                "Meeting Minutes".to_string()
            } else {
                title
            },
            summary: flatten(&minutes.summary),
            agenda: minutes
                .agenda
                .iter()
                .map(|item: &AgendaItem| AgendaEntry {
                    heading: flatten(&item.heading),
                    time_label: agenda_time_label(item),
                })
                .filter(|item| !item.heading.is_empty())
                .collect(),
            // A section with nothing in it is noise in a shared document.
            sections: [
                ("Decisions", Accent::Settled, &minutes.decisions),
                ("Action Items", Accent::Settled, &minutes.action_items),
                ("Unresolved Matters", Accent::Open, &minutes.unresolved),
            ]
            .into_iter()
            .map(|(label, accent, events)| ledger_section(label, accent, events))
            .filter(|section| !section.entries.is_empty())
            .collect(),
        }
    }

    pub fn has_body(&self) -> bool {
        !self.summary.is_empty() || !self.agenda.is_empty() || !self.sections.is_empty()
    }
}

fn agenda_time_label(item: &AgendaItem) -> Option<String> {
    match (item.start_seconds, item.end_seconds) {
        (Some(start), Some(end)) => Some(format!("{}–{}", clock(start), clock(end))),
        _ => None,
    }
}

fn ledger_section(label: &str, accent: Accent, events: &[LedgerEvent]) -> ReportSection {
    ReportSection {
        label: label.to_string(),
        accent,
        entries: events
            .iter()
            .map(|event| LedgerEntry {
                summary: flatten(&event.summary),
                owner: optional_flat(&event.owner),
                due: optional_flat(&event.due),
                evidence: event.evidence.first().map(|item| {
                    format!("{}–{}", clock(item.start_seconds), clock(item.end_seconds))
                }),
            })
            .filter(|entry| !entry.summary.is_empty())
            .collect(),
    }
}

// ===========================================================================
// PDF
// ===========================================================================

const PAGE_WIDTH: f64 = 612.0;
const PAGE_HEIGHT: f64 = 792.0;
pub const MARGIN: f64 = 58.0;
const BOTTOM: f64 = 58.0;
const TOP: f64 = 54.0;
const FOOTER_BASELINE: f64 = 30.0;
pub const CONTENT_WIDTH: f64 = PAGE_WIDTH - 2.0 * MARGIN;
/// Vertical space a section heading must be able to claim: the heading itself
/// plus at least one entry. Without this a label strands itself at a page foot.
const HEADING_KEEP_AHEAD: f64 = 74.0;
const BULLET_INDENT: f64 = 16.0;
const PANEL_INSET: f64 = 10.0;
const PANEL_BLEED: f64 = 10.0;
const CAP_HEIGHT_RATIO: f64 = 0.717;
const BASELINE_RATIO: f64 = 0.76;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Font {
    Regular,
    Bold,
    Italic,
}

impl Font {
    fn resource(self) -> &'static str {
        match self {
            Font::Regular => "F1",
            Font::Bold => "F2",
            Font::Italic => "F3",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct TextStyle {
    font: Font,
    size: f64,
    color: Color,
    /// Extra advance per glyph, in points. Small-caps-style labels need it to
    /// stop reading as a smudge at 8pt.
    tracking: f64,
}

impl TextStyle {
    const fn new(font: Font, size: f64, color: Color, tracking: f64) -> Self {
        Self {
            font,
            size,
            color,
            tracking,
        }
    }

    const fn body(size: f64) -> Self {
        Self::new(Font::Regular, size, BODY, 0.0)
    }

    const fn label(size: f64, color: Color) -> Self {
        Self::new(Font::Bold, size, color, 0.9)
    }
}

struct Run {
    text: String,
    style: TextStyle,
}

fn run(text: impl Into<String>, style: TextStyle) -> Run {
    Run {
        text: text.into(),
        style,
    }
}

// -- Font metrics ----------------------------------------------------------
//
// Advance widths for the Adobe standard-14 Helvetica faces, in 1/1000 em.
// Without them there is no such thing as a wrapped PDF: every paragraph would
// be a single line running off the page.

#[rustfmt::skip]
const HELVETICA: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278,
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556,
    1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778,
    667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556,
    333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556,
    556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];

#[rustfmt::skip]
const HELVETICA_BOLD: [u16; 95] = [
    278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278,
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 333, 333, 584, 584, 584, 611,
    975, 722, 722, 722, 722, 667, 611, 778, 722, 278, 556, 722, 611, 833, 722, 778,
    667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 333, 278, 333, 584, 556,
    333, 556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556, 278, 889, 611, 611,
    611, 611, 389, 556, 333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584,
];

/// WinAnsi characters outside ASCII: the punctuation the model actually emits
/// (en dashes, curly quotes, ellipses) plus the bullet and the euro sign.
fn extended_width(byte: u8) -> Option<f64> {
    Some(match byte {
        0x85 | 0x97 | 0x99 => 1000.0, // ellipsis, em dash, trademark
        0x80 => 556.0,                // euro — the AFM says 556, not 1000
        0x91 | 0x92 => 222.0,         // left/right single quote
        0x93 | 0x94 => 333.0,         // left/right double quote
        0x95 => 350.0,                // bullet
        0x96 => 556.0,                // en dash
        0xB7 => 278.0,                // periodcentered
        0xDF => 611.0,                // sharp s
        _ => return None,
    })
}

/// Accented Latin-1 letters are as wide as the plain letter they are built from,
/// which is what keeps the metric tables above printable ASCII. Case matters:
/// `ñ` is as wide as `n`, not as wide as `Ñ`.
fn fold_to_ascii(byte: u8) -> Option<u8> {
    Some(match byte {
        0xC0..=0xC5 | 0xE0..=0xE5 => b'A', // À Á Â Ã Ä Å à á â ã ä å
        0xC7 => b'C',                      // Ç ç
        0xC8..=0xCB | 0xE8..=0xEB => b'E', // È É Ê Ë è é ê ë
        0xCC..=0xCF | 0xEC..=0xEF => b'I', // Ì Í Î Ï ì í î ï
        0xD1 => b'N',                      // Ñ
        0xD2..=0xD6 | 0xF2..=0xF6 => b'O', // Ò Ó Ô Õ Ö ò ó ô õ ö
        0xD9..=0xDC => b'U',               // Ù Ú Û Ü
        0xDD => b'Y',                      // Ý
        0xF1 => b'n',                      // ñ
        0xF9..=0xFC => b'u',               // ù ú û ü
        0xFD | 0xFF => b'y',               // ý ÿ
        _ => return None,
    })
}

fn ascii_width(byte: u8, font: Font) -> f64 {
    let index = usize::from(byte) - 32;
    f64::from(match font {
        Font::Regular | Font::Italic => HELVETICA[index],
        // Oblique is a slanted Roman, so it keeps Roman's widths. Only Bold is
        // genuinely wider.
        Font::Bold => HELVETICA_BOLD[index],
    })
}

fn glyph_width(byte: u8, font: Font) -> f64 {
    match byte {
        0x20..=0x7E => ascii_width(byte, font),
        _ => extended_width(byte)
            .or_else(|| fold_to_ascii(byte).map(|base| ascii_width(base, font)))
            .unwrap_or(556.0),
    }
}

/// Maps a character onto the WinAnsi byte a Type1 Helvetica can draw. Anything
/// outside that repertoire degrades to `?` rather than emitting bytes a reader
/// would render as mojibake.
fn win_ansi(character: char) -> u8 {
    match character {
        '\u{00A0}' => 0x20,
        '\u{2018}' => 0x91,
        '\u{2019}' => 0x92,
        '\u{201C}' => 0x93,
        '\u{201D}' => 0x94,
        '\u{2013}' => 0x96,
        '\u{2014}' => 0x97,
        '\u{2022}' => 0x95,
        '\u{2026}' => 0x85,
        '\u{20AC}' => 0x80,
        '\u{2122}' => 0x99,
        other if (other as u32) < 0x80 => other as u8,
        other if (other as u32) <= 0xFF => other as u8,
        _ => b'?',
    }
}

fn text_width(value: &str, style: &TextStyle) -> f64 {
    value.chars().count() as f64 * style.tracking
        + value
            .chars()
            .map(|character| glyph_width(win_ansi(character), style.font) * style.size / 1000.0)
            .sum::<f64>()
}

#[derive(Clone, Copy)]
struct Glyph {
    character: char,
    style: TextStyle,
    advance: f64,
}

struct Piece {
    text: String,
    x: f64,
    style: TextStyle,
}

struct Line {
    pieces: Vec<Piece>,
    width: f64,
}

impl Line {
    fn empty() -> Self {
        Self {
            pieces: Vec::new(),
            width: 0.0,
        }
    }

    fn push(&mut self, glyph: &Glyph, x: f64) {
        match self.pieces.last_mut() {
            Some(piece) if piece.style == glyph.style => piece.text.push(glyph.character),
            _ => self.pieces.push(Piece {
                text: glyph.character.to_string(),
                x,
                style: glyph.style,
            }),
        }
    }
}

/// Greedy word wrap across mixed-style runs. Returns laid-out lines, each a list
/// of same-styled pieces carrying their x offset from the text origin.
fn wrap(runs: &[Run], width: f64) -> Vec<Line> {
    let glyphs: Vec<Glyph> = runs
        .iter()
        .flat_map(|entry| {
            let style = entry.style;
            entry.text.chars().map(move |character| Glyph {
                character,
                style,
                advance: glyph_width(win_ansi(character), style.font) * style.size / 1000.0
                    + style.tracking,
            })
        })
        .collect();

    let mut lines: Vec<Line> = Vec::new();
    let mut index = 0;
    while index < glyphs.len() {
        // A continuation line must not begin with the space that broke the
        // previous one.
        while index < glyphs.len() && glyphs[index].character.is_whitespace() {
            index += 1;
        }
        if index >= glyphs.len() {
            break;
        }
        let start = index;
        let mut used = 0.0;
        let mut break_at: Option<usize> = None;
        let mut overflowed = false;
        while index < glyphs.len() {
            let glyph = &glyphs[index];
            if glyph.character == '\n' {
                break;
            }
            if index > start && used + glyph.advance > width {
                overflowed = true;
                break;
            }
            if glyph.character == ' ' {
                break_at = Some(index);
            }
            used += glyph.advance;
            index += 1;
        }
        let mut end = index;
        if index < glyphs.len() && glyphs[index].character == '\n' {
            index += 1;
        }
        if overflowed {
            // Rewind to the last space so the word stays whole — but only when
            // the line actually ran out of room. `end == start` cannot happen
            // here: the skip loop above already consumed every leading
            // whitespace including newlines, and the overflow guard requires
            // `index > start`, so the inner loop always advances at least once.
            if let Some(candidate) = break_at {
                if candidate > start {
                    end = candidate;
                    index = candidate;
                }
            }
        }
        let mut line = Line::empty();
        let mut x = 0.0;
        for glyph in &glyphs[start..end] {
            line.push(glyph, x);
            x += glyph.advance;
        }
        line.width = x;
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(Line::empty());
    }
    lines
}

// -- Painting ---------------------------------------------------------------

#[derive(Debug, Clone)]
enum Op {
    Rect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        color: Color,
    },
    Circle {
        x: f64,
        y: f64,
        radius: f64,
        color: Color,
    },
    Text {
        x: f64,
        y: f64,
        style: TextStyle,
        text: String,
    },
}

type Page = Vec<Op>;

struct Canvas {
    pages: Vec<Page>,
    ops: Page,
    cursor: f64,
    limit: f64,
}

impl Canvas {
    fn new() -> Self {
        Self {
            pages: Vec::new(),
            ops: Vec::new(),
            cursor: TOP,
            limit: PAGE_HEIGHT - BOTTOM,
        }
    }

    fn finish(mut self) -> Vec<Page> {
        self.pages.push(self.ops);
        self.pages
    }

    fn new_page(&mut self) {
        self.pages.push(std::mem::take(&mut self.ops));
        self.cursor = TOP;
    }

    /// Breaks the page when `height` will not fit. A block too tall to ever fit
    /// is left to overflow from a fresh page rather than looping forever.
    fn reserve(&mut self, height: f64) {
        if self.cursor > 0.0 && self.cursor + height > self.limit {
            self.new_page();
        }
    }

    /// Vertical advance. A gap that would spill starts the next page instead of
    /// leaving a hole at the top of one.
    fn gap(&mut self, height: f64) {
        if self.cursor > 0.0 && self.cursor + height > self.limit {
            self.new_page();
        } else {
            self.cursor += height;
        }
    }

    fn rect(&mut self, width: f64, height: f64, color: Color) {
        self.overlay(MARGIN, self.cursor, width, height, color);
        self.cursor += height;
    }

    /// Draws a shape with its top edge at `top` without moving the cursor.
    fn overlay(&mut self, x: f64, top: f64, width: f64, height: f64, color: Color) {
        self.ops.push(Op::Rect {
            x,
            y: PAGE_HEIGHT - (top + height),
            width,
            height,
            color,
        });
    }

    /// Draws a filled dot `dy` points below the cursor, without moving it.
    fn dot(&mut self, dx: f64, dy: f64, radius: f64, color: Color) {
        self.ops.push(Op::Circle {
            x: MARGIN + dx,
            y: PAGE_HEIGHT - (self.cursor + dy),
            radius,
            color,
        });
    }

    fn text_line(&mut self, line: &Line, leading: f64, inset: f64) {
        self.reserve(leading);
        let baseline = self.cursor + leading * BASELINE_RATIO;
        for piece in &line.pieces {
            if piece.text.trim().is_empty() {
                continue;
            }
            self.ops.push(Op::Text {
                x: MARGIN + inset + piece.x,
                y: PAGE_HEIGHT - baseline,
                style: piece.style,
                text: piece.text.clone(),
            });
        }
        self.cursor += leading;
    }

    fn paragraph(&mut self, runs: &[Run], width: f64, leading: f64, inset: f64) {
        for line in wrap(runs, width) {
            self.text_line(&line, leading, inset);
        }
    }
}

fn draw_document(doc: &ReportDoc) -> Vec<Page> {
    let mut canvas = Canvas::new();

    // -- Masthead -----------------------------------------------------------
    canvas.rect(CONTENT_WIDTH, 3.5, TEAL);
    canvas.gap(20.0);
    canvas.paragraph(
        &[run(
            BRAND_WORDMARK,
            TextStyle::new(Font::Bold, 13.0, TEAL_INK, 3.0),
        )],
        CONTENT_WIDTH,
        18.0,
        0.0,
    );
    canvas.paragraph(
        &[run(
            DOCUMENT_KIND,
            TextStyle::new(Font::Regular, 8.0, MUTED, 2.2),
        )],
        CONTENT_WIDTH,
        12.0,
        0.0,
    );
    canvas.gap(20.0);
    canvas.paragraph(
        &[run(&doc.title, TextStyle::new(Font::Bold, 23.0, INK, -0.2))],
        CONTENT_WIDTH,
        28.0,
        0.0,
    );
    if !doc.summary.is_empty() {
        canvas.gap(11.0);
        canvas.paragraph(
            &[run(&doc.summary, TextStyle::body(11.0))],
            CONTENT_WIDTH,
            16.5,
            0.0,
        );
    }
    canvas.gap(20.0);
    canvas.rect(CONTENT_WIDTH, 1.2, TEAL);

    // -- Agenda -------------------------------------------------------------
    if !doc.agenda.is_empty() {
        draw_section_heading(&mut canvas, "Agenda", Accent::Settled);
        for item in &doc.agenda {
            draw_agenda_entry(&mut canvas, item);
        }
    }

    // -- Decisions / actions / open items -----------------------------------
    for section in &doc.sections {
        draw_section_heading(&mut canvas, &section.label, section.accent);
        for entry in &section.entries {
            draw_ledger_entry(&mut canvas, entry, section.accent);
        }
    }

    if !doc.has_body() {
        canvas.gap(16.0);
        canvas.paragraph(
            &[run(
                EMPTY_NOTE,
                TextStyle::new(Font::Italic, 10.5, MUTED, 0.0),
            )],
            CONTENT_WIDTH,
            16.0,
            0.0,
        );
    }

    canvas.finish()
}

fn draw_section_heading(canvas: &mut Canvas, label: &str, accent: Accent) {
    // A heading alone at a page foot reads as a section with no content.
    if canvas.cursor + HEADING_KEEP_AHEAD > canvas.limit {
        canvas.new_page();
    } else {
        canvas.gap(24.0);
    }
    let top = canvas.cursor;
    let cap = 9.5 * CAP_HEIGHT_RATIO;
    canvas.paragraph(
        &[run(
            label.to_uppercase(),
            TextStyle::label(9.5, accent.ink()),
        )],
        CONTENT_WIDTH - 10.0,
        13.0,
        10.0,
    );
    // The 2.5pt accent tab is the visual anchor every section heading shares.
    canvas.overlay(
        MARGIN,
        top + 13.0 * BASELINE_RATIO - cap,
        2.5,
        cap,
        accent.mark(),
    );
    canvas.gap(6.0);
    canvas.rect(CONTENT_WIDTH, 0.8, RULE);
    canvas.gap(12.0);
}

fn draw_agenda_entry(canvas: &mut Canvas, item: &AgendaEntry) {
    let time_style = TextStyle::new(Font::Regular, 9.0, MUTED, 0.0);
    let label = item.time_label.as_deref().unwrap_or_default();
    // Reserve the timestamp column on every line, so a heading that wraps can
    // never run underneath the right-aligned time.
    let gutter = if label.is_empty() {
        0.0
    } else {
        text_width(label, &time_style) + 18.0
    };
    let lines = wrap(
        &[run(&item.heading, TextStyle::body(10.5))],
        CONTENT_WIDTH - BULLET_INDENT - gutter,
    );
    canvas.reserve(lines.len() as f64 * 15.0);
    canvas.dot(3.0, 8.8, 2.0, TEAL);
    for line in &lines {
        canvas.text_line(line, 15.0, BULLET_INDENT);
    }
    if let Some(time) = item.time_label.as_deref() {
        canvas.ops.push(Op::Text {
            x: MARGIN + CONTENT_WIDTH - text_width(time, &time_style),
            y: PAGE_HEIGHT - (canvas.cursor - 15.0 + 15.0 * BASELINE_RATIO),
            style: time_style,
            text: time.to_string(),
        });
    }
    canvas.gap(3.0);
}

fn draw_ledger_entry(canvas: &mut Canvas, entry: &LedgerEntry, accent: Accent) {
    let panel = accent == Accent::Open;
    let inset = if panel { PANEL_INSET } else { 0.0 };
    let column = CONTENT_WIDTH - BULLET_INDENT - inset * 2.0;
    let padding = if panel { 8.0 } else { 0.0 };

    let summary_lines = wrap(&[run(&entry.summary, TextStyle::body(11.0))], column);

    let mut meta: Vec<Run> = Vec::new();
    if let Some(owner) = &entry.owner {
        meta.push(run("OWNER  ", TextStyle::label(8.0, accent.ink())));
        meta.push(run(owner, TextStyle::body(9.0)));
    }
    if let Some(due) = &entry.due {
        if !meta.is_empty() {
            meta.push(run("     ·     ", TextStyle::body(9.0)));
        }
        meta.push(run("DUE  ", TextStyle::label(8.0, accent.ink())));
        meta.push(run(due, TextStyle::body(9.0)));
    }
    // `wrap` always returns at least one line, so an absent owner/due has to be
    // short-circuited or it would reserve a phantom line of vertical space.
    let meta_lines = if meta.is_empty() {
        Vec::new()
    } else {
        wrap(&meta, column)
    };

    let evidence_lines = entry.evidence.as_deref().map(|label| {
        wrap(
            &[run(
                format!("Evidence  {label}"),
                TextStyle::new(Font::Italic, 9.0, QUIET, 0.0),
            )],
            column,
        )
    });

    // Reserve the whole entry up front: a panel has to know its own height
    // before it can paint the wash behind the text.
    let total = summary_lines.len() as f64 * 15.5
        + if meta_lines.is_empty() {
            0.0
        } else {
            5.0 + meta_lines.len() as f64 * 12.5
        }
        + match &evidence_lines {
            Some(lines) => 3.0 + lines.len() as f64 * 12.0,
            None => 0.0,
        }
        + if panel { padding * 2.0 } else { 8.0 };
    canvas.reserve(total);

    if panel {
        let top = canvas.cursor;
        canvas.overlay(
            MARGIN - PANEL_BLEED,
            top,
            CONTENT_WIDTH + PANEL_BLEED * 2.0,
            total,
            AMBER_WASH,
        );
        canvas.overlay(MARGIN - PANEL_BLEED, top, 2.5, total, AMBER);
        canvas.cursor += padding;
    }

    canvas.dot(BULLET_INDENT + inset - 6.0, 9.2, 2.0, accent.mark());
    for line in &summary_lines {
        canvas.text_line(line, 15.5, BULLET_INDENT + inset);
    }
    if !meta_lines.is_empty() {
        canvas.gap(5.0);
        for line in &meta_lines {
            canvas.text_line(line, 12.5, BULLET_INDENT + inset);
        }
    }
    if let Some(lines) = &evidence_lines {
        canvas.gap(3.0);
        for line in lines {
            canvas.text_line(line, 12.0, BULLET_INDENT + inset);
        }
    }
    canvas.cursor += if panel { padding } else { 8.0 };
}

/// The running footer is painted after pagination because it reports the total.
fn add_footers(pages: &mut [Page]) {
    let total = pages.len();
    // PDF y grows upward, so the footer sits `FOOTER_BASELINE` points above the
    // bottom edge of the sheet, not below the content.
    let baseline = FOOTER_BASELINE;
    let brand = TextStyle::new(Font::Bold, 8.0, TEAL_INK, 1.2);
    let quiet = TextStyle::new(Font::Regular, 8.0, QUIET, 0.2);
    for (index, page) in pages.iter_mut().enumerate() {
        page.push(Op::Rect {
            x: MARGIN,
            y: baseline + 15.0,
            width: CONTENT_WIDTH,
            height: 0.6,
            color: RULE,
        });
        page.push(Op::Text {
            x: MARGIN,
            y: baseline,
            style: brand,
            text: BRAND_WORDMARK.to_string(),
        });
        page.push(Op::Text {
            x: MARGIN + text_width(BRAND_WORDMARK, &brand) + 6.0,
            y: baseline,
            style: quiet,
            text: format!("  ·  {FOOTER_KIND}"),
        });
        let label = format!("Page {} of {total}", index + 1);
        page.push(Op::Text {
            x: MARGIN + CONTENT_WIDTH - text_width(&label, &quiet),
            y: baseline,
            style: quiet,
            text: label,
        });
    }
}

fn render_ops(ops: &[Op]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    for op in ops {
        match op {
            Op::Rect {
                x,
                y,
                width,
                height,
                color,
            } => {
                out.extend_from_slice(color.fill_operator().as_bytes());
                out.extend_from_slice(
                    format!("{x:.2} {y:.2} {width:.2} {height:.2} re f\n").as_bytes(),
                );
            }
            Op::Circle {
                x,
                y,
                radius,
                color,
            } => {
                // Four Bézier arcs; 0.5523 is the circular-arc magic number.
                // Each arc needs three points, and adjacent arcs share theirs.
                let (x, y, radius) = (*x, *y, *radius);
                let k = radius * 0.552_284_749_8;
                let arc = [
                    (x + radius, y + k),
                    (x + k, y + radius),
                    (x, y + radius),
                    (x - k, y + radius),
                    (x - radius, y + k),
                    (x - radius, y - k),
                    (x - k, y - radius),
                    (x, y - radius),
                    (x + k, y - radius),
                    (x + radius, y - k),
                    (x + radius, y),
                    (x + radius, y + k),
                ];
                out.extend_from_slice(color.fill_operator().as_bytes());
                out.extend_from_slice(format!("{x:.2} {y:.2} m ").as_bytes());
                for (index, (px, py)) in arc.iter().enumerate() {
                    out.extend_from_slice(format!("{px:.2} {py:.2} ").as_bytes());
                    if index % 3 == 2 {
                        out.extend_from_slice(b"c ");
                    }
                }
                out.extend_from_slice(b"f\n");
            }
            Op::Text { x, y, style, text } => {
                // Text has to restate its fill colour: `rg` is part of the
                // graphics state, so a run would otherwise inherit whatever the
                // last painted shape left behind.
                out.extend_from_slice(style.color.fill_operator().as_bytes());
                out.extend_from_slice(b"BT ");
                out.extend_from_slice(
                    format!(
                        "/{} {:.2} Tf {:.2} Tc 1 0 0 1 {x:.2} {y:.2} Tm ",
                        style.font.resource(),
                        style.size,
                        style.tracking
                    )
                    .as_bytes(),
                );
                out.push(b'(');
                for character in text.chars() {
                    match win_ansi(character) {
                        b'\\' => out.extend_from_slice(b"\\\\"),
                        b'(' => out.extend_from_slice(b"\\("),
                        b')' => out.extend_from_slice(b"\\)"),
                        other => out.push(other),
                    }
                }
                out.extend_from_slice(b") Tj ET\n");
            }
        }
    }
    out
}

fn serialize_pdf(mut pages: Vec<Page>) -> Vec<u8> {
    add_footers(&mut pages);
    let page_count = pages.len().max(1);
    let font_regular = 3 + page_count * 2;
    let resources = format!(
        "<< /Font << /F1 {font_regular} 0 R /F2 {} 0 R /F3 {} 0 R >> >>",
        font_regular + 1,
        font_regular + 2
    );
    let kids: Vec<String> = (0..page_count)
        .map(|page| format!("{} 0 R", 3 + page * 2))
        .collect();

    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!(
            "<< /Type /Pages /Kids [{}] /Count {page_count} >>",
            kids.join(" ")
        )
        .into_bytes(),
    ];
    for (index, ops) in pages.iter().enumerate() {
        let content = render_ops(ops);
        objects.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAGE_WIDTH} {PAGE_HEIGHT}] /Resources {resources} /Contents {} 0 R >>",
                4 + index * 2
            )
            .into_bytes(),
        );
        let mut stream = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
        stream.extend_from_slice(&content);
        stream.extend_from_slice(b"\nendstream");
        objects.push(stream);
    }
    for face in ["Helvetica", "Helvetica-Bold", "Helvetica-Oblique"] {
        objects.push(
            format!(
                "<< /Type /Font /Subtype /Type1 /BaseFont /{face} /Encoding /WinAnsiEncoding >>"
            )
            .into_bytes(),
        );
    }

    let mut pdf: Vec<u8> = b"%PDF-1.4\n".to_vec();
    let mut offsets: Vec<usize> = Vec::with_capacity(objects.len());
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        pdf.extend_from_slice(object);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for offset in &offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF",
            objects.len() + 1
        )
        .as_bytes(),
    );
    pdf
}

/// Renders the minutes as a branded, paginated PDF.
pub fn render_pdf(doc: &ReportDoc) -> Vec<u8> {
    serialize_pdf(draw_document(doc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Evidence, Minutes};

    fn sample() -> Minutes {
        Minutes {
            title: "Q3 Roadmap Sync".into(),
            summary: "The team locked the launch date.\nAnd split the migration work.".into(),
            agenda: vec![AgendaItem {
                heading: "Roadmap review".into(),
                start_seconds: Some(0),
                end_seconds: Some(320),
            }],
            visual_observations: Vec::new(),
            decisions: vec![LedgerEvent {
                kind: "decision".into(),
                summary: "Ship the beta on the 14th.".into(),
                owner: None,
                due: None,
                confidence: 0.9,
                evidence: vec![Evidence {
                    start_seconds: 61,
                    end_seconds: 70,
                    quote: "we ship on the 14th".into(),
                    title: String::new(),
                }],
            }],
            action_items: vec![LedgerEvent {
                kind: "action".into(),
                summary: "Prepare the migration plan".into(),
                owner: Some("John Reyes".into()),
                due: Some("Fri 12 Sep".into()),
                confidence: 0.8,
                evidence: vec![],
            }],
            unresolved: vec![LedgerEvent {
                kind: "open".into(),
                summary: "Pricing tier for the EU launch is undecided.".into(),
                owner: None,
                due: None,
                confidence: 0.4,
                evidence: vec![],
            }],
        }
    }

    #[test]
    fn report_model_normalises_model_output_whitespace() {
        let doc = ReportDoc::from_minutes(&sample());
        assert_eq!(doc.title, "Q3 Roadmap Sync");
        assert!(
            !doc.summary.contains('\n'),
            "hard breaks leak into the layout"
        );
        assert_eq!(doc.agenda[0].time_label.as_deref(), Some("00:00–05:20"));
        assert_eq!(
            doc.sections[0].entries[0].evidence.as_deref(),
            Some("01:01–01:10")
        );
    }

    #[test]
    fn empty_sections_are_dropped_and_open_items_keep_the_amber_tone() {
        let doc = ReportDoc::from_minutes(&sample());
        let labels: Vec<&str> = doc.sections.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, ["Decisions", "Action Items", "Unresolved Matters"]);
        assert_eq!(doc.sections[2].accent, Accent::Open);
        assert_eq!(doc.sections[0].accent, Accent::Settled);
    }

    #[test]
    fn pdf_carries_the_brand_chrome_and_no_raw_markdown() {
        let pdf = render_pdf(&ReportDoc::from_minutes(&sample()));
        assert!(pdf.starts_with(b"%PDF-1.4"));
        assert!(pdf.ends_with(b"%%EOF"));
        let text = String::from_utf8_lossy(&pdf);
        assert!(text.contains("BEA"), "masthead wordmark missing");
        assert!(text.contains("MEETING MINUTES"), "document kind missing");
        assert!(text.contains("Q3 Roadmap Sync"), "title missing");
        assert!(text.contains("Page 1 of 1"), "footer pagination missing");
        assert!(!text.contains("# Minutes"), "raw markdown leaked in");
        assert!(text.contains("/WinAnsiEncoding"), "font encoding missing");
    }

    #[test]
    fn pdf_paginates_long_minutes_and_keeps_every_entry() {
        let mut long = sample();
        long.action_items = (0..90)
            .map(|index| LedgerEvent {
                kind: "action".into(),
                summary: format!("Action item number {index} with a fairly long description"),
                owner: Some("Owner With A Long Name".into()),
                due: Some("Some future date".into()),
                confidence: 0.7,
                evidence: vec![Evidence {
                    start_seconds: index,
                    end_seconds: index + 5,
                    quote: "q".into(),
                    title: String::new(),
                }],
            })
            .collect();
        let bytes = render_pdf(&ReportDoc::from_minutes(&long));
        let text = String::from_utf8_lossy(&bytes);
        let pages = text.matches("/Type /Page ").count();
        assert!(pages > 1, "expected multiple pages, got {pages}");
        assert!(
            text.contains("Action item number 89"),
            "the last entry was dropped"
        );
        assert!(
            text.contains(&format!("Page 2 of {pages}")),
            "footer does not track the page count"
        );
    }

    /// The object graph and xref table are written by hand, so the numbering is
    /// worth pinning down: every declared offset must land exactly on its
    /// `N 0 obj` header. The other tests only substring-match, so an off-by-one
    /// here would sail past them.
    ///
    /// This works on raw bytes on purpose. The content streams carry WinAnsi
    /// high bytes (an en dash is 0x96), which are not valid UTF-8, so going via
    /// `from_utf8_lossy` would shift every offset after the first one.
    #[test]
    fn pdf_object_table_is_internally_consistent() {
        fn count(haystack: &[u8], needle: &[u8]) -> usize {
            haystack
                .windows(needle.len())
                .filter(|window| *window == needle)
                .count()
        }
        for entries in [1usize, 2, 7, 40] {
            let mut minutes = sample();
            minutes.action_items = (0..entries)
                .map(|index| LedgerEvent {
                    kind: "action".into(),
                    summary: format!("Action item number {index} with a description"),
                    owner: Some("Owner With A Long Name".into()),
                    due: Some("Some future date".into()),
                    confidence: 0.7,
                    evidence: vec![],
                })
                .collect();
            let bytes = render_pdf(&ReportDoc::from_minutes(&minutes));

            let page_count = count(&bytes, b"/Type /Page ");
            assert!(page_count > 0, "no pages emitted");
            let object_count = 2 + page_count * 2 + 3;
            assert_eq!(
                count(&bytes, b" 0 obj"),
                object_count,
                "expected {object_count} objects for {page_count} pages"
            );

            // startxref must address the xref table itself.
            let tail = b"startxref\n";
            let start = bytes
                .windows(tail.len())
                .rposition(|window| window == tail)
                .expect("no startxref");
            let digits: Vec<u8> = bytes[start + tail.len()..]
                .iter()
                .copied()
                .take_while(|byte| byte.is_ascii_digit())
                .collect();
            let xref_at: usize = std::str::from_utf8(&digits)
                .expect("startxref value is not ascii")
                .parse()
                .expect("startxref value is not a number");
            assert_eq!(
                &bytes[xref_at..xref_at + 4],
                b"xref",
                "startxref points at {:?}, not the xref table",
                &bytes[xref_at..bytes.len().min(xref_at + 12)]
            );

            // "xref\n0 <n>\n" then one 20-byte row per entry, each laid out as
            // 10-digit offset, space, 5-digit generation, space, type, space, LF.
            let header = format!("xref\n0 {}\n", object_count + 1);
            assert!(
                bytes[xref_at..].starts_with(header.as_bytes()),
                "xref subsection header is wrong"
            );
            let rows_at = xref_at + header.len();
            for index in 0..=object_count {
                let row = &bytes[rows_at + index * 20..rows_at + (index + 1) * 20];
                assert_eq!(row.len(), 20, "xref row {index} is not 20 bytes");
                assert_eq!(
                    (row[10], row[16], row[18], row[19]),
                    (b' ', b' ', b' ', b'\n'),
                    "xref row {index} has the wrong field layout: {:?}",
                    String::from_utf8_lossy(row)
                );
                assert!(
                    row[11..16].iter().all(u8::is_ascii_digit),
                    "xref row {index} has a non-numeric generation: {:?}",
                    String::from_utf8_lossy(row)
                );
                let kind = row[17];
                assert!(
                    kind == b'n' || kind == b'f',
                    "xref row {index} has entry type {:?}",
                    kind as char
                );
                if index == 0 {
                    assert_eq!(kind, b'f', "the first entry must be the free head");
                    continue;
                }
                assert_eq!(kind, b'n', "object {index} must be an in-use entry");
                let offset: usize = std::str::from_utf8(&row[..10])
                    .expect("offset is not ascii")
                    .trim()
                    .parse()
                    .expect("offset is not a number");
                let header = format!("{} 0 obj", index);
                assert!(
                    bytes[offset..].starts_with(header.as_bytes()),
                    "xref entry {index} points at {:?}, not {header}",
                    &bytes[offset..bytes.len().min(offset + 12)]
                );
            }

            // Each face is written as "/BaseFont /<face> /Encoding", so the
            // trailing space is what stops "Helvetica" matching inside
            // "Helvetica-Bold".
            for face in ["Helvetica", "Helvetica-Bold", "Helvetica-Oblique"] {
                let needle = format!("/BaseFont /{face} ").into_bytes();
                assert!(
                    bytes
                        .windows(needle.len())
                        .any(|window| window == needle.as_slice()),
                    "{face} is not embedded"
                );
            }
        }
    }

    #[test]
    fn wrapping_respects_the_column_width() {
        let lines = wrap(
            &[run(
                "Supercalifragilistic meeting notes that must wrap somewhere sensible",
                TextStyle::body(11.0),
            )],
            200.0,
        );
        assert!(lines.len() > 1, "long text must wrap");
        for line in &lines {
            assert!(
                line.width <= 200.5,
                "a {}pt line overflowed a 200pt column",
                line.width
            );
        }
    }

    #[test]
    fn metrics_handle_accents_and_typography() {
        let style = TextStyle::body(11.0);
        assert!((text_width("a–b", &style) - text_width("a-b", &style)).abs() < 5.0);
        assert!((text_width("“x”", &style) - text_width("\"x\"", &style)).abs() < 5.0);
        // Accented Latin-1 letters measure as their plain counterpart.
        assert!((text_width("ñ", &style) - text_width("n", &style)).abs() < 0.01);
    }
}
