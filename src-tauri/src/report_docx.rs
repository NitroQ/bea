//! Branded Word (OOXML) rendering for meeting minutes.
//!
//! Consumes the same [`ReportDoc`](crate::report::ReportDoc) as the PDF writer.
//! Everything is expressed with real Word parts and real run properties rather
//! than hand-placed shapes, so the recipient can restyle the file.

use crate::report::{
    Color, ReportDoc, BODY, BRAND_WORDMARK, CONTENT_WIDTH, DOCUMENT_KIND, EMPTY_NOTE, INK, MUTED,
    PAPER, QUIET, RULE, TEAL, TEAL_INK,
};
use crate::BeaError;
use std::io::{Cursor, Write};
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

const TWIPS_PER_POINT: f64 = 20.0;

fn points(value: f64) -> i64 {
    (value * TWIPS_PER_POINT).round() as i64
}

/// Escapes text for an XML text node, and drops the characters XML 1.0 forbids.
///
/// Substituting entities alone is not enough. The Char production only permits
/// #x9, #xA, #xD, [#x20-#xD7FF], [#xE000-#xFFFD] and [#x10000-#x10FFFF], so a
/// stray C0 control from provider output — a literal U+0001 inside a minutes
/// JSON string decodes to one and `flatten` keeps it — would land raw in
/// document.xml and make Word refuse the whole package with no hint why.
/// Filtering here covers every part at once, since all of them go through this.
fn escape_xml(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        let code = character as u32;
        let legal = matches!(character, '\t' | '\n' | '\r')
            || (0x20..=0xD7FF).contains(&code)
            || (0xE000..=0xFFFD).contains(&code)
            || (0x10000..=0x10FFFF).contains(&code);
        if !legal {
            continue;
        }
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

/// Character formatting for one run of Word text. Element order inside `w:rPr`
/// and `w:pPr` is fixed by the OOXML schema, so these two builders are the only
/// place that knows the sequence.
#[derive(Debug, Clone, Copy)]
struct CharStyle {
    size: f64,
    color: Color,
    bold: bool,
    italic: bool,
    caps: bool,
    tracking: f64,
}

impl CharStyle {
    const fn body(size: f64) -> Self {
        Self {
            size,
            color: BODY,
            bold: false,
            italic: false,
            caps: false,
            tracking: 0.0,
        }
    }

    const fn muted(size: f64) -> Self {
        Self {
            size,
            color: MUTED,
            bold: false,
            italic: false,
            caps: false,
            tracking: 0.0,
        }
    }

    const fn evidence(size: f64) -> Self {
        Self {
            size,
            color: QUIET,
            bold: false,
            italic: true,
            caps: false,
            tracking: 0.2,
        }
    }

    /// Small tracked caps — a section heading, the label under an entry, the
    /// strapline in the masthead.
    const fn label(size: f64, color: Color) -> Self {
        Self {
            size,
            color,
            bold: true,
            italic: false,
            caps: true,
            tracking: 0.9,
        }
    }

    const fn title() -> Self {
        Self {
            size: 21.0,
            color: INK,
            bold: true,
            italic: false,
            caps: false,
            tracking: -0.4,
        }
    }

    const fn wordmark(size: f64, tracking: f64) -> Self {
        Self {
            size,
            color: PAPER,
            bold: false,
            italic: false,
            caps: true,
            tracking,
        }
    }

    fn run_properties(self) -> String {
        let mut properties =
            String::from("<w:rPr><w:rFonts w:ascii=\"Arial\" w:hAnsi=\"Arial\" w:cs=\"Arial\"/>");
        if self.bold {
            properties.push_str("<w:b/>");
        }
        if self.italic {
            properties.push_str("<w:i/>");
        }
        if self.caps {
            properties.push_str("<w:caps/>");
        }
        properties.push_str(&format!("<w:color w:val=\"{}\"/>", self.color.hex()));
        // w:spacing is ST_SignedTwipsMeasure (twentieths of a point), but w:sz is
        // ST_HpsMeasure — HALF-points. Running `points()` on the size made every
        // run ten times too large: a 10.5pt body emitted w:sz="210" = 105pt.
        // The hand-written docDefaults and footer below use half-points, which is
        // what made the inconsistency visible.
        properties.push_str(&format!("<w:spacing w:val=\"{}\"/>", points(self.tracking)));
        let half_points = (self.size * 2.0).round() as i64;
        properties.push_str(&format!(
            "<w:sz w:val=\"{half_points}\"/><w:szCs w:val=\"{half_points}\"/></w:rPr>"
        ));
        properties
    }
}

struct Span {
    text: String,
    style: CharStyle,
}

impl Span {
    fn to_xml(&self) -> String {
        format!(
            "<w:r>{}<w:t xml:space=\"preserve\">{}</w:t></w:r>",
            self.style.run_properties(),
            escape_xml(&self.text)
        )
    }
}

fn span(text: impl Into<String>, style: CharStyle) -> Span {
    Span {
        text: text.into(),
        style,
    }
}

struct Block {
    spans: Vec<Span>,
    before: i64,
    after: i64,
    bullet: bool,
    indent: Option<i64>,
    accent_rule: Option<Color>,
}

impl Block {
    fn new(spans: Vec<Span>) -> Self {
        Self {
            spans,
            before: 0,
            after: 120,
            bullet: false,
            indent: None,
            accent_rule: None,
        }
    }

    fn spacing(mut self, before: i64, after: i64) -> Self {
        self.before = before;
        self.after = after;
        self
    }

    fn bullet(mut self, indent: i64) -> Self {
        self.bullet = true;
        self.indent = Some(indent);
        self
    }

    /// Indent a continuation line under its parent entry.
    fn hanging(mut self, indent: i64) -> Self {
        self.indent = Some(indent);
        self
    }

    /// A rule has to be a real paragraph, not a drawn line, or the recipient
    /// cannot move or recolour it.
    fn rule(color: Color) -> Self {
        let mut block = Self::new(Vec::new()).spacing(0, 140);
        block.accent_rule = Some(color);
        block
    }

    fn to_xml(&self) -> String {
        let mut properties = String::new();
        if self.bullet {
            properties.push_str("<w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"1\"/></w:numPr>");
        }
        if let Some(color) = self.accent_rule {
            properties.push_str(&format!(
                "<w:pBdr><w:bottom w:val=\"single\" w:sz=\"6\" w:space=\"6\" w:color=\"{}\"/></w:pBdr>",
                color.hex()
            ));
        }
        properties.push_str(&format!(
            "<w:spacing w:before=\"{}\" w:after=\"{}\" w:line=\"{}\" w:lineRule=\"auto\"/>",
            self.before,
            self.after,
            points(13.2)
        ));
        if let Some(indent) = self.indent {
            if self.bullet {
                properties.push_str(&format!(
                    "<w:ind w:left=\"{indent}\" w:hanging=\"{indent}\"/>"
                ));
            } else {
                properties.push_str(&format!("<w:ind w:left=\"{indent}\"/>"));
            }
        }
        let runs: String = self.spans.iter().map(Span::to_xml).collect();
        format!("<w:p><w:pPr>{properties}</w:pPr>{runs}</w:p>")
    }
}

/// A one-cell teal band. A shaded table is the only construct Word keeps a solid
/// brand colour across a reflow, a re-layout, or a paste into another document.
fn masthead_table() -> String {
    let runs = [
        span(BRAND_WORDMARK, CharStyle::label(10.0, PAPER)),
        span(format!("   {DOCUMENT_KIND}"), CharStyle::wordmark(8.0, 1.4)),
    ]
    .iter()
    .map(Span::to_xml)
    .collect::<String>();
    format!(
        concat!(
            "<w:tbl><w:tblPr><w:tblW w:w=\"5000\" w:type=\"pct\"/>",
            "<w:tblBorders><w:top w:val=\"nil\"/><w:left w:val=\"nil\"/><w:bottom w:val=\"nil\"/>",
            "<w:right w:val=\"nil\"/><w:insideH w:val=\"nil\"/><w:insideV w:val=\"nil\"/></w:tblBorders>",
            "<w:tblCellMar><w:top w:w=\"240\" w:type=\"dxa\"/><w:left w:w=\"220\" w:type=\"dxa\"/>",
            "<w:bottom w:w=\"240\" w:type=\"dxa\"/><w:right w:w=\"220\" w:type=\"dxa\"/></w:tblCellMar></w:tblPr>",
            "<w:tblGrid><w:gridCol w:w=\"{grid}\"/></w:tblGrid>",
            "<w:tr><w:tc><w:tcPr><w:tcW w:w=\"5000\" w:type=\"pct\"/>",
            "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{fill}\"/></w:tcPr>",
            "<w:p><w:pPr><w:spacing w:before=\"0\" w:after=\"0\"/></w:pPr>{runs}</w:p></w:tc></w:tr></w:tbl>",
            "<w:p><w:pPr><w:spacing w:before=\"0\" w:after=\"0\"/></w:pPr></w:p>"
        ),
        grid = points(CONTENT_WIDTH),
        fill = TEAL.hex(),
        runs = runs,
    )
}

fn document_body(doc: &ReportDoc) -> Vec<String> {
    let mut blocks = vec![masthead_table()];
    blocks.push(Block::new(vec![span(&doc.title, CharStyle::title())]).to_xml());

    if !doc.summary.is_empty() {
        blocks.push(
            Block::new(vec![span(&doc.summary, CharStyle::body(10.5))])
                .spacing(200, 200)
                .to_xml(),
        );
    }
    blocks.push(Block::rule(TEAL).spacing(0, 200).to_xml());

    if !doc.agenda.is_empty() {
        blocks.push(
            Block::new(vec![span("Agenda", CharStyle::label(9.5, TEAL_INK))])
                .spacing(260, 40)
                .to_xml(),
        );
        blocks.push(Block::rule(RULE).to_xml());
        for item in &doc.agenda {
            let mut spans = vec![span(&item.heading, CharStyle::body(10.5))];
            if let Some(time) = &item.time_label {
                spans.push(span(format!("    {time}"), CharStyle::muted(9.0)));
            }
            blocks.push(Block::new(spans).bullet(points(16.0)).to_xml());
        }
    }

    let hanging = points(16.0);
    for section in &doc.sections {
        blocks.push(
            Block::new(vec![span(
                &section.label,
                CharStyle::label(9.5, section.accent.ink()),
            )])
            .spacing(300, 40)
            .to_xml(),
        );
        blocks.push(Block::rule(RULE).to_xml());
        for entry in &section.entries {
            let has_meta = entry.owner.is_some() || entry.due.is_some();
            blocks.push(
                Block::new(vec![span(&entry.summary, CharStyle::body(10.5))])
                    .bullet(hanging)
                    .spacing(0, if has_meta { 40 } else { 140 })
                    .to_xml(),
            );
            let mut meta: Vec<Span> = Vec::new();
            if let Some(owner) = &entry.owner {
                meta.push(span("Owner  ", CharStyle::label(8.0, section.accent.ink())));
                meta.push(span(owner, CharStyle::muted(9.0)));
            }
            if let Some(due) = &entry.due {
                if !meta.is_empty() {
                    meta.push(span("     ·     ", CharStyle::muted(9.0)));
                }
                meta.push(span("Due  ", CharStyle::label(8.0, section.accent.ink())));
                meta.push(span(due, CharStyle::muted(9.0)));
            }
            if !meta.is_empty() {
                blocks.push(Block::new(meta).hanging(hanging).spacing(0, 40).to_xml());
            }
            if let Some(evidence) = &entry.evidence {
                blocks.push(
                    Block::new(vec![span(
                        format!("Evidence  {evidence}"),
                        CharStyle::evidence(9.0),
                    )])
                    .hanging(hanging)
                    .spacing(0, 140)
                    .to_xml(),
                );
            }
        }
    }

    if !doc.has_body() {
        blocks.push(
            Block::new(vec![span(EMPTY_NOTE, CharStyle::evidence(10.0))])
                .spacing(240, 0)
                .to_xml(),
        );
    }
    blocks
}

const CONTENT_TYPES: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>",
    "<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">",
    "<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>",
    "<Default Extension=\"xml\" ContentType=\"application/xml\"/>",
    "<Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>",
    "<Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>",
    "<Override PartName=\"/word/numbering.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml\"/>",
    "<Override PartName=\"/word/footer1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml\"/>",
    "<Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/>",
    "<Override PartName=\"/docProps/app.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.extended-properties+xml\"/>",
    "</Types>"
);

