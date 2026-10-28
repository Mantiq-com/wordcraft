use wordcraft_doc::ListKind;
use wordcraft_doc::numbering::{Level, LevelSuffix, levels_for};
use wordcraft_doc::section::NumFormat;

use super::xml::{Attrs, Ev, events, get, length, local};
use super::{Align, Arc, Columns, Fmt, HashMap, Kind, Rgb, VAlign, model};

// ---------------------------------------------------------------------------------------------
// Import

#[derive(Clone, Debug, Default)]
struct FmtDelta {
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    sup: Option<bool>,
    sub: Option<bool>,
    color: Option<Rgb>,
    background: Option<Rgb>,
    size: Option<f32>,
    font: Option<Arc<str>>,
    code: Option<bool>,
}

impl FmtDelta {
    fn apply(&self, f: &mut Fmt) {
        macro_rules! set {
            ($($k:ident),*) => { $( if let Some(v) = self.$k.clone() { f.$k = v; } )* };
        }
        set!(bold, italic, underline, strike, sup, sub, code);
        if self.color.is_some() {
            f.color = self.color;
        }
        if self.background.is_some() {
            f.background = self.background;
        }
        if self.size.is_some() {
            f.size = self.size;
        }
        if let Some(fam) = &self.font {
            f.font = Some(fam.clone());
        }
    }
}

#[derive(Clone, Debug, Default)]
struct StyleInfo {
    parent: Option<String>,
    display: Option<String>,
    auto: bool,
    fmt: FmtDelta,
    align: Option<Align>,
    page_break: bool,
    /// `fo:keep-with-next`: `Some(true)` for "always", `Some(false)` for "auto".
    keep_next: Option<bool>,
    outline: Option<u8>,
    border_bottom: bool,
    /// `fo:background-color` of a table-cell style's `style:table-cell-properties`.
    cell_background: Option<Rgb>,
    /// `style:vertical-align` of a table-cell style's `style:table-cell-properties`.
    cell_valign: Option<VAlign>,
    /// `style:column-width` of a table-column style's `style:table-column-properties`, in points.
    column_width: Option<f32>,
    /// `style:master-page-name` of a paragraph or table style: its paragraphs (tables) start a
    /// page of that master page. Empty: explicitly none, which stops the lookup up the chain.
    master_page: Option<String>,
    /// `style:page-number` of a paragraph or table style (applied only with a master page): `Some(Some(n))` restarts page numbering at n,
    /// `Some(None)` ("auto") continues it.
    page_number: Option<Option<u32>>,
    /// `style:columns` of a section style.
    columns: Option<Columns>,
    /// `style:graphic-properties` of a frame or shape style.
    graphic: Graphic,
}

/// How a frame or drawing shape sits and looks (`style:graphic-properties`, ODF 1.3 §17.27): each
/// field as the style states it, `None` where it doesn't, so a parent style can fill it in.
#[derive(Clone, Debug, Default)]
pub(super) struct Graphic {
    /// `style:wrap`: `none`, `left`, `right`, `parallel`, `dynamic`, `run-through`, `biggest`.
    pub(super) wrap: Option<String>,
    /// `style:run-through`: `background` puts a run-through object behind the text.
    pub(super) background: Option<bool>,
    pub(super) h_pos: Option<String>,
    pub(super) h_rel: Option<String>,
    pub(super) v_pos: Option<String>,
    pub(super) v_rel: Option<String>,
    /// Fill: `Some(None)` is `draw:fill="none"`.
    pub(super) fill: Option<Option<Rgb>>,
    /// Outline colour and width in points: `Some(None)` is no outline.
    pub(super) stroke: Option<Option<(Rgb, f32)>>,
    /// `fo:padding`, points: the room between a frame's border and its text.
    pub(super) padding: Option<f32>,
    /// A frame's height is only the least it may have (`fo:min-height`): it grows to its text.
    pub(super) min_height: Option<bool>,
    /// `draw:auto-grow-height`: a shape grows to its text.
    pub(super) auto_grow: Option<bool>,
    /// `fo:wrap-option`: a shape's text wraps at its edges (`wrap`) or runs on (`no-wrap`).
    pub(super) wraps_text: Option<bool>,
}

