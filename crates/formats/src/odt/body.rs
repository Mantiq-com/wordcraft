use wordcraft_doc::fill::Fill;
use wordcraft_doc::para::{Anchor, Float, FloatAlign, NoteKind, ShapeKind, Wrap, WrapText};
use wordcraft_doc::{InlineObject, MAX_TEXT_BOX_INSET, PartKind, Rgb, TextBody};

use super::import::Pages;
use super::limits::{MAX_COMMENTS, MAX_INFO_CHARS, MAX_OPEN_INSERTIONS, MAX_REGIONS, MAX_SECTIONS, MAX_STORY_DEPTH, MAX_XML_DEPTH};
use super::styles::{Graphic, Styles};
use super::xml::{Attrs, Ev, events, get, length, local};
use super::{Cell, FBlock, FTable, Fmt, HashMap, Inline, Kind, ListInfo, Para, RevMark, RevisionKind, SectionProps, SectionStart, make_img, model};
use crate::model::FObject;

/// An embedded ODF formula sub-document's `content.xml`, read for its `math:math` element
/// (ODF 1.3 §10.4.3.2 wraps it in `office:document-content`/`office:body`/`office:math`, but
/// some writers emit it bare): the first element named `math` in document order, with its
/// subtree, passed to [`crate::mathml::read`]. `None` when there's no such element, it holds
/// nothing, or the XML is malformed. Nesting and element count are bounded like other XML here.
fn read_formula(xml: &[u8]) -> Option<(String, bool)> {
    use crate::mathml::{El, Item, MAX_NODES, local};
    let mut stack: Vec<El> = Vec::new();
    let mut math: Option<El> = None;
    let mut over = 0usize;
    let mut count = 0usize;
    events(xml, |ev| {
        if math.is_some() {
            return;
        }
        count += 1;
        if count > MAX_NODES * 4 {
            math = stack.drain(..).next();
            return;
        }
        match ev {
            Ev::Start(q, a) => {
                if stack.is_empty() && local(&q) != "math" {
                    return;
                }
                if stack.len() < wordcraft_doc::math::MAX_DEPTH {
                    stack.push(El { name: q, attrs: a, children: Vec::new() });
                } else {
                    over += 1;
                }
            }
            Ev::End(_) => {
                if over > 0 {
                    over -= 1;
                } else if stack.len() > 1 {
                    if let Some(done) = stack.pop()
                        && let Some(parent) = stack.last_mut()
                    {
                        parent.children.push(Item::El(done));
                    }
                } else if stack.len() == 1 {
                    math = stack.pop();
                }
            }
            Ev::Text(t) => {
                if over == 0
                    && let Some(top) = stack.last_mut()
                {
                    top.children.push(Item::Text(t));
                }
            }
        }
    });
    crate::mathml::read(&math?)
}

struct TableB {
    rows: Vec<Vec<Cell>>,
    row: Vec<Cell>,
    /// Covered cells still owed to the previous cell's column span.
    hcover: usize,
    /// Widths in points of the `table:table-column`s so far, 0 where the style gives none.
    widths: Vec<f32>,
}

impl TableB {
    fn finish(self) -> FTable {
        // A column without a width leaves the whole grid to the equal split.
        let widths = if self.widths.iter().all(|w| *w > 0.0) { self.widths } else { Vec::new() };
        FTable { rows: self.rows, widths, borderless: false }
    }
}

/// A `text:changed-region` read from `text:tracked-changes`.
#[derive(Default)]
struct Region {
    kind: Option<RevisionKind>,
    /// Sequence number: tells regions by the same author at the same time apart.
    id: u32,
    author: String,
    date: String,
    /// A deletion's deleted paragraphs.
    blocks: Vec<FBlock>,
}

impl Region {
    fn mark(&self) -> RevMark {
        RevMark { author: self.author.trim().to_string(), date: self.date.trim().to_string(), id: Some(self.id) }
    }
}

/// Reader state saved while a deletion's content is read into its own container.
struct DelState {
    containers: usize,
    tables: usize,
    cells: usize,
    lists_saved: usize,
    fmt: usize,
    lists: Vec<(Option<String>, bool)>,
    item_used: Vec<bool>,
    /// Insertions open inside the deleted content (text inserted, then deleted).
    ins: Vec<(String, RevMark)>,
}

pub(super) struct Body<'a> {
    styles: &'a Styles,
    zip: &'a mut dyn FnMut(&str) -> Option<Vec<u8>>,
    containers: Vec<Vec<FBlock>>,
    fmt: Vec<Fmt>,
    para: Option<Para>,
    /// The text read last ended in a space collapsed from XML whitespace (not a `text:s`).
    xml_space_end: bool,
    para_cont: bool,
    /// Open lists: (style name, item open, item used).
    lists: Vec<(Option<String>, bool)>,
    item_used: Vec<bool>,
    /// Lists outside the table cells being read.
    lists_saved: Vec<(Vec<(Option<String>, bool)>, Vec<bool>)>,
    tables: Vec<TableB>,
    cells: Vec<Cell>,
    pub(super) in_text: bool,
    skip: usize,
    depth: usize,
    frame: Option<Frame>,
    in_desc: bool,
    /// Notes, comments, text boxes and shapes being read, innermost last.
    stories: Vec<Story>,
    /// The comments read, in document order: a comment's index is the id its marks carry.
    pub(super) comments: Vec<ReadComment>,
    /// Comment index by `office:name`, for a range's end and a reply's parent.
    comment_names: HashMap<String, usize>,
    /// Notes and comments are read: in the body, not in a header or footer.
    pub(super) with_notes: bool,
    /// The element whose text is kept apart (a comment's author, a field's result), and that text.
    capture: Option<(Capture, String)>,
    /// The master pages' page setups.
    pages: &'a Pages,
    /// The current master page's setup (a column section's page, without its columns).
    page: SectionProps,
    /// The open section; where its blocks start in the body; how many sections have ended.
    pub(super) sec: SectionProps,
    pub(super) sec_from: usize,
    sections: usize,
    /// Element depth of the open columned `text:section`.
    cols_at: Option<usize>,
    /// Inside `text:tracked-changes`.
    in_tc: bool,
    /// `text:track-changes` of the document's tracked changes (default on), when it has them.
    pub(super) track: Option<bool>,
    /// Changed regions read, by id.
    regions: HashMap<String, Region>,
    /// Regions seen (capped at [`MAX_REGIONS`]).
    region_count: usize,
    /// The region being read.
    region: Option<(String, Region)>,
    /// Depth of the `office:change-info` being read, and the field its text goes to.
    info_at: Option<usize>,
    info_field: Option<bool>,
    /// Reading a deletion's content.
    del: Option<DelState>,
    /// Insertions open in the body: (region id, revision).
    active_ins: Vec<(String, RevMark)>,
}

impl<'a> Body<'a> {
    pub(super) fn new(styles: &'a Styles, pages: &'a Pages, zip: &'a mut dyn FnMut(&str) -> Option<Vec<u8>>) -> Self {
        let page = pages.first.clone().unwrap_or_default();
        Body {
            styles,
            zip,
            containers: vec![Vec::new()],
            fmt: Vec::new(),
            para: None,
            xml_space_end: false,
            para_cont: false,
            lists: Vec::new(),
            lists_saved: Vec::new(),
            item_used: Vec::new(),
            tables: Vec::new(),
            cells: Vec::new(),
            in_text: false,
            skip: 0,
            depth: 0,
            frame: None,
            in_desc: false,
            stories: Vec::new(),
            comments: Vec::new(),
            comment_names: HashMap::new(),
            with_notes: false,
            capture: None,
            pages,
            sec: page.clone(),
            page,
            sec_from: 0,
            sections: 0,
            cols_at: None,
            in_tc: false,
            track: None,
            regions: HashMap::new(),
            region_count: 0,
            region: None,
            info_at: None,
            info_field: None,
            del: None,
            active_ins: Vec::new(),
        }
    }