const PACKAGE_RELS: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>",
    "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
    "<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/>",
    "<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/>",
    "<Relationship Id=\"rId3\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties\" Target=\"docProps/app.xml\"/>",
    "</Relationships>"
);

const DOCUMENT_RELS: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>",
    "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
    "<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>",
    "<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering\" Target=\"numbering.xml\"/>",
    "<Relationship Id=\"rId3\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer\" Target=\"footer1.xml\"/>",
    "</Relationships>"
);

const STYLES: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>",
    "<w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">",
    "<w:docDefaults><w:rPrDefault><w:rPr>",
    "<w:rFonts w:ascii=\"Arial\" w:hAnsi=\"Arial\" w:cs=\"Arial\"/>",
    "<w:color w:val=\"3D4A57\"/><w:spacing w:val=\"0\"/><w:sz w:val=\"21\"/><w:szCs w:val=\"21\"/>",
    "</w:rPr></w:rPrDefault><w:pPrDefault><w:pPr>",
    "<w:spacing w:after=\"120\" w:line=\"264\" w:lineRule=\"auto\"/>",
    "</w:pPr></w:pPrDefault></w:docDefaults>",
    "<w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/>",
    "<w:pPr><w:spacing w:after=\"120\" w:line=\"264\" w:lineRule=\"auto\"/></w:pPr></w:style>",
    "<w:style w:type=\"table\" w:default=\"1\" w:styleId=\"TableNormal\"><w:name w:val=\"Normal Table\"/>",
    "<w:tblPr><w:tblCellMar><w:top w:w=\"0\" w:type=\"dxa\"/><w:left w:w=\"108\" w:type=\"dxa\"/>",
    "<w:bottom w:w=\"0\" w:type=\"dxa\"/><w:right w:w=\"108\" w:type=\"dxa\"/></w:tblCellMar></w:tblPr></w:style>",
    "<w:style w:type=\"numbering\" w:default=\"1\" w:styleId=\"NoList\"><w:name w:val=\"No List\"/></w:style>",
    "</w:styles>"
);