impl Graphic {
    fn read(&mut self, a: &Attrs) {
        let text = |k: &str| get(a, k).map(|v| v.trim().to_string());
        self.wrap = text("style:wrap").or(self.wrap.take());
        if let Some(r) = text("style:run-through") {
            self.background = Some(r == "background");
        }
        self.h_pos = text("style:horizontal-pos").or(self.h_pos.take());
        self.h_rel = text("style:horizontal-rel").or(self.h_rel.take());
        self.v_pos = text("style:vertical-pos").or(self.v_pos.take());
        self.v_rel = text("style:vertical-rel").or(self.v_rel.take());
        match get(a, "draw:fill").map(str::trim) {
            Some("none") => self.fill = Some(None),
            Some("solid") => self.fill = Some(get(a, "draw:fill-color").and_then(model::parse_color)),
            _ => {}
        }
        if let Some(c) = get(a, "fo:background-color") {
            self.fill = Some(model::parse_color(c));
        }
        if get(a, "draw:stroke").map(str::trim) == Some("none") {
            self.stroke = Some(None);
        } else if let Some(c) = get(a, "svg:stroke-color").and_then(model::parse_color) {
            let width = get(a, "svg:stroke-width").and_then(length).unwrap_or(0.75);
            self.stroke = Some(Some((c, width)));
        }
        if let Some(b) = get(a, "fo:border") {
            self.stroke = Some(border_line(b));
        }
        if let Some(p) = get(a, "fo:padding").and_then(length).filter(|p| *p >= 0.0) {
            self.padding = Some(p.min(144.0));
        }
        if get(a, "fo:min-height").is_some() {
            self.min_height = Some(true);
        }
        if let Some(g) = get(a, "draw:auto-grow-height") {
            self.auto_grow = Some(g.trim() == "true");
        }
        if let Some(w) = get(a, "fo:wrap-option") {
            self.wraps_text = Some(w.trim() == "wrap");
        }
    }

    /// `self`, with what it leaves unsaid taken from `parent`.
    fn or(mut self, parent: &Graphic) -> Graphic {
        self.wrap = self.wrap.or_else(|| parent.wrap.clone());
        self.background = self.background.or(parent.background);
        self.h_pos = self.h_pos.or_else(|| parent.h_pos.clone());
        self.h_rel = self.h_rel.or_else(|| parent.h_rel.clone());
        self.v_pos = self.v_pos.or_else(|| parent.v_pos.clone());
        self.v_rel = self.v_rel.or_else(|| parent.v_rel.clone());
        self.fill = self.fill.or(parent.fill);
        self.stroke = self.stroke.or(parent.stroke);
        self.padding = self.padding.or(parent.padding);
        self.min_height = self.min_height.or(parent.min_height);
        self.auto_grow = self.auto_grow.or(parent.auto_grow);
        self.wraps_text = self.wraps_text.or(parent.wraps_text);
        self
    }
}

/// A `text:list-style` being read: whether each level is numbered, and the levels it defines.
struct ListRead {
    name: String,
    ordered: Vec<bool>,
    levels: Vec<Option<Level>>,
    /// The level whose properties come next.
    at: usize,
}

impl ListRead {
    fn new(name: &str) -> ListRead {
        ListRead { name: name.to_string(), ordered: vec![false; 10], levels: vec![None; 10], at: 0 }
    }

    /// An element of the list style (local name `name`, attributes `a`): a level, or where the
    /// current level's label sits.
    fn element(&mut self, name: &str, a: &Attrs) {
        if name == "list-level-label-alignment" {
            self.alignment(a);
        } else {
            self.level(name, a);
        }
    }

