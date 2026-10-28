//! Headers and footers of the master pages in `styles.xml` (ODF 1.3 §16.10), read with the body
//! reader (#863).
use std::collections::{HashMap, HashSet};

use wordcraft_doc::{Block, Document, PartKind};

use super::{Body, Ev, FBlock, Pages, SectionProps, Styles, events, get, model};

/// Most XML events kept for all headers and footers together; a slot past it is left out.
const MAX_EVENTS: usize = 1 << 20;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    Header,
    HeaderLeft,
    HeaderFirst,
    Footer,
    FooterLeft,
    FooterFirst,
}

impl Slot {
    fn of(q: &str) -> Option<Self> {
        Some(match q {
            "style:header" => Self::Header,
            "style:header-left" => Self::HeaderLeft,
            "style:header-first" | "loext:header-first" => Self::HeaderFirst,
            "style:footer" => Self::Footer,
            "style:footer-left" => Self::FooterLeft,
            "style:footer-first" | "loext:footer-first" => Self::FooterFirst,
            _ => return None,
        })
    }

    fn footer(self) -> bool {
        matches!(self, Self::Footer | Self::FooterLeft | Self::FooterFirst)
    }

    fn set(self, sec: &mut SectionProps, id: u32) {
        let set = if self.footer() { &mut sec.footers } else { &mut sec.headers };
        match self {
            Self::Header | Self::Footer => set.default = Some(id),
            Self::HeaderLeft | Self::FooterLeft => set.even = Some(id),
            Self::HeaderFirst | Self::FooterFirst => {
                set.first = Some(id);
                sec.title_page = true;
            }
        }
    }
}

/// The headers and footers read, in part-id order from 1, and whether one is for left pages.
#[derive(Default)]
pub(super) struct Parts {
    parts: Vec<(PartKind, Vec<FBlock>)>,
    even_odd: bool,
}

/// Read each visible header and footer of the master pages in `pages`, and name it on its master
/// page's section as part id 1, 2, … in order, so the sections the body starts carry them.
/// [`add`] puts those parts into the document.
pub(super) fn read(xml: &[u8], styles: &Styles, pages: &mut Pages, fetch: &mut dyn FnMut(&str) -> Option<Vec<u8>>) -> Parts {
    let mut found: Vec<(String, Slot, Vec<Ev>)> = Vec::new();
    let mut master: Option<String> = None;
    // The slot being read, its nesting depth and whether it is kept.
    let mut slot: Option<(usize, bool)> = None;
    let mut seen = HashSet::new();
    let mut room = MAX_EVENTS;
    events(xml, |ev| {
        if let Some((depth, keep)) = &mut slot {
            match &ev {
                Ev::End(_) if *depth == 0 => {
                    slot = None;
                    return;
                }
                Ev::Start(..) => *depth += 1,
                Ev::End(_) => *depth -= 1,
                Ev::Text(_) => {}
            }
            if *keep && room == 0 {
                *keep = false;
                found.pop();
            }
            if *keep && let Some((.., evs)) = found.last_mut() {
                room -= 1;
                evs.push(ev);
            }
            return;
        }
        match ev {
            Ev::Start(q, a) if q == "style:master-page" => {
                master = get(&a, "style:name").filter(|n| pages.masters.contains_key(*n)).map(str::to_string);
            }
            Ev::End(q) if q == "style:master-page" => master = None,
            Ev::Start(q, a) => {
                if let (Some(m), Some(kind)) = (&master, Slot::of(&q)) {
                    let keep = get(&a, "style:display") != Some("false") && seen.insert((m.clone(), q.clone()));
                    if keep {
                        found.push((m.clone(), kind, Vec::new()));
                    }
                    slot = Some((0, keep));
                }
            }
            _ => {}
        }
    });
    let none = Pages::default();
    let mut out = Parts::default();
    for (name, kind, evs) in found {
        let mut reader = Body::new(styles, &none, fetch);
        reader.in_text = true;
        for ev in evs {
            reader.feed(ev);
        }
        out.parts.push((if kind.footer() { PartKind::Footer } else { PartKind::Header }, reader.take_blocks()));
        if let Some(sec) = pages.masters.get_mut(&name) {
            kind.set(sec, out.parts.len() as u32);
        }
        out.even_odd |= matches!(kind, Slot::HeaderLeft | Slot::FooterLeft);
    }
    // Each master page stands alone, but a section without its own header or footer continues
    // the previous one's: give those an empty one, and left pages the right pages' one.
    for footer in [false, true] {
        let pick = |s: &SectionProps| if footer { s.footers } else { s.headers };
        if !pages.masters.values().any(|s| pick(s).default.is_some() || pick(s).even.is_some()) {
            continue;
        }
        let mut empty = None;
        for sec in pages.masters.values_mut() {
            let set = if footer { &mut sec.footers } else { &mut sec.headers };
            if set.default.is_none() {
                set.default = Some(*empty.get_or_insert_with(|| {
                    out.parts.push((if footer { PartKind::Footer } else { PartKind::Header }, Vec::new()));
                    out.parts.len() as u32
                }));
            }
            if out.even_odd && set.even.is_none() {
                set.even = set.default;
            }
        }
    }
    if let Some(first) = pages.first_name.as_ref().and_then(|n| pages.masters.get(n)) {
        pages.first = Some(first.clone());
    }
    out
}

/// Put the parts [`read`] found into `doc`, and point its sections at them. [`read`] numbered them
/// 1, 2, … in order, but [`model::to_doc`] may have given those ids to notes and text boxes, so
/// each gets a free id and the sections are renumbered.
pub(super) fn add(doc: &mut Document, parts: Parts) {
    let mut ids = HashMap::new();
    for (i, (kind, blocks)) in parts.parts.into_iter().enumerate() {
        let blocks = model::part_blocks(doc, &blocks);
        ids.insert(i as u32 + 1, doc.add_part(kind, blocks));
    }
    renumber(&mut doc.last_section, &ids);
    for block in &mut doc.body {
        if let Block::Para(p) = &**block
            && p.section.is_some()
            && let Block::Para(p) = std::sync::Arc::make_mut(block)
            && let Some(sec) = &mut p.section
        {
            renumber(sec, &ids);
        }
    }
    doc.settings.even_odd_headers |= parts.even_odd;
}

/// Point the headers and footers of `sec` at the ids `ids` gives the ones [`read`] numbered.
fn renumber(sec: &mut SectionProps, ids: &HashMap<u32, u32>) {
    for set in [&mut sec.headers, &mut sec.footers] {
        for slot in [&mut set.default, &mut set.even, &mut set.first] {
            if let Some(id) = slot {
                *slot = ids.get(id).copied();
            }
        }
    }
}