const NUMBERING: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>",
    "<w:numbering xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">",
    "<w:abstractNum w:abstractNumId=\"0\"><w:multiLevelType w:val=\"hybridMultilevel\"/>",
    "<w:lvl w:ilvl=\"0\"><w:start w:val=\"1\"/><w:numFmt w:val=\"bullet\"/><w:lvlText w:val=\"&#8226;\"/>",
    "<w:lvlJc w:val=\"left\"/><w:pPr><w:ind w:left=\"320\" w:hanging=\"320\"/></w:pPr>",
    "<w:rPr><w:rFonts w:ascii=\"Arial\" w:hAnsi=\"Arial\" w:hint=\"default\"/>",
    "<w:color w:val=\"17B08A\"/></w:rPr>",
    "</w:lvl></w:abstractNum>",
    "<w:num w:numId=\"1\"><w:abstractNumId w:val=\"0\"/></w:num>",
    "</w:numbering>"
);

const APP_PROPERTIES: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>",
    "<Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\" ",
    "xmlns:vt=\"http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes\">",
    "<Application>Bea</Application></Properties>"
);

fn core_properties(title: &str) -> String {
    format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>",
            "<cp:coreProperties ",
            "xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" ",
            "xmlns:dc=\"http://purl.org/dc/elements/1.1/\">",
            "<dc:title>{} — Meeting minutes</dc:title>",
            "<dc:creator>Bea</dc:creator><cp:lastModifiedBy>Bea</cp:lastModifiedBy>",
            "</cp:coreProperties>"
        ),
        escape_xml(title)
    )
}