    /// A `text:list-level-style-*` element (local name `kind`, attributes `a`).
    fn level(&mut self, kind: &str, a: &Attrs) {
        let lv = get(a, "text:level").and_then(|l| l.trim().parse::<usize>().ok()).unwrap_or(1).clamp(1, 10);
        if let Some(slot) = self.ordered.get_mut(lv - 1) {
            *slot = kind == "list-level-style-number" && get(a, "style:num-format").is_some_and(|f| !f.is_empty());
        }
        self.at = lv - 1;
        if let Some(slot) = self.levels.get_mut(self.at) {
            *slot = Some(list_level(kind, a, lv));
        }
    }

    /// Where the current level's label and text sit (ODF 1.3 §17.20): the text's left indent, how
    /// far left of it the label hangs and what follows the label.
    fn alignment(&mut self, a: &Attrs) {
        let Some(Some(level)) = self.levels.get_mut(self.at) else { return };
        let len = |k: &str| get(a, k).and_then(length).filter(|v| v.is_finite());
        if let Some(left) = len("fo:margin-left") {
            level.indent = left.clamp(0.0, 1584.0);
        }
        if let Some(first) = len("fo:text-indent") {
            level.hanging = (-first).clamp(0.0, 1584.0);
        }
        level.tab = len("text:list-tab-stop-position").map(|t| t.clamp(0.0, 1584.0));
        level.suffix = match get(a, "text:label-followed-by").map(str::trim) {
            Some("space") => LevelSuffix::Space,
            Some("nothing") => LevelSuffix::Nothing,
            _ => LevelSuffix::Tab,
        };
    }

    /// The levels the style defines, up to its last; the levels it leaves out keep the defaults of
    /// their kind. `None` when it defines none.
    fn levels(&self) -> Option<Vec<Level>> {
        let last = self.levels.iter().rposition(Option::is_some)?;
        let defaults = levels_for(if self.ordered.first().copied().unwrap_or(false) { ListKind::Numbered } else { ListKind::Bullet });
        let own = (0..=last.min(8)).map(|i| self.levels.get(i).cloned().flatten().or_else(|| defaults.get(i).cloned()).unwrap_or_default());
        Some(own.collect())
    }
}

/// Level `lv` (1-based) of a list style, from its `text:list-level-style-*` element (local name
/// `kind`, attributes `a`, ODF 1.3 §16.31–33): a number in its format between its prefix and
/// suffix, with as many levels above it as it displays (`1.2`), or a bullet.
fn list_level(kind: &str, a: &Attrs, lv: usize) -> Level {
    let mut level = Level { indent: 36.0 * lv as f32, hanging: 18.0, ..Level::default() };
    if kind != "list-level-style-number" {
        level.format = NumFormat::Bullet;
        level.text = get(a, "text:bullet-char").and_then(|c| c.chars().next()).unwrap_or('•').to_string();
        return level;
    }
    level.format = match get(a, "style:num-format").map(str::trim) {
        Some("a") => NumFormat::LowerLetter,
        Some("A") => NumFormat::UpperLetter,
        Some("i") => NumFormat::LowerRoman,
        Some("I") => NumFormat::UpperRoman,
        Some("") | None => NumFormat::None,
        _ => NumFormat::Decimal,
    };
    let shown = get(a, "text:display-levels").and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(1).clamp(1, lv);
    let numbers: Vec<String> = (lv + 1 - shown..=lv).map(|n| format!("%{n}")).collect();
    let cap = |k: &str| get(a, k).unwrap_or("").chars().take(32).collect::<String>();
    let (prefix, suffix) = (cap("style:num-prefix"), cap("style:num-suffix"));
    level.text = if level.format == NumFormat::None { format!("{prefix}{suffix}") } else { format!("{prefix}{}{suffix}", numbers.join(".")) };
    level.start = get(a, "text:start-value").and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(1).min(32_767);
    level
}