    pub(super) fn feed(&mut self, ev: Ev) {
        match ev {
            Ev::Start(q, a) => {
                if self.depth < MAX_XML_DEPTH {
                    self.start(&q, &a);
                } else {
                    self.depth += 1;
                }
            }
            Ev::End(q) => {
                if self.depth <= MAX_XML_DEPTH {
                    self.end(&q);
                } else {
                    self.depth -= 1;
                }
            }
            Ev::Text(t) => self.text(&t),
        }
    }

    /// The blocks read, with anything still open closed.
    pub(super) fn take_blocks(&mut self) -> Vec<FBlock> {
        self.end_para();
        while self.containers.len() > 1 {
            let c = self.containers.pop().unwrap_or_default();
            self.container().extend(c);
        }
        let mut blocks = self.containers.pop().unwrap_or_default();
        for t in self.tables.drain(..) {
            if !t.rows.is_empty() {
                blocks.push(FBlock::Table(t.finish()));
            }
        }
        let open: Vec<u32> = self.comments.iter().enumerate().filter(|(_, c)| c.parent.is_none() && !c.ended).map(|(i, _)| i as u32).collect();
        if !open.is_empty() {
            close_comments(&mut blocks, &open);
        }
        blocks
    }
}

impl Body<'_> {
    fn container(&mut self) -> &mut Vec<FBlock> {
        if self.containers.is_empty() {
            self.containers.push(Vec::new());
        }
        let n = self.containers.len() - 1;
        &mut self.containers[n]
    }

    fn cur_fmt(&self) -> Fmt {
        self.fmt.last().cloned().unwrap_or_default()
    }

    /// Reading the body itself, not a table cell or a frame.
    fn in_body(&self) -> bool {
        self.containers.len() == 1 && self.tables.is_empty() && self.frame.is_none()
    }

    /// End the open section before what comes next, which starts section `next`. An open section
    /// with nothing in it yet takes `next`'s place instead. Only in the body, and at most
    /// [`MAX_SECTIONS`]. True when what comes next follows a section break.
    fn break_section(&mut self, next: SectionProps) -> bool {
        if !self.in_body() {
            return false;
        }
        let Some(body) = self.containers.first_mut() else { return false };
        if body.len() <= self.sec_from {
            let restart = self.sec.page_num_start;
            self.sec = next;
            self.sec.page_num_start = self.sec.page_num_start.or(restart);
            return self.sections > 0;
        }
        if self.sections + 1 >= MAX_SECTIONS {
            return false;
        }
        // A section ends on a paragraph: after a table, an empty one (as Word writes it).
        if !matches!(body.last(), Some(FBlock::Para(p)) if p.section.is_none()) {
            body.push(FBlock::Para(Para::default()));
        }
        if let Some(FBlock::Para(p)) = body.last_mut() {
            p.section = Some(Box::new(std::mem::replace(&mut self.sec, next)));
        }
        self.sec_from = body.len();
        self.sections += 1;
        true
    }

    /// A body paragraph or table of style `name` that names a master page starts a new page with
    /// that page's setup and, when the style says so, page numbering restarted (ODF 1.3 applies
    /// `style:page-number` only with a master page). The section that starts there, if any.
    ///
    /// Rare cases left simple: a master-page paragraph first inside a columned `text:section`
    /// replaces that (still empty) column section, so the columns are lost; a columned section
    /// nested in another is read as part of the outer one.
    fn page_section(&mut self, name: &str) -> Option<SectionProps> {
        if !self.in_body() {
            return None;
        }
        let (master, number) = self.styles.page_start(name);
        let restart = number.flatten();
        let page = master.and_then(|m| self.pages.masters.get(m))?.clone();
        self.page = page.clone();
        Some(SectionProps { start: SectionStart::NextPage, page_num_start: restart, ..page })
    }

    fn start_para(&mut self, style: Option<&str>, heading: Option<u8>) {
        self.end_para();
        let (mut kind, align, mut pb, rule, keep_next, base) =
            style.map(|s| self.styles.para(s)).unwrap_or((Kind::Normal, None, false, false, false, Fmt::default()));
        // The section break starts the new page; no page break besides.
        if let Some(next) = style.and_then(|s| self.page_section(s))
            && self.break_section(next)
        {
            pb = false;
        }
        if let Some(h) = heading {
            kind = Kind::Heading(h.clamp(1, 6));
        }
        let mut p = Para { kind, align, page_break: pb, keep_next, ..Default::default() };
        if rule && kind == Kind::Normal {
            p.kind = Kind::Rule;
        }
        if let Some(lv) = self.lists.len().checked_sub(1)
            && self.lists.last().is_some_and(|l| l.1)
        {
            let style = self.lists.iter().rev().find_map(|l| l.0.clone());
            let ordered = style.as_ref().and_then(|s| self.styles.lists.get(s)).and_then(|v| v.get(lv.min(9)).copied()).unwrap_or(false);
            p.list = Some(ListInfo { ordered, level: lv.min(8) as u8, start: 1 });
            p.list_levels = style.and_then(|s| self.styles.list_levels.get(&s).cloned());
            self.para_cont = self.item_used.last().copied().unwrap_or(false);
            if let Some(u) = self.item_used.last_mut() {
                *u = true;
            }
        } else {
            self.para_cont = false;
        }
        self.fmt.push(base);
        self.para = Some(p);
    }

    fn end_para(&mut self) {
        let Some(mut p) = self.para.take() else { return };
        self.fmt.pop();
        // A paragraph ending inside an open insertion has an inserted mark. (Paragraphs that
        // `place_deletion` splits off keep only the marks read inside their region.)
        if p.mark_ins.is_none() {
            p.mark_ins = self.open_insertion().map(Box::new);
        }
        self.finish_para(p);
    }

    /// The innermost insertion open where text is being read (the body, or deleted content).
    fn open_insertion(&self) -> Option<RevMark> {
        let open = match &self.del {
            Some(d) => &d.ins,
            None => &self.active_ins,
        };
        open.last().map(|(_, m)| m.clone())
    }

    /// Add a finished paragraph to the current container.
    fn finish_para(&mut self, mut p: Para) {
        // Spaces next to tracked text are part of a change.
        let tracked = self.del.is_some()
            || p.mark_ins.is_some()
            || p.mark_del.is_some()
            || p.inlines.iter().any(|i| matches!(i, Inline::Text(_, f) if f.ins.is_some() || f.del.is_some()));
        // Leading XML whitespace is never read (see `text`); a trailing one is trimmed here.
        // Explicit `text:s` spaces and tabs stay.
        let xml_space_end = std::mem::take(&mut self.xml_space_end);
        if p.kind != Kind::Code
            && !tracked
            && xml_space_end
            && let Some(Inline::Text(t, _)) = p.inlines.last_mut()
            && t.ends_with(' ')
        {
            t.pop();
            if t.is_empty() {
                p.inlines.pop();
            }
        }
        if p.kind == Kind::Rule && !p.is_empty() {
            p.kind = Kind::Normal;
        }
        if matches!(p.kind, Kind::Code | Kind::Heading(_) | Kind::Title) {
            for i in &mut p.inlines {
                if let Inline::Text(_, f) = i {
                    if p.kind == Kind::Code {
                        f.code = false;
                        f.font = None;
                    }
                    f.size = None;
                }
            }
        }
        let cont = std::mem::take(&mut self.para_cont);
        let level = p.list.map(|l| l.level);
        let c = self.container();
        if cont
            && let Some(FBlock::Para(prev)) = c.last_mut()
            && prev.list.is_some()
            && prev.list.map(|l| l.level) == level
        {
            prev.push_text("\n", &Fmt::default());
            prev.inlines.extend(p.inlines);
            return;
        }
        c.push(FBlock::Para(p));
    }

    fn push_text(&mut self, t: &str) {
        let mut f = self.cur_fmt();
        f.ins = self.open_insertion();
        if self.para.is_none() {
            if t.trim().is_empty() {
                return;
            }
            self.start_para(None, None);
        }
        self.xml_space_end = false;
        if let Some(p) = &mut self.para {
            p.push_text(t, &f);
        }
    }

    fn text(&mut self, t: &str) {
        if self.skip > 0 || !self.in_text {
            return;
        }
        if self.info_at.is_some() {
            if let (Some(field), Some((_, r))) = (self.info_field, &mut self.region) {
                let s = if field { &mut r.author } else { &mut r.date };
                let room = MAX_INFO_CHARS.saturating_sub(s.chars().count());
                s.extend(t.chars().take(room));
            }
            return;
        }
        if self.in_tc && self.del.is_none() {
            return;
        }
        if let Some((_, s)) = &mut self.capture {
            let room = MAX_INFO_CHARS.saturating_sub(s.chars().count());
            s.extend(t.chars().take(room));
            return;
        }
        if self.in_desc {
            if let Some(fr) = &mut self.frame {
                fr.alt.push_str(t);
            }
            return;
        }
        let Some(p) = &self.para else {
            return;
        };
        // Collapse XML whitespace.
        let mut prev_space = match p.inlines.last() {
            Some(Inline::Text(x, _)) => x.ends_with([' ', '\n', '\t']),
            Some(_) => false,
            None => true,
        };
        let mut s = String::with_capacity(t.len());
        for c in t.chars() {
            if matches!(c, ' ' | '\t' | '\n' | '\r') {
                if !prev_space {
                    s.push(' ');
                    prev_space = true;
                }
            } else {
                s.push(c);
                prev_space = false;
            }
        }
        if !s.is_empty() {
            self.push_text(&s);
            self.xml_space_end = s.ends_with(' ');
        }
    }

    fn start(&mut self, q: &str, a: &Attrs) {
        self.depth += 1;
        if q == "office:text" {
            self.in_text = true;
            return;
        }
        if self.skip > 0 {
            self.skip += 1;
            return;
        }
        if !self.in_text {
            return;
        }
        if self.info_at.is_some() {
            self.info_field = match local(q) {
                "creator" => Some(true),
                "date" => Some(false),
                _ => None,
            };
            return;
        }
        if self.tracked_context(q, a) {
            return;
        }
        if self.start_inline(q, a) || self.start_story(q, a) {
            return;
        }
        match local(q) {
            "tracked-changes" => {
                if self.para.is_none() && self.tables.is_empty() && self.cells.is_empty() && self.frame.is_none() {
                    self.in_tc = true;
                    self.track = Some(get(a, "text:track-changes") != Some("false"));
                } else {
                    self.skip = 1;
                }
            }
            "change" => {
                if let Some(id) = get(a, "text:change-id")
                    && self.regions.get(id).is_some_and(|r| r.kind == Some(RevisionKind::Delete))
                    && let Some(r) = self.regions.remove(id)
                {
                    let mark = r.mark();
                    self.place_deletion(r.blocks, mark);
                }
            }
            "change-start" | "change-end" => change_mark(&self.regions, &mut self.active_ins, local(q) == "change-start", a),
            "sequence-decls" | "forms" | "variable-decls" | "user-field-decls" | "index-title-template" | "table-of-content-source" | "text-box" => {
                self.skip = 1;
            }
            "p" | "h" if !self.in_desc => {
                let h =
                    if local(q) == "h" { Some(get(a, "text:outline-level").and_then(|v| v.trim().parse::<u8>().ok()).unwrap_or(1)) } else { None };
                self.start_para(get(a, "text:style-name"), h);
            }
            "list" => {
                self.end_para();
                if self.lists.len() < 64 {
                    self.lists.push((get(a, "text:style-name").map(String::from), false));
                    self.item_used.push(false);
                } else {
                    self.skip = 1;
                }
            }
            "list-item" | "list-header" => {
                if let Some(l) = self.lists.last_mut() {
                    l.1 = true;
                }
                if let Some(u) = self.item_used.last_mut() {
                    *u = false;
                }
            }
            "table" if q.starts_with("table:") => {
                self.end_para();
                // A body table whose style names a master page starts that page (ODF 1.3 §19.509).
                if let Some(next) = get(a, "table:style-name").and_then(|s| self.page_section(s)) {
                    self.break_section(next);
                }
                if self.tables.len() >= model::MAX_DEPTH {
                    self.skip = 1;
                    return;
                }
                self.tables.push(TableB { rows: Vec::new(), row: Vec::new(), hcover: 0, widths: Vec::new() });
            }
            "table-column" => {
                let w = get(a, "table:style-name").and_then(|n| self.styles.column_width(n)).unwrap_or(0.0);
                let n = get(a, "table:number-columns-repeated").and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(1).max(1);
                if let Some(t) = self.tables.last_mut() {
                    let room = wordcraft_doc::table::MAX_COLS.saturating_sub(t.widths.len());
                    t.widths.extend(std::iter::repeat_n(w, n.min(room)));
                }
            }
            "table-row" => {
                if let Some(t) = self.tables.last_mut() {
                    t.row.clear();
                    t.hcover = 0;
                }
            }
            "table-cell" => {
                self.end_para();
                let span = get(a, "table:number-columns-spanned").and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(1).clamp(1, 63);
                let rows = get(a, "table:number-rows-spanned").and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(1).clamp(1, 1000);
                let shading = get(a, "table:style-name").and_then(|s| self.styles.cell_background(s));
                let valign = get(a, "table:style-name").and_then(|s| self.styles.cell_valign(s));
                if let Some(t) = self.tables.last_mut() {
                    t.hcover = span as usize - 1;
                }
                self.cells.push(Cell { colspan: span, rowspan: rows, shading, valign, ..Default::default() });
                self.containers.push(Vec::new());
                self.lists_saved.push((std::mem::take(&mut self.lists), std::mem::take(&mut self.item_used)));
            }
            "covered-table-cell" => {
                if let Some(t) = self.tables.last_mut() {
                    if t.hcover > 0 {
                        t.hcover -= 1;
                    } else if t.row.len() < 63 {
                        t.row.push(Cell { covered: true, ..Default::default() });
                    }
                }
            }
            "frame" => {
                let graphic = get(a, "draw:style-name").map(|s| self.styles.graphic(s)).unwrap_or_default();
                self.frame = Some(Frame {
                    w: get(a, "svg:width").and_then(length),
                    h: get(a, "svg:height").and_then(length),
                    float: placement(a, &graphic, Wrap::Square),
                    graphic,
                    ..Default::default()
                });
            }
            "image" => {
                if let Some(href) = get(a, "xlink:href")
                    && let Some(fr) = &mut self.frame
                    && fr.img.is_none()
                    && fr.eq.is_none()
                {
                    let path = href.trim_start_matches("./");
                    fr.img = (self.zip)(path);
                }
            }
            // An embedded object (ODF 1.3 §10.4.3): a formula sub-document's `content.xml` holds
            // a `math:math` tree (#3099). Other object kinds have none, so the frame still falls
            // back to its `draw:image` replacement preview, read at `end` below.
            "object" => {
                if let Some(href) = get(a, "xlink:href")
                    && let Some(fr) = &mut self.frame
                    && fr.eq.is_none()
                {
                    let path = format!("{}/content.xml", href.trim_start_matches("./").trim_end_matches('/'));
                    if let Some(xml) = (self.zip)(&path) {
                        fr.eq = read_formula(&xml);
                    }
                }
            }
            "desc" | "title" if self.frame.is_some() => self.in_desc = true,
            // A columned section (ODF 1.3 §5.4) is a continuous section with its columns.
            "section" if q.starts_with("text:") => {
                self.end_para();
                if self.cols_at.is_none()
                    && self.in_body()
                    && let Some(columns) = get(a, "text:style-name")
                        .and_then(|s| self.styles.columns(s))
                        // A section whose columns match the open section's (a plain one-column
                        // section in a one-column page) is no new section.
                        .filter(|c| c.count != self.sec.columns.count || (c.count > 1 && *c != self.sec.columns))
                {
                    let next = SectionProps { start: SectionStart::Continuous, page_num_start: None, columns, ..self.page.clone() };
                    self.break_section(next);
                    self.cols_at = Some(self.depth);
                }
            }
            _ => {}
        }
    }

    /// An element inside `text:tracked-changes`, outside deleted content.
    fn tracked_start(&mut self, q: &str, a: &Attrs) {
        match local(q) {
            "changed-region" if self.region.is_none() && self.region_count < MAX_REGIONS => {
                let id = get(a, "text:id").unwrap_or_default().to_string();
                let r = Region { id: self.region_count as u32, ..Default::default() };
                self.region_count += 1;
                self.region = Some((id, r));
            }
            "insertion" | "deletion" | "format-change" if self.region.as_ref().is_some_and(|(_, r)| r.kind.is_none()) => {
                let kind = match local(q) {
                    "insertion" => RevisionKind::Insert,
                    "deletion" => RevisionKind::Delete,
                    _ => RevisionKind::Format,
                };
                if let Some((_, r)) = &mut self.region {
                    r.kind = Some(kind);
                }
                if kind == RevisionKind::Delete {
                    self.containers.push(Vec::new());
                    self.del = Some(DelState {
                        containers: self.containers.len(),
                        tables: self.tables.len(),
                        cells: self.cells.len(),
                        lists_saved: self.lists_saved.len(),
                        fmt: self.fmt.len(),
                        lists: std::mem::take(&mut self.lists),
                        item_used: std::mem::take(&mut self.item_used),
                        ins: Vec::new(),
                    });
                }
            }
            "change-info" if self.region.is_some() => self.info_at = Some(self.depth),
            _ => self.skip = 1,
        }
    }

    /// End a deletion's content: its paragraphs go to the region being read.
    fn close_deletion(&mut self) {
        if self.del.is_none() {
            return;
        }
        self.end_para();
        let Some(d) = self.del.take() else { return };
        let base = d.containers.saturating_sub(1).min(self.containers.len());
        let blocks: Vec<FBlock> = self.containers.split_off(base).into_iter().flatten().filter(|b| matches!(b, FBlock::Para(_))).collect();
        if self.containers.is_empty() {
            self.containers.push(Vec::new());
        }
        self.tables.truncate(d.tables);
        self.cells.truncate(d.cells);
        self.lists_saved.truncate(d.lists_saved);
        self.fmt.truncate(d.fmt);
        self.lists = d.lists;
        self.item_used = d.item_used;
        self.frame = None;
        self.in_desc = false;
        self.para_cont = false;
        if let Some((_, r)) = &mut self.region {
            r.blocks = blocks;
        }
    }

    /// Put a deletion's paragraphs back at a `text:change` point, as deleted text. Inside a
    /// paragraph, each deleted paragraph mark splits it; between paragraphs, each deleted
    /// paragraph stands alone with its mark deleted.
    fn place_deletion(&mut self, blocks: Vec<FBlock>, mark: RevMark) {
        let mut paras: Vec<Para> = Vec::new();
        for b in blocks {
            let FBlock::Para(mut p) = b else { continue };
            // Pictures and other objects carry no revision mark: leave them out.
            p.inlines.retain(|i| matches!(i, Inline::Text(..) | Inline::Anchor(_)));
            for i in &mut p.inlines {
                if let Inline::Text(_, f) = i {
                    f.del = Some(mark.clone());
                }
            }
            paras.push(p);
        }
        let append = |dst: &mut Para, src: Vec<Inline>| {
            for i in src {
                match i {
                    Inline::Text(t, f) => dst.push_text(&t, &f),
                    other => dst.inlines.push(other),
                }
            }
        };
        let Some(mut cur) = self.para.take() else {
            self.para_cont = false;
            for mut p in paras {
                p.mark_del = Some(Box::new(mark.clone()));
                self.finish_para(p);
            }
            return;
        };
        let n = paras.len();
        let mut it = paras.into_iter();
        let mut first_mark_ins = None;
        if let Some(first) = it.next() {
            first_mark_ins = first.mark_ins;
            append(&mut cur, first.inlines);
        }
        if n < 2 {
            self.para = Some(cur);
            return;
        }
        let mut rest = Para { kind: cur.kind, list: cur.list, align: cur.align, ..Default::default() };
        // The host paragraph now ends at the first deleted paragraph's mark.
        cur.mark_del = Some(Box::new(mark.clone()));
        cur.mark_ins = first_mark_ins;
        self.finish_para(cur);
        for (k, mut p) in it.enumerate() {
            if k + 2 == n {
                // The last deleted paragraph runs on into the rest of the body paragraph.
                rest.kind = p.kind;
                rest.align = p.align.or(rest.align);
                append(&mut rest, p.inlines);
            } else {
                p.mark_del = Some(Box::new(mark.clone()));
                self.finish_para(p);
            }
        }
        self.para = Some(rest);
    }

    fn tracked_context(&mut self, q: &str, a: &Attrs) -> bool {
        if !self.in_tc {
            return false;
        }
        if self.del.is_none() {
            self.tracked_start(q, a);
            return true;
        }
        // Inside deleted content: only insertions (text inserted, then deleted) nest.
        if matches!(local(q), "change-start" | "change-end") {
            let regions = &self.regions;
            if let Some(d) = &mut self.del {
                change_mark(regions, &mut d.ins, local(q) == "change-start", a);
            }
            return true;
        }
        if matches!(local(q), "tracked-changes" | "changed-region" | "insertion" | "deletion" | "format-change" | "change") {
            self.skip = 1;
            return true;
        }
        if local(q) == "change-info" && self.region.is_some() {
            self.info_at = Some(self.depth);
            return true;
        }
        false
    }

    fn start_inline(&mut self, q: &str, a: &Attrs) -> bool {
        match local(q) {
            "span" => {
                let base = self.cur_fmt();
                let fmt = get(a, "text:style-name").map_or_else(|| base.clone(), |s| self.styles.text_fmt(s, &base));
                self.fmt.push(fmt);
            }
            "a" => {
                let mut fmt = self.cur_fmt();
                if let Some(href) = get(a, "xlink:href").filter(|href| !href.is_empty()) {
                    fmt.link = model::kept_link(href);
                }
                if let Some(style) = get(a, "text:style-name") {
                    fmt = self.styles.text_fmt(style, &fmt);
                }
                self.fmt.push(fmt);
            }
            "s" => {
                let count = get(a, "text:c").and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(1).clamp(1, 1000);
                self.push_text(&" ".repeat(count));
            }
            "tab" => self.push_text("\t"),
            "line-break" => self.push_text("\n"),
            "bookmark" | "bookmark-start" => {
                if let Some(name) = get(a, "text:name") {
                    if self.para.is_none() {
                        self.start_para(None, None);
                    }
                    if let Some(p) = &mut self.para {
                        p.inlines.push(Inline::Anchor(name.to_string()));
                    }
                }
            }
            // Page number and page count (ODF 1.3 §7.3.5, §7.5.18): fields, so they count the
            // pages of the document they are shown in. Their text is the result last shown.
            "page-number" | "page-count" if q.starts_with("text:") => {
                let instr = if local(q) == "page-number" { "PAGE" } else { "NUMPAGES" };
                self.capture = Some((Capture::Field(instr), String::new()));
            }
            "annotation-end" if q.starts_with("office:") => {
                if let Some(&idx) = get(a, "office:name").and_then(|n| self.comment_names.get(n))
                    && self.comments.get(idx).is_some_and(|c| c.parent.is_none() && !c.ended)
                {
                    if let Some(c) = self.comments.get_mut(idx) {
                        c.ended = true;
                    }
                    self.push_object(InlineObject::CommentEnd { id: idx as u32 }, None, TextBody::default());
                }
            }
            _ => return false,
        }
        true
    }

    fn end(&mut self, q: &str) {
        self.depth = self.depth.saturating_sub(1);
        if q == "office:text" {
            if self.del.is_some() {
                // Unclosed deleted content (hostile): drop it.
                self.close_deletion();
            }
            self.in_tc = false;
            self.region = None;
            self.info_at = None;
            self.end_para();
            self.in_text = false;
            return;
        }
        if self.skip > 0 {
            self.skip -= 1;
            return;
        }
        if !self.in_text {
            return;
        }
        if let Some((what, text)) = self.capture.take() {
            self.end_capture(what, text);
            return;
        }
        if self.stories.last().is_some_and(|s| s.at == self.depth + 1) {
            self.close_story();
            return;
        }
        if let Some(d) = self.info_at {
            if self.depth < d {
                self.info_at = None;
            }
            self.info_field = None;
            return;
        }
        if self.in_tc {
            match local(q) {
                "deletion" if self.del.is_some() => {
                    self.close_deletion();
                    return;
                }
                "changed-region" => {
                    // A deletion left open (hostile) ends with its region.
                    self.close_deletion();
                    if let Some((id, r)) = self.region.take()
                        && r.kind.is_some()
                    {
                        // The first region with an id wins.
                        self.regions.entry(id).or_insert(r);
                    }
                    return;
                }
                "tracked-changes" => {
                    self.close_deletion();
                    self.in_tc = false;
                    self.region = None;
                    return;
                }
                _ if self.del.is_some() => {}
                _ => return,
            }
        }
        match local(q) {
            "p" | "h" if !self.in_desc => self.end_para(),
            "span" | "a" if self.fmt.len() > usize::from(self.para.is_some()) => {
                self.fmt.pop();
            }
            "list" => {
                self.end_para();
                self.lists.pop();
                self.item_used.pop();
            }
            "list-item" | "list-header" => {
                self.end_para();
                if let Some(l) = self.lists.last_mut() {
                    l.1 = false;
                }
            }
            "table-cell" => {
                self.end_para();
                let blocks = if self.containers.len() > 1 { self.containers.pop().unwrap_or_default() } else { Vec::new() };
                (self.lists, self.item_used) = self.lists_saved.pop().unwrap_or_default();
                let mut cell = self.cells.pop().unwrap_or_default();
                cell.blocks = blocks;
                if let Some(t) = self.tables.last_mut()
                    && t.row.len() < 63
                {
                    t.row.push(cell);
                }
            }
            "table-row" => {
                if let Some(t) = self.tables.last_mut() {
                    let r = std::mem::take(&mut t.row);
                    if !r.is_empty() && t.rows.len() < wordcraft_doc::table::MAX_ROWS {
                        t.rows.push(r);
                    }
                }
            }
            "table-header-rows" => {
                if let Some(t) = self.tables.last_mut() {
                    for r in &mut t.rows {
                        for c in r {
                            c.header = true;
                        }
                    }
                }
            }
            "table" if q.starts_with("table:") => {
                self.end_para();
                if let Some(t) = self.tables.pop()
                    && !t.rows.is_empty()
                {
                    self.container().push(FBlock::Table(t.finish()));
                }
            }
            "desc" | "title" => self.in_desc = false,
            "section" if q.starts_with("text:") && self.cols_at == Some(self.depth + 1) => {
                self.end_para();
                self.cols_at = None;
                let next = SectionProps { start: SectionStart::Continuous, page_num_start: None, ..self.page.clone() };
                self.break_section(next);
            }
            "frame" => {
                if let Some(fr) = self.frame.take() {
                    if let Some(text) = fr.text {
                        let grows = fr.h.is_none() || fr.min_h.is_some() || fr.graphic.min_height == Some(true);
                        self.push_text_box(text, fr.w, fr.h.or(fr.min_h), grows, fr.float, &fr.graphic);
                        return;
                    }
                    let inline = match fr.eq {
                        Some((linear, display)) => Some(Inline::Equation { linear, display }),
                        None => fr.img.and_then(|data| make_img(data, fr.w, fr.h, fr.alt.trim())).map(Inline::Image),
                    };
                    if let Some(inline) = inline {
                        if self.para.is_none() {
                            self.start_para(None, None);
                        }
                        if let Some(p) = &mut self.para {
                            p.inlines.push(inline);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// A `text:change-start` (`start`) or `text:change-end` (attributes `a`) among the insertions
/// `open`: unknown ids, ends without a start and starts past [`MAX_OPEN_INSERTIONS`] are ignored.
fn change_mark(regions: &HashMap<String, Region>, open: &mut Vec<(String, RevMark)>, start: bool, a: &Attrs) {
    let Some(id) = get(a, "text:change-id") else { return };
    if !start {
        if let Some(k) = open.iter().position(|(o, _)| o == id) {
            open.remove(k);
        }
        return;
    }
    if let Some(r) = regions.get(id).filter(|r| r.kind == Some(RevisionKind::Insert))
        && open.len() < MAX_OPEN_INSERTIONS
        && !open.iter().any(|(o, _)| o == id)
    {
        open.push((id.to_string(), r.mark()));
    }
}

/// A `draw:frame` being read: a picture, an equation or a text box.
#[derive(Default)]
struct Frame {
    w: Option<f32>,
    h: Option<f32>,
    img: Option<Vec<u8>>,
    alt: String,
    /// An equation read from an embedded `draw:object`.
    eq: Option<(String, bool)>,
    /// Its `draw:text-box`'s text, once read, and that box's least height.
    text: Option<Vec<FBlock>>,
    min_h: Option<f32>,
    float: Float,
    graphic: Graphic,
}

/// A comment read from an `office:annotation` (ODF 1.3 §14.1).
#[derive(Default)]
pub(super) struct ReadComment {
    pub(super) author: String,
    pub(super) initials: String,
    pub(super) date: String,
    /// The comment this one answers (`loext:parent-name`). A reply marks no text of its own.
    pub(super) parent: Option<usize>,
    pub(super) resolved: bool,
    pub(super) blocks: Vec<FBlock>,
    /// Its range has ended.
    ended: bool,
}

/// Where the text of the element being read goes.
#[derive(Clone, Copy)]
enum Capture {
    Author,
    Date,
    Initials,
    /// A field's result, for the field with this code.
    Field(&'static str),
}

enum StoryKind {
    Note {
        kind: NoteKind,
        custom: String,
    },
    Comment {
        idx: usize,
        named: bool,
    },
    /// The text of the open frame's text box.
    TextBox,
    Shape(Box<ShapeRead>),
}

/// A drawing shape being read (ODF 1.3 §10.3).
struct ShapeRead {
    kind: ShapeKind,
    w: Option<f32>,
    h: Option<f32>,
    float: Float,
    graphic: Graphic,
}

/// What a note's, comment's, text box's or shape's text interrupted: the reader's state when its
/// element started, put back when it ends.
struct Story {
    kind: StoryKind,
    /// Element depth of the story's element.
    at: usize,
    containers: usize,
    para: Option<Para>,
    fmt: Vec<Fmt>,
    lists: Vec<(Option<String>, bool)>,
    item_used: Vec<bool>,
    lists_saved: Vec<(Vec<(Option<String>, bool)>, Vec<bool>)>,
    tables: Vec<TableB>,
    cells: Vec<Cell>,
    para_cont: bool,
    xml_space_end: bool,
    frame: Option<Frame>,
    in_desc: bool,
}

/// LibreOffice's own fill and outline of a shape whose style gives none.
const SHAPE_FILL: Rgb = Rgb(0x72, 0x9f, 0xcf);
const SHAPE_STROKE: Rgb = Rgb(0x34, 0x65, 0xa4);

impl Body<'_> {
    /// An element of a note, comment, text box or drawing shape (`q`, attributes `a`); false when
    /// `q` is none.
    fn start_story(&mut self, q: &str, a: &Attrs) -> bool {
        match local(q) {
            // A comment's author, date and initials (ODF 1.3 §14.1), its first elements.
            "creator" | "date" | "creator-initials" | "sender-initials" if self.in_comment_head() => {
                let what = match local(q) {
                    "creator" => Capture::Author,
                    "date" => Capture::Date,
                    _ => Capture::Initials,
                };
                self.capture = Some((what, String::new()));
            }
            // A note's number, or the mark it shows instead (`text:label`).
            "note-citation" => {
                if let Some(Story { kind: StoryKind::Note { custom, .. }, .. }) = self.stories.last_mut() {
                    *custom = get(a, "text:label").map(|l| l.trim().chars().take(MAX_INFO_CHARS).collect()).unwrap_or_default();
                }
                self.skip = 1;
            }
            "note" if q.starts_with("text:") => {
                if !self.with_notes || self.in_note() || self.stories.len() >= MAX_STORY_DEPTH {
                    self.skip = 1;
                    return true;
                }
                let kind = if get(a, "text:note-class") == Some("endnote") { NoteKind::Endnote } else { NoteKind::Footnote };
                self.begin_story(StoryKind::Note { kind, custom: String::new() });
            }
            "annotation" if q.starts_with("office:") => self.start_comment(a),
            "text-box" if self.frame.is_some() && self.stories.len() < MAX_STORY_DEPTH => {
                if let Some(fr) = &mut self.frame {
                    fr.min_h = get(a, "fo:min-height").and_then(length).filter(|h| *h > 0.0);
                }
                self.begin_story(StoryKind::TextBox);
            }
            // Drawing shapes (ODF 1.3 §10.3), with their text. Groups are walked, so the pictures
            // and shapes in them are read one by one.
            "rect" | "line" | "polyline" | "polygon" | "regular-polygon" | "path" | "circle" | "ellipse" | "connector" | "caption" | "measure"
            | "custom-shape"
                if q.starts_with("draw:") =>
            {
                if self.stories.len() >= MAX_STORY_DEPTH {
                    self.skip = 1;
                    return true;
                }
                let shape = self.shape(local(q), a);
                self.begin_story(StoryKind::Shape(Box::new(shape)));
            }
            // A custom shape's geometry, after its text: its outline's kind.
            "enhanced-geometry" => {
                if let Some(Story { kind: StoryKind::Shape(s), .. }) = self.stories.last_mut()
                    && let Some(kind) = get(a, "draw:type").and_then(custom_shape_kind)
                {
                    s.kind = kind;
                }
            }
            _ => return false,
        }
        true
    }

    /// An `office:annotation`: a comment, read as a story of its own, unless notes and comments
    /// are left out here or it sits in another story.
    fn start_comment(&mut self, a: &Attrs) {
        if !self.with_notes || !self.stories.is_empty() || self.comments.len() >= MAX_COMMENTS {
            self.skip = 1;
            return;
        }
        let idx = self.comments.len();
        let name = get(a, "office:name").filter(|n| !n.is_empty());
        let parent = get(a, "loext:parent-name").and_then(|p| self.comment_names.get(p).copied());
        self.comments.push(ReadComment { parent, resolved: get(a, "loext:resolved") == Some("true"), ..Default::default() });
        if let Some(n) = name {
            self.comment_names.entry(n.to_string()).or_insert(idx);
        }
        self.begin_story(StoryKind::Comment { idx, named: name.is_some() });
    }

    /// Reading the elements that open a comment: its author, date and initials.
    fn in_comment_head(&self) -> bool {
        self.stories.last().is_some_and(|s| matches!(s.kind, StoryKind::Comment { .. }) && s.at + 1 == self.depth)
    }

    fn in_note(&self) -> bool {
        self.stories.iter().any(|s| matches!(s.kind, StoryKind::Note { .. }))
    }

    /// Read what follows, up to the end of the element just started, as a story of its own.
    fn begin_story(&mut self, kind: StoryKind) {
        use std::mem::take;
        let story = Story {
            kind,
            at: self.depth,
            containers: self.containers.len(),
            para: self.para.take(),
            fmt: take(&mut self.fmt),
            lists: take(&mut self.lists),
            item_used: take(&mut self.item_used),
            lists_saved: take(&mut self.lists_saved),
            tables: take(&mut self.tables),
            cells: take(&mut self.cells),
            para_cont: take(&mut self.para_cont),
            xml_space_end: take(&mut self.xml_space_end),
            frame: self.frame.take(),
            in_desc: take(&mut self.in_desc),
        };
        self.stories.push(story);
        self.containers.push(Vec::new());
    }

    /// End the innermost story: its text goes to the note, comment, text box or shape, which
    /// takes its place in the text the story interrupted.
    fn close_story(&mut self) {
        self.end_para();
        let Some(s) = self.stories.pop() else { return };
        let mut blocks: Vec<FBlock> = self.containers.split_off(s.containers.min(self.containers.len())).into_iter().flatten().collect();
        for t in std::mem::take(&mut self.tables) {
            if !t.rows.is_empty() {
                blocks.push(FBlock::Table(t.finish()));
            }
        }
        if self.containers.is_empty() {
            self.containers.push(Vec::new());
        }
        self.para = s.para;
        self.fmt = s.fmt;
        self.lists = s.lists;
        self.item_used = s.item_used;
        self.lists_saved = s.lists_saved;
        self.tables = s.tables;
        self.cells = s.cells;
        self.para_cont = s.para_cont;
        self.xml_space_end = s.xml_space_end;
        self.frame = s.frame;
        self.in_desc = s.in_desc;
        match s.kind {
            StoryKind::Note { kind, custom } => {
                // A note's text starts with its own mark, as the notes Word writes do.
                let mark = Inline::Object(Box::new(FObject {
                    obj: InlineObject::NoteRef { kind, id: 0, custom: custom.clone() },
                    fmt: Fmt::default(),
                    story: None,
                    body: TextBody::default(),
                }));
                match blocks.first_mut() {
                    Some(FBlock::Para(p)) => p.inlines.insert(0, mark),
                    _ => blocks.insert(0, FBlock::Para(Para { inlines: vec![mark], ..Default::default() })),
                }
                let part = if kind == NoteKind::Endnote { PartKind::Endnote } else { PartKind::Footnote };
                self.push_object(InlineObject::NoteRef { kind, id: 0, custom }, Some((part, blocks)), TextBody::default());
            }
            StoryKind::Comment { idx, named } => {
                let Some(c) = self.comments.get_mut(idx) else { return };
                c.blocks = blocks;
                if c.parent.is_some() {
                    return;
                }
                // A comment without a name marks no range: it starts and ends where it stands.
                c.ended = !named;
                self.push_object(InlineObject::CommentStart { id: idx as u32 }, None, TextBody::default());
                if !named {
                    self.push_object(InlineObject::CommentEnd { id: idx as u32 }, None, TextBody::default());
                }
            }
            StoryKind::TextBox => {
                if let Some(fr) = &mut self.frame {
                    fr.text = Some(blocks);
                }
            }
            StoryKind::Shape(shape) => self.push_shape(*shape, blocks),
        }
    }

    /// The text kept apart from the element just ended, where it belongs.
    fn end_capture(&mut self, what: Capture, text: String) {
        let text = text.trim().to_string();
        let comment = match self.stories.last() {
            Some(Story { kind: StoryKind::Comment { idx, .. }, .. }) => Some(*idx),
            _ => None,
        };
        match what {
            Capture::Field(instr) => {
                let field =
                    InlineObject::Field { instr: instr.to_string(), result: text, locked: false, dirty: false, code: false, form: String::new() };
                self.push_object(field, None, TextBody::default());
            }
            Capture::Author | Capture::Date | Capture::Initials => {
                let Some(c) = comment.and_then(|i| self.comments.get_mut(i)) else { return };
                match what {
                    Capture::Author => c.author = text,
                    Capture::Date => c.date = text,
                    _ => c.initials = text,
                }
            }
        }
    }

    /// Add `obj` to the paragraph being read, with the text `story` it points at.
    fn push_object(&mut self, obj: InlineObject, story: Option<(PartKind, Vec<FBlock>)>, body: TextBody) {
        if self.para.is_none() {
            self.start_para(None, None);
        }
        let mut fmt = self.cur_fmt();
        fmt.ins = self.open_insertion();
        self.xml_space_end = false;
        if let Some(p) = &mut self.para {
            p.inlines.push(Inline::Object(Box::new(FObject { obj, fmt, story, body })));
        }
    }

    /// A frame's text box, `text` its content. A box `grows` when its height is only the least it
    /// may have (`fo:min-height`), as LibreOffice grows it to its text.
    fn push_text_box(&mut self, text: Vec<FBlock>, w: Option<f32>, h: Option<f32>, grows: bool, float: Float, g: &Graphic) {
        let lines = text.len().max(1) as f32;
        let w = w.filter(|w| w.is_finite() && *w > 0.0).unwrap_or(144.0).clamp(4.0, 1584.0);
        let h = h.filter(|h| h.is_finite() && *h > 0.0).unwrap_or(lines * 14.0 + 8.0).clamp(4.0, 1584.0);
        let stroke = g.stroke.flatten();
        let obj = InlineObject::Shape {
            kind: ShapeKind::TextBox,
            w,
            h,
            fill: g.fill.flatten().map(Fill::solid),
            stroke: stroke.map(|s| s.0),
            stroke_width: stroke.map_or(0.0, |s| s.1),
            float,
            story: None,
            freeform: None,
            effects: Default::default(),
            extra: Default::default(),
        };
        // A frame without padding sets its text against its border, where Word's own inset
        // would leave room.
        let body = TextBody { inset: Some(inset(g.padding.unwrap_or(0.0))), auto_fit: grows, ..Default::default() };
        self.push_object(obj, Some((PartKind::TextBox, text)), body);
    }

    /// A drawing shape with its text: an outline Word knows, or only its text where the outline is
    /// one read here as none (a polygon, a path).
    fn push_shape(&mut self, s: ShapeRead, text: Vec<FBlock>) {
        let has_text = text.iter().any(|b| !matches!(b, FBlock::Para(p) if p.is_empty()));
        let outline = s.kind != ShapeKind::TextBox;
        if !has_text && !outline {
            return;
        }
        let line = s.kind == ShapeKind::Line;
        let size = |v: Option<f32>| v.filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(72.0).clamp(if line { 0.0 } else { 1.0 }, 1584.0);
        // LibreOffice runs a shape's text on past its edges unless the style wraps it; Word wraps
        // it, and the box would then grow over the text below. Widen the box to its longest line.
        let w = match (has_text, s.graphic.wraps_text) {
            (true, None | Some(false)) => size(s.w).max(text_width(&text, s.graphic.padding)).min(1584.0),
            _ => size(s.w),
        };
        let (fill, stroke) = if outline {
            let fill = if line { None } else { s.graphic.fill.unwrap_or(Some(SHAPE_FILL)) };
            (fill, s.graphic.stroke.unwrap_or(Some((SHAPE_STROKE, 0.75))))
        } else {
            (None, None)
        };
        let obj = InlineObject::Shape {
            kind: s.kind,
            w,
            h: size(s.h),
            fill: fill.map(Fill::solid),
            stroke: stroke.map(|s| s.0),
            stroke_width: stroke.map_or(0.0, |s| s.1),
            float: s.float,
            story: None,
            freeform: None,
            effects: Default::default(),
            extra: Default::default(),
        };
        let story = has_text.then_some((PartKind::TextBox, text));
        // A shape grows to its text unless its style says it doesn't, so none of its text is cut.
        let body = TextBody { inset: s.graphic.padding.map(inset), auto_fit: has_text && s.graphic.auto_grow != Some(false), ..Default::default() };
        self.push_object(obj, story, body);
    }

    /// The shape a `draw:` element (local name `name`) draws, where and how large.
    fn shape(&self, name: &str, a: &Attrs) -> ShapeRead {
        let graphic = get(a, "draw:style-name").map(|s| self.styles.graphic(s)).unwrap_or_default();
        let mut float = placement(a, &graphic, Wrap::InFrontOfText);
        let num = |k: &str| get(a, k).and_then(length);
        let (kind, w, h) = match name {
            "line" | "connector" => {
                let (x1, y1) = (num("svg:x1").unwrap_or(0.0), num("svg:y1").unwrap_or(0.0));
                let (x2, y2) = (num("svg:x2").unwrap_or(x1), num("svg:y2").unwrap_or(y1));
                if float.wrap != Wrap::Inline {
                    float.x = x1.min(x2);
                    float.y = y1.min(y2);
                }
                // A line runs from the top left to the bottom right corner of its box, unless flipped.
                float.flip_h = x2 < x1;
                float.flip_v = y2 < y1;
                (ShapeKind::Line, Some((x2 - x1).abs()), Some((y2 - y1).abs()))
            }
            "rect" => {
                let round = num("draw:corner-radius").is_some_and(|r| r > 0.0);
                (if round { ShapeKind::RoundedRectangle } else { ShapeKind::Rectangle }, num("svg:width"), num("svg:height"))
            }
            "ellipse" | "circle" => (ShapeKind::Ellipse, num("svg:width"), num("svg:height")),
            // A custom shape's outline is named by its geometry, read after its text; until then,
            // and for a geometry Word has no preset for, only its text is shown.
            _ => (ShapeKind::TextBox, num("svg:width"), num("svg:height")),
        };
        ShapeRead { kind, w, h, float, graphic }
    }
}

/// `padding` points as a text box's internal margins on every side, in EMU.
fn inset(padding: f32) -> [u32; 4] {
    [((padding * 12_700.0) as u32).min(MAX_TEXT_BOX_INSET); 4]
}

/// About how wide a box must be for the longest paragraph of `text` to fit on one line, with its
/// inner margins (`padding` points each side, else Word's 7.2): no font is measured here, so each
/// character counts as most of an en of its size.
fn text_width(text: &[FBlock], padding: Option<f32>) -> f32 {
    let longest = text
        .iter()
        .filter_map(|b| match b {
            FBlock::Para(p) => Some(
                p.inlines
                    .iter()
                    .map(|i| match i {
                        Inline::Text(t, f) => t.chars().count() as f32 * f.size.unwrap_or(11.0) * 0.55,
                        _ => 0.0,
                    })
                    .sum::<f32>(),
            ),
            FBlock::Table(_) => None,
        })
        .fold(0.0, f32::max);
    longest + 2.0 * padding.unwrap_or(7.2)
}

/// Where a frame or shape with attributes `a` and style `g` sits: in line with the text when
/// anchored as a character, else floating, relative to the page, the paragraph or the character it
/// is anchored to (ODF 1.3 §19.874), and wrapped as its style says (`wrap` when it says nothing).
fn placement(a: &Attrs, g: &Graphic, wrap: Wrap) -> Float {
    let anchor = get(a, "text:anchor-type").unwrap_or("paragraph");
    if anchor == "as-char" {
        return Float::default();
    }
    let (h_def, v_def) = match anchor {
        "page" => (Anchor::Page, Anchor::Page),
        "char" => (Anchor::Character, Anchor::Paragraph),
        _ => (Anchor::Column, Anchor::Paragraph),
    };
    let mut f = Float { wrap, ..Default::default() };
    f.h_rel = g.h_rel.as_deref().map_or(h_def, |r| relative_to(r, true));
    f.v_rel = g.v_rel.as_deref().map_or(v_def, |r| relative_to(r, false));
    f.x = get(a, "svg:x").and_then(length).filter(|v| v.is_finite()).unwrap_or(0.0).clamp(-1584.0, 1584.0);
    f.y = get(a, "svg:y").and_then(length).filter(|v| v.is_finite()).unwrap_or(0.0).clamp(-1584.0, 1584.0);
    f.h_align = match g.h_pos.as_deref() {
        Some("left") => Some(FloatAlign::Start),
        Some("center") => Some(FloatAlign::Center),
        Some("right") => Some(FloatAlign::End),
        Some("inside") => Some(FloatAlign::Inside),
        Some("outside") => Some(FloatAlign::Outside),
        _ => None,
    };
    f.v_align = match g.v_pos.as_deref() {
        Some("top") => Some(FloatAlign::Start),
        Some("middle") | Some("center") => Some(FloatAlign::Center),
        Some("bottom") => Some(FloatAlign::End),
        _ => None,
    };
    match g.wrap.as_deref() {
        Some("none") => f.wrap = Wrap::TopAndBottom,
        Some("run-through") => f.wrap = if g.background == Some(true) { Wrap::BehindText } else { Wrap::InFrontOfText },
        Some("left") => (f.wrap, f.wrap_text) = (Wrap::Square, WrapText::Left),
        Some("right") => (f.wrap, f.wrap_text) = (Wrap::Square, WrapText::Right),
        Some("biggest") => (f.wrap, f.wrap_text) = (Wrap::Square, WrapText::Largest),
        Some("parallel") | Some("dynamic") => f.wrap = Wrap::Square,
        _ => {}
    }
    f.z = get(a, "draw:z-index").and_then(|z| z.trim().parse::<u32>().ok()).map_or(0, |z| z.saturating_add(1));
    f
}

/// What a `style:horizontal-rel` or `style:vertical-rel` value places an object against.
fn relative_to(rel: &str, horizontal: bool) -> Anchor {
    match rel.trim() {
        "page" => Anchor::Page,
        "page-content" => Anchor::Margin,
        "page-start-margin" => Anchor::LeftMargin,
        "page-end-margin" => Anchor::RightMargin,
        "char" => Anchor::Character,
        "line" | "baseline" | "text" => Anchor::Line,
        _ if horizontal => Anchor::Column,
        _ => Anchor::Paragraph,
    }
}

/// The outline of a custom shape's `draw:type` (ODF 1.3 §19.167), where Word has a preset for it.
fn custom_shape_kind(t: &str) -> Option<ShapeKind> {
    Some(match t.trim() {
        "rectangle" | "flowchart-process" | "frame" => ShapeKind::Rectangle,
        "round-rectangle" | "round-rect" | "flowchart-alternate-process" => ShapeKind::RoundedRectangle,
        "ellipse" | "circle" | "flowchart-connector" => ShapeKind::Ellipse,
        "diamond" | "flowchart-decision" => ShapeKind::Diamond,
        "isosceles-triangle" | "flowchart-extract" => ShapeKind::Triangle,
        "right-triangle" => ShapeKind::RightTriangle,
        "right-arrow" => ShapeKind::Arrow,
        "left-arrow" => ShapeKind::ArrowLeft,
        "up-arrow" => ShapeKind::ArrowUp,
        "down-arrow" => ShapeKind::ArrowDown,
        "star5" => ShapeKind::Star,
        "heart" => ShapeKind::Heart,
        "flowchart-or" => ShapeKind::FlowchartOr,
        _ => return None,
    })
}

/// End each comment in `blocks` whose range never ended (an `office:annotation-end` missing): at
/// its start, as a comment that marks no text.
fn close_comments(blocks: &mut [FBlock], open: &[u32]) {
    for b in blocks {
        match b {
            FBlock::Para(p) => {
                let mut i = 0;
                while i < p.inlines.len() {
                    if let Some(Inline::Object(o)) = p.inlines.get(i)
                        && let InlineObject::CommentStart { id } = o.obj
                        && open.contains(&id)
                    {
                        let end = FObject { obj: InlineObject::CommentEnd { id }, ..(**o).clone() };
                        p.inlines.insert(i + 1, Inline::Object(Box::new(end)));
                        i += 1;
                    }
                    i += 1;
                }
            }
            FBlock::Table(t) => {
                for row in &mut t.rows {
                    for c in row {
                        close_comments(&mut c.blocks, open);
                    }
                }
            }
        }
    }
}