/// Running footer: brand on the left, live page fields on the right.
fn footer_xml() -> String {
    let field = |instruction: &str| {
        format!(
            "<w:fldSimple w:instr=\"{instruction}\"><w:r><w:rPr><w:color w:val=\"{}\"/><w:sz w:val=\"16\"/></w:rPr><w:t>1</w:t></w:r></w:fldSimple>",
            QUIET.hex()
        )
    };
    let text = |value: &str, color: Color, bold: bool| {
        format!(
            "<w:r><w:rPr>{}<w:color w:val=\"{}\"/><w:sz w:val=\"16\"/></w:rPr><w:t xml:space=\"preserve\">{}</w:t></w:r>",
            if bold { "<w:b/>" } else { "" },
            color.hex(),
            escape_xml(value)
        )
    };
    format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>",
            "<w:ftr xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">",
            "<w:p><w:pPr>",
            "<w:pBdr><w:top w:val=\"single\" w:sz=\"4\" w:space=\"6\" w:color=\"{rule}\"/></w:pBdr>",
            "<w:tabs><w:tab w:val=\"right\" w:pos=\"{tab}\"/></w:tabs>",
            "<w:spacing w:before=\"0\" w:after=\"0\"/></w:pPr>",
            "{brand}{kind}",
            "<w:r><w:rPr><w:color w:val=\"{quiet}\"/><w:sz w:val=\"16\"/></w:rPr><w:tab/></w:r>",
            "{page}{current}{of}{pages}",
            "</w:p></w:ftr>"
        ),
        rule = RULE.hex(),
        tab = points(CONTENT_WIDTH),
        brand = text(BRAND_WORDMARK, TEAL_INK, true),
        kind = text("  ·  Meeting minutes", QUIET, false),
        quiet = QUIET.hex(),
        page = text("Page ", QUIET, false),
        current = field(" PAGE "),
        of = text(" of ", QUIET, false),
        pages = field(" NUMPAGES "),
    )
}