/// A border (`0.06pt solid #000000`, ODF 1.3 §20.183) as an outline: its colour and width, or
/// none for `none` and `hidden`.
fn border_line(v: &str) -> Option<(Rgb, f32)> {
    let mut width = 0.75;
    let mut color = None;
    for part in v.split_whitespace() {
        if matches!(part, "none" | "hidden") {
            return None;
        }
        if let Some(c) = model::parse_color(part) {
            color = Some(c);
        } else if let Some(w) = length(part) {
            width = w.clamp(0.0, 20.0);
        }
    }
    Some((color.unwrap_or(Rgb(0, 0, 0)), width))
}

/// Columns of a `style:columns` element (ODF 1.3 §17.12): count and gap, clamped as the RTF and
/// DOCX readers do; equal widths only (`style:column` widths are not read).
pub(super) fn columns_of(a: &Attrs) -> Columns {
    let mut c = Columns::default();
    if let Some(n) = get(a, "fo:column-count").and_then(|v| v.trim().parse::<u64>().ok()) {
        c.count = n.clamp(1, 45) as u32;
    }
    if let Some(g) = get(a, "fo:column-gap").and_then(length).filter(|v| *v >= 0.0) {
        c.space = g.min(1584.0);
    }
    c
}

/// A `style:column-sep` (ODF 1.3 §17.13) draws a line between columns unless its style is "none".
pub(super) fn column_sep(a: &Attrs) -> bool {
    get(a, "style:style") != Some("none")
}

fn default_font(a: &Attrs) -> Option<String> {
    let family = get(a, "fo:font-family").or_else(|| get(a, "style:font-name"))?;
    let family = family.split(',').next().unwrap_or("").trim().trim_matches(['\'', '"']);
    (!family.is_empty() && !model::is_mono(family)).then(|| family.to_string())
}

fn text_properties(a: &Attrs, style: &mut StyleInfo) {
    let fmt = &mut style.fmt;
    let is_bold = |weight: &str| {
        let weight = weight.trim();
        weight.eq_ignore_ascii_case("bold") || weight.parse::<u32>().is_ok_and(|n| n >= 600)
    };
    // Latin attributes take precedence; Asian and complex-script attributes are fallbacks.
    if let Some(weight) = get(a, "fo:font-weight").or_else(|| get(a, "style:font-weight-asian")).or_else(|| get(a, "style:font-weight-complex")) {
        fmt.bold = Some(is_bold(weight));
    }
    let is_italic = |value: &str| value.trim().eq_ignore_ascii_case("italic") || value.trim().eq_ignore_ascii_case("oblique");
    if let Some(value) = get(a, "fo:font-style").or_else(|| get(a, "style:font-style-asian")).or_else(|| get(a, "style:font-style-complex")) {
        fmt.italic = Some(is_italic(value));
    }
    let is_not_none = |value: &str| !value.trim().eq_ignore_ascii_case("none");
    if let Some(value) = get(a, "style:text-underline-style").or_else(|| get(a, "style:text-underline-type")) {
        fmt.underline = Some(is_not_none(value));
    }
    if let Some(value) = get(a, "style:text-line-through-style").or_else(|| get(a, "style:text-line-through-type")) {
        fmt.strike = Some(is_not_none(value));
    }
    if let Some(value) = get(a, "style:text-position") {
        let first = value.split_whitespace().next().unwrap_or("");
        let pct = first.trim_end_matches('%').parse::<f32>().unwrap_or(0.0);
        fmt.sup = Some(first.eq_ignore_ascii_case("super") || pct > 0.0);
        fmt.sub = Some(first.eq_ignore_ascii_case("sub") || pct < 0.0);
    }
    if let Some(color) = get(a, "fo:color").and_then(model::parse_color) {
        fmt.color = Some(color);
    }
    if let Some(color) = get(a, "fo:background-color").and_then(model::parse_color) {
        fmt.background = Some(color);
    }
    if let Some(size) = get(a, "fo:font-size").and_then(length).filter(|size| (1.0..=1638.0).contains(size)) {
        fmt.size = Some(size);
    }
    if let Some(family) = get(a, "fo:font-family").or_else(|| get(a, "style:font-name")) {
        let family = family.split(',').next().unwrap_or("").trim().trim_matches(['\'', '"']).to_string();
        if model::is_mono(&family) || get(a, "style:font-pitch").is_some_and(|pitch| pitch.trim().eq_ignore_ascii_case("fixed")) {
            fmt.code = Some(true);
        } else if !family.is_empty() {
            fmt.font = model::kept_font(&family);
        }
    }
}

