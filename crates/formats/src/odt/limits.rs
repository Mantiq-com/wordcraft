/// Largest zip entry we inflate.
pub(super) const MAX_ENTRY: u64 = 256 << 20;

/// Deepest element nesting handled in content.xml.
pub(super) const MAX_XML_DEPTH: usize = 512;

/// Most sections an import makes (page-style changes and column sections); later ones are ignored.
pub(super) const MAX_SECTIONS: usize = 10_000;

/// Most `text:changed-region`s read (hostile input).
pub(super) const MAX_REGIONS: usize = 10_000;

/// Most insertions open at once (`text:change-start` without its end yet).
pub(super) const MAX_OPEN_INSERTIONS: usize = 64;

/// Longest change author or date kept, in characters.
pub(super) const MAX_INFO_CHARS: usize = 256;

/// Most comments read from one document; later ones are left out.
pub(super) const MAX_COMMENTS: usize = 10_000;

/// Most notes, comments, text boxes and shapes open inside one another (a text box in a shape's
/// text…); one nested deeper is left out.
pub(super) const MAX_STORY_DEPTH: usize = 8;