fn write_part(
    zip: &mut ZipWriter<&mut Cursor<Vec<u8>>>,
    name: &str,
    body: &[u8],
) -> Result<(), BeaError> {
    zip.start_file(name, SimpleFileOptions::default())
        .map_err(|error| BeaError::InvalidState(error.to_string()))?;
    zip.write_all(body)
        .map_err(|error| BeaError::InvalidState(error.to_string()))
}

/// Renders the minutes as a branded Word document using real OOXML parts, so
/// the recipient can restyle it instead of inheriting hand-applied formatting.
pub fn render_docx(doc: &ReportDoc) -> Result<Vec<u8>, BeaError> {
    let document = format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>",
            "<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" ",
            "xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">",
            "<w:body>{body}",
            "<w:sectPr><w:footerReference w:type=\"default\" r:id=\"rId3\"/>",
            "<w:pgSz w:w=\"12240\" w:h=\"15840\"/>",
            "<w:pgMar w:top=\"1120\" w:right=\"1160\" w:bottom=\"1160\" w:left=\"1160\" ",
            "w:header=\"720\" w:footer=\"720\" w:gutter=\"0\"/>",
            "</w:sectPr></w:body></w:document>"
        ),
        body = document_body(doc).join("")
    );

    let mut cursor = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(&mut cursor);
    write_part(&mut zip, "[Content_Types].xml", CONTENT_TYPES.as_bytes())?;
    write_part(&mut zip, "_rels/.rels", PACKAGE_RELS.as_bytes())?;
    write_part(
        &mut zip,
        "docProps/core.xml",
        core_properties(&doc.title).as_bytes(),
    )?;
    write_part(&mut zip, "docProps/app.xml", APP_PROPERTIES.as_bytes())?;
    write_part(
        &mut zip,
        "word/_rels/document.xml.rels",
        DOCUMENT_RELS.as_bytes(),
    )?;
    write_part(&mut zip, "word/styles.xml", STYLES.as_bytes())?;
    write_part(&mut zip, "word/numbering.xml", NUMBERING.as_bytes())?;
    write_part(&mut zip, "word/footer1.xml", footer_xml().as_bytes())?;
    write_part(&mut zip, "word/document.xml", document.as_bytes())?;
    zip.finish()
        .map_err(|error| BeaError::InvalidState(error.to_string()))?;
    Ok(cursor.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{AgendaEntry, LedgerEntry, ReportSection};
    use crate::{AgendaItem, Evidence, LedgerEvent, Minutes};
    use zip::ZipArchive;

    /// A real `Minutes`, so the `ReportDoc::from_minutes` path is exercised
    /// rather than bypassed by hand-building the layout model.
    fn sample_minutes() -> Minutes {
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

    fn doc() -> ReportDoc {
        ReportDoc::from_minutes(&sample_minutes())
    }

    fn part(bytes: Vec<u8>, name: &str) -> String {
        let mut archive = ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut body = String::new();
        use std::io::Read;
        archive
            .by_name(name)
            .unwrap()
            .read_to_string(&mut body)
            .unwrap();
        body
    }

    #[test]
    fn docx_package_carries_every_relationship_it_declares() {
        let bytes = render_docx(&doc()).unwrap();
        let mut archive = ZipArchive::new(std::io::Cursor::new(bytes.clone())).unwrap();
        let entries: Vec<String> = archive.file_names().map(str::to_owned).collect();

        // Every relationship Target must resolve to an entry that is actually
        // in the package, relative to the part that declares it. A dangling
        // target is what puts Word's "unreadable content" dialog on screen.
        for (rels_part, base) in [
            ("_rels/.rels", ""),
            ("word/_rels/document.xml.rels", "word/"),
        ] {
            let mut body = String::new();
            use std::io::Read;
            archive
                .by_name(rels_part)
                .unwrap_or_else(|_| panic!("{rels_part} is missing"))
                .read_to_string(&mut body)
                .unwrap();
            let targets: Vec<&str> = body
                .match_indices("Target=\"")
                .map(|(index, _)| {
                    let tail = &body[index + "Target=\"".len()..];
                    &tail[..tail.find('"').expect("unterminated Target")]
                })
                .collect();
            assert!(!targets.is_empty(), "{rels_part} declares no targets");
            for target in targets {
                let resolved = format!("{base}{target}").replace("//", "/");
                assert!(
                    entries.iter().any(|entry| entry == &resolved),
                    "{rels_part}: Target {target} resolves to {resolved}, which is not in the package"
                );
            }
        }

        // Conversely, every real part must be declared by content type, or Word
        // will not know what it is.
        let types = part(bytes.clone(), "[Content_Types].xml");
        for entry in &entries {
            if entry.ends_with(".rels") {
                continue;
            }
            let extension = entry.rsplit('.').next().unwrap_or_default();
            let declared = types.contains(&format!("Extension=\"{extension}\""))
                || types.contains(&format!("PartName=\"/{entry}\""));
            assert!(declared, "{entry} has no content type declared");
        }
        assert!(part(bytes, "word/document.xml").contains("<w:footerReference"));
    }

    #[test]
    fn docx_applies_bea_branding_and_no_raw_markdown() {
        // Built from a real Minutes, not a hand-made ReportDoc, so the
        // markdown-leak assertion below actually exercises the path it claims
        // to guard: if anyone reintroduced export_markdown into the renderer,
        // these headings would come back as literal "# Minutes".
        let mut minutes = sample_minutes();
        minutes.title = "Q3 Roadmap Sync".into();
        let document = part(
            render_docx(&ReportDoc::from_minutes(&minutes)).unwrap(),
            "word/document.xml",
        );
        // The teal masthead band.
        assert!(document.contains(&format!("w:fill=\"{}\"", TEAL.hex())));
        assert!(document.contains(BRAND_WORDMARK));
        assert!(document.contains(DOCUMENT_KIND));
        // Real run properties, not a wall of plain paragraphs.
        assert!(document.contains("<w:caps/>"));
        assert!(document.contains("<w:numPr>"));
        assert!(document.contains(&format!("w:color w:val=\"{}\"", TEAL_INK.hex())));
        assert!(!document.contains("# Minutes"), "raw markdown leaked in");
        assert!(document.contains("John Reyes"));
        assert!(document.contains("Evidence  01:01"));
    }

    /// Regression guard for the unit mix-up that made every run ten times too
    /// large: `w:sz` is half-points, not twips. 10.5pt must emit 21.
    #[test]
    fn docx_font_sizes_are_half_points() {
        let document = part(render_docx(&doc()).unwrap(), "word/document.xml");
        assert!(
            document.contains("<w:sz w:val=\"21\"/><w:szCs w:val=\"21\"/>"),
            "a 10.5pt body style must emit w:sz=21 half-points"
        );
        assert!(
            !document.contains("<w:sz w:val=\"210\"/>"),
            "w:sz is half-points; 210 would be 105pt"
        );
        // Letter-spacing really is twips, so tracking must still be scaled by 20.
        let tracked = CharStyle::label(10.0, TEAL_INK);
        assert!(tracked
            .run_properties()
            .contains("<w:spacing w:val=\"18\"/>"));
    }

    /// A C0 control from provider output is not a legal XML character, and
    /// `flatten` keeps it. Word rejects the whole package if it reaches a part.
    #[test]
    fn docx_drops_characters_xml_forbids() {
        let mut hostile = doc();
        hostile.title = "Launch\u{1}date\u{b}approved".into();
        hostile.summary = "Budget \u{0}frozen \u{1f}for Q3".into();
        let document = part(render_docx(&hostile).unwrap(), "word/document.xml");
        for illegal in ['\u{0}', '\u{1}', '\u{b}', '\u{1f}'] {
            assert!(
                !document.contains(illegal),
                "U+{:04X} must not reach document.xml",
                illegal as u32
            );
        }
        assert!(document.contains("Launchdateapproved"));
        // Tab, newline and carriage return stay legal and must survive.
        assert_eq!(escape_xml("a\tb\nc\rd"), "a\tb\nc\rd");
    }

    #[test]
    fn docx_escapes_markup_in_generated_text() {
        let mut hostile = doc();
        hostile.title = "Budget <Q3> & \"launch\"".into();
        let document = part(render_docx(&hostile).unwrap(), "word/document.xml");
        assert!(document.contains("Budget &lt;Q3&gt; &amp; &quot;launch&quot;"));
    }

    #[test]
    fn docx_footer_uses_live_page_fields() {
        let footer = part(render_docx(&doc()).unwrap(), "word/footer1.xml");
        assert!(footer.contains(" PAGE "));
        assert!(footer.contains(" NUMPAGES "));
        assert!(footer.contains(BRAND_WORDMARK));
    }

    /// The OOXML parts are hand-written strings, and Word refuses to open a
    /// file whose tags are not properly nested. A stack walk catches that
    /// without pulling in an XML parser.
    ///
    /// Named for what it actually checks: it compares tag names, so it is a
    /// balance check, not a full well-formedness check. It is deliberately
    /// blind to an unescaped `&` and to XML-illegal codepoints, both of which
    /// are covered directly by `docx_escapes_markup_in_generated_text` and
    /// `docx_drops_characters_xml_forbids`.
    #[test]
    fn every_docx_part_has_balanced_tags() {
        let bytes = render_docx(&doc()).unwrap();
        let mut archive = ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        for name in archive.file_names().map(str::to_owned).collect::<Vec<_>>() {
            if !name.ends_with(".xml") && !name.ends_with(".rels") {
                continue;
            }
            let mut body = String::new();
            use std::io::Read;
            archive
                .by_name(&name)
                .unwrap()
                .read_to_string(&mut body)
                .unwrap();
            let mut open: Vec<&str> = Vec::new();
            let mut rest = body.as_str();
            while let Some(start) = rest.find('<') {
                rest = &rest[start + 1..];
                let end = match rest.find('>') {
                    Some(end) => end,
                    None => panic!("{name}: unterminated tag"),
                };
                let tag = &rest[..end];
                rest = &rest[end + 1..];
                if tag.starts_with('?') || tag.starts_with('!') {
                    continue;
                }
                match tag.strip_prefix('/') {
                    Some(closing) => {
                        assert_eq!(
                            open.pop(),
                            Some(closing),
                            "{name}: </{closing}> does not close the open element"
                        );
                    }
                    None if tag.ends_with('/') => {}
                    None => {
                        let name = tag.split_whitespace().next().unwrap_or_default();
                        open.push(name);
                    }
                }
            }
            assert!(open.is_empty(), "{name}: {open:?} left unclosed");
        }
    }
}