#[derive(Default)]
pub(super) struct Styles {
    map: HashMap<String, StyleInfo>,
    pub(super) lists: HashMap<String, Vec<bool>>,
    /// The levels of each list style: number format, label and indents.
    pub(super) list_levels: HashMap<String, Arc<Vec<Level>>>,
    /// Font of `style:default-style` for paragraphs (the document default, ODF 1.3 §16.4).
    default_font: Option<String>,
}

impl Styles {
    pub(super) fn read(&mut self, xml: &[u8], auto_section_is_auto: bool) {
        let mut in_auto = false;
        let mut cur: Option<(String, StyleInfo)> = None;
        let mut in_default = false;
        let mut list: Option<ListRead> = None;
        events(xml, |ev| match ev {
            Ev::Start(q, a) => match local(&q) {
                "automatic-styles" => in_auto = true,
                "style" if q.starts_with("style:") => {
                    if let Some(name) = get(&a, "style:name") {
                        let info = StyleInfo {
                            parent: get(&a, "style:parent-style-name").map(String::from),
                            display: get(&a, "style:display-name").map(String::from),
                            auto: in_auto && auto_section_is_auto,
                            outline: get(&a, "style:default-outline-level").and_then(|v| v.trim().parse::<u8>().ok()),
                            master_page: get(&a, "style:master-page-name").map(String::from),
                            ..Default::default()
                        };
                        cur = Some((name.to_string(), info));
                    }
                }
                "default-style" if q.starts_with("style:") => in_default = get(&a, "style:family") == Some("paragraph"),
                "text-properties" if in_default && cur.is_none() => {
                    self.default_font = default_font(&a);
                }
                "text-properties" => {
                    if let Some((_, s)) = &mut cur {
                        text_properties(&a, s);
                    }
                }
                "table-properties" => {
                    if let Some((_, s)) = &mut cur
                        && let Some(v) = get(&a, "style:page-number")
                    {
                        s.page_number = Some(v.trim().parse::<u32>().ok().map(|n| n.min(32_767)));
                    }
                }
                "table-column-properties" => {
                    if let Some((_, s)) = &mut cur
                        && let Some(w) = get(&a, "style:column-width").and_then(length)
                    {
                        s.column_width = Some(w);
                    }
                }
                "table-cell-properties" => {
                    if let Some((_, s)) = &mut cur {
                        if let Some(c) = get(&a, "fo:background-color").and_then(model::parse_color) {
                            s.cell_background = Some(c);
                        }
                        if let Some(va) = get(&a, "style:vertical-align") {
                            s.cell_valign = match va.trim() {
                                "top" => Some(VAlign::Top),
                                "middle" | "center" => Some(VAlign::Center),
                                "bottom" => Some(VAlign::Bottom),
                                _ => None,
                            };
                        }
                    }
                }
                "paragraph-properties" => {
                    if let Some((_, s)) = &mut cur {
                        s.align = match get(&a, "fo:text-align").map(|v| v.trim().to_ascii_lowercase()).as_deref() {
                            Some("center") => Some(Align::Center),
                            Some("end") | Some("right") => Some(Align::Right),
                            Some("justify") => Some(Align::Justify),
                            Some("start") | Some("left") => Some(Align::Left),
                            _ => s.align,
                        };
                        if get(&a, "fo:break-before").is_some_and(|b| b.trim().eq_ignore_ascii_case("page")) {
                            s.page_break = true;
                        }
                        match get(&a, "fo:keep-with-next") {
                            Some("always") => s.keep_next = Some(true),
                            Some("auto") => s.keep_next = Some(false),
                            _ => {}
                        }
                        if let Some(v) = get(&a, "style:page-number") {
                            s.page_number = Some(v.trim().parse::<u32>().ok().map(|n| n.min(32_767)));
                        }
                        if get(&a, "fo:border-bottom").is_some_and(|b| !b.trim().eq_ignore_ascii_case("none")) {
                            s.border_bottom = true;
                        }
                    }
                }
                "columns" if q.starts_with("style:") => {
                    if let Some((_, s)) = &mut cur {
                        s.columns = Some(columns_of(&a));
                    }
                }
                "graphic-properties" => {
                    if let Some((_, s)) = &mut cur {
                        s.graphic.read(&a);
                    }
                }
                "column-sep" => {
                    if let Some((_, s)) = &mut cur
                        && let Some(c) = &mut s.columns
                    {
                        c.separator = column_sep(&a);
                    }
                }
                "list-style" => list = get(&a, "style:name").map(ListRead::new),
                "list-level-style-number" | "list-level-style-bullet" | "list-level-style-image" | "list-level-label-alignment" => {
                    if let Some(l) = &mut list {
                        l.element(local(&q), &a);
                    }
                }
                _ => {}
            },
            Ev::End(q) => match local(&q) {
                "automatic-styles" => in_auto = false,
                "default-style" if q.starts_with("style:") => in_default = false,
                "style" if q.starts_with("style:") => {
                    if let Some((n, s)) = cur.take()
                        && self.map.len() < 100_000
                    {
                        self.map.insert(n, s);
                    }
                }
                "list-style" => {
                    if let Some(l) = list.take() {
                        self.add_list(l);
                    }
                }
                _ => {}
            },
            Ev::Text(_) => {}
        });
    }

    /// The document's default font: the Standard paragraph style's, else the default style's.
    pub(super) fn doc_font(&self) -> Option<String> {
        self.map.get("Standard").filter(|s| !s.auto).and_then(|s| s.fmt.font.as_deref().map(str::to_string)).or_else(|| self.default_font.clone())
    }

    fn chain<'a>(&'a self, name: &'a str) -> Vec<(&'a str, &'a StyleInfo)> {
        let mut out = Vec::new();
        let mut cur = Some(name);
        while let Some(n) = cur {
            if out.len() >= 16 {
                break;
            }
            let Some(s) = self.map.get(n) else { break };
            if out.iter().any(|(m, _): &(&str, &StyleInfo)| *m == n) {
                break;
            }
            out.push((n, s));
            cur = s.parent.as_deref();
        }
        out
    }

    /// Text formatting of a text style (whole chain).
    pub(super) fn text_fmt(&self, name: &str, base: &Fmt) -> Fmt {
        let mut f = base.clone();
        for (_, s) in self.chain(name).iter().rev() {
            s.fmt.apply(&mut f);
        }
        f
    }

    /// The page a paragraph style starts (`style:master-page-name` and `style:page-number`,
    /// ODF 1.3 §16.2 and §17.6): the nearest master page and page-number setting in its chain.
    pub(super) fn page_start<'a>(&'a self, name: &'a str) -> (Option<&'a str>, Option<Option<u32>>) {
        let chain = self.chain(name);
        let master = chain.iter().find_map(|(_, s)| s.master_page.as_deref()).filter(|m| !m.is_empty());
        (master, chain.iter().find_map(|(_, s)| s.page_number))
    }

    /// Columns of a section style (whole chain).
    pub(super) fn columns(&self, name: &str) -> Option<Columns> {
        self.chain(name).iter().find_map(|(_, s)| s.columns.clone())
    }

    /// A list style read: whether its levels are numbered, and the levels it defines.
    fn add_list(&mut self, l: ListRead) {
        if let Some(levels) = l.levels() {
            self.list_levels.insert(l.name.clone(), Arc::new(levels));
        }
        self.lists.insert(l.name, l.ordered);
    }

    /// Placement and look of a frame or shape style (whole chain).
    pub(super) fn graphic(&self, name: &str) -> Graphic {
        self.chain(name).iter().fold(Graphic::default(), |g, (_, s)| g.or(&s.graphic))
    }

    /// Width in points of a table-column style (whole chain).
    pub(super) fn column_width(&self, name: &str) -> Option<f32> {
        self.chain(name).iter().find_map(|(_, s)| s.column_width)
    }

    /// Background colour of a table-cell style (whole chain).
    pub(super) fn cell_background(&self, name: &str) -> Option<Rgb> {
        self.chain(name).iter().find_map(|(_, s)| s.cell_background)
    }

    /// Vertical alignment of a table-cell style (whole chain).
    pub(super) fn cell_valign(&self, name: &str) -> Option<VAlign> {
        self.chain(name).iter().find_map(|(_, s)| s.cell_valign)
    }

    /// Paragraph kind, alignment, page break, rule, keep-with-next and base formatting. The text formatting of
    /// every style in the chain applies (automatic and common, ODF 1.3 §16.2 inheritance), except
    /// the style that gives the paragraph its kind and that style's ancestors: their formatting
    /// is the kind's own look (a heading's size and colour), not direct formatting. `Standard`,
    /// the default paragraph style, holds document defaults and is left out too.
    pub(super) fn para(&self, name: &str) -> (Kind, Option<Align>, bool, bool, bool, Fmt) {
        let chain = self.chain(name);
        let mut kind = Kind::Normal;
        let mut kind_at = chain.len();
        for (i, (n, s)) in chain.iter().enumerate() {
            kind_at = i;
            let dn = s.display.as_deref().unwrap_or(n).to_ascii_lowercase().replace("_20_", " ");
            let nn = n.to_ascii_lowercase().replace("_20_", " ");
            for cand in [dn.as_str(), nn.as_str()] {
                if let Some(d) = cand.strip_prefix("heading ").and_then(|r| r.trim().parse::<u8>().ok()) {
                    kind = Kind::Heading(d.clamp(1, 6));
                } else if cand == "title" {
                    kind = Kind::Title;
                } else if cand == "quotations" || cand == "quote" {
                    kind = Kind::Quote;
                } else if cand == "preformatted text" || cand == "source text" || cand == "code" {
                    kind = Kind::Code;
                }
            }
            if kind != Kind::Normal {
                break;
            }
            if let Some(o) = s.outline.filter(|o| (1..=10).contains(o)) {
                kind = Kind::Heading(o.min(6));
                break;
            }
            kind_at = chain.len();
        }
        let align = chain.iter().find_map(|(_, s)| s.align);
        let pb = chain.iter().take_while(|(_, s)| s.auto).any(|(_, s)| s.page_break);
        let rule = chain.iter().any(|(_, s)| s.border_bottom);
        let mut f = Fmt::default();
        for (_, s) in chain.iter().take(kind_at).rev().filter(|(n, s)| s.auto || *n != "Standard") {
            s.fmt.apply(&mut f);
        }
        // Like the text formatting, the kind style's own `fo:keep-with-next` (a heading's) is the kind's look, not direct formatting.
        let keep_next = chain.iter().take(kind_at).find_map(|(_, s)| s.keep_next).unwrap_or(false);
        (kind, align, pb, rule, keep_next, f)
    }
}
