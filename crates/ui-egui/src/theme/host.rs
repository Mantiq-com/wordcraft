//! Host-supplied theme overrides for embedded browser sessions (`ui.hostTheme`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use craft::theme::{
    ColorToken, ModePins, ModeSeeds, Pins, Radius, RadiusSet, Rgba, SUITE_DEFAULT, SeedOverrides, ShapeSeed, ThemeDef, ThemeId, Variants, builtins,
};
use egui::Color32;

use super::Appearance;

/// How a page that embeds WordCraft wants the interface to look: its own colours over each
/// palette, its corner radius, its light or dark appearance and whether the community or Share
/// buttons and specific tabs show. It belongs to the visit, never to the saved preferences, so
/// WordCraft opened on its own keeps its look.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HostTheme {
    pub light: Vec<(&'static str, Color32)>,
    pub dark: Vec<(&'static str, Color32)>,
    /// The corner radius of buttons and fields, in points; menus and windows are rounder by the
    /// same steps as ours (4, 6 and 8).
    pub radius: Option<u8>,
    pub appearance: Option<Appearance>,
    pub community: Option<bool>,
    /// Whether the Share button shows: a host that keeps the files itself may hide it.
    pub share: Option<bool>,
    /// Ribbon tabs the host hides, by their English names ("Citations", "Developer"; "Zotero" is
    /// accepted as a legacy alias for "Citations").
    pub hidden_tabs: Vec<String>,
    /// The interface faces: regular text, and the medium and semibold weights of labels and
    /// headings. A weight not given falls back to ours.
    pub fonts: HostFonts,
}

/// The host page's interface faces, as plain OpenType bytes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HostFonts {
    pub regular: Option<Arc<Vec<u8>>>,
    pub medium: Option<Arc<Vec<u8>>>,
    pub semibold: Option<Arc<Vec<u8>>>,
}

/// One interface face from the host: base64 of a TTF, OTF, WOFF or WOFF2 file, unwrapped and
/// checked to hold a face that draws Latin text.
fn host_font(field: &str, v: &serde_json::Value) -> Result<Arc<Vec<u8>>, String> {
    let data = v.as_str().ok_or_else(|| format!("{field} must be the base64 of a font file"))?;
    let bytes = wordcraft_engine::cmd::insert::base64_decode(data).ok_or_else(|| format!("{field} is not valid base64"))?;
    let font = wordcraft_fonts::decode_font_file(&bytes).map_err(|e| format!("{field}: {e}"))?;
    let face = skrifa::FontRef::new(&font).map_err(|e| format!("{field}: the font can't be read ({e})"))?;
    use skrifa::MetadataProvider as _;
    if face.charmap().map('a').is_none() {
        return Err(format!("{field}: the font has no Latin letters"));
    }
    Ok(Arc::new(font))
}

/// The palette entries a host may set, by the names the `ui.hostTheme` command takes.
const HOST_TOKENS: &[&str] = &[
    "titleBar",
    "ribbon",
    "tabStrip",
    "canvas",
    "panel",
    "border",
    "borderStrong",
    "text",
    "textDim",
    "textDisabled",
    "icon",
    "hover",
    "pressed",
    "checked",
    "accent",
    "nav",
    "accentText",
    "onAccent",
    "input",
    "inputBorder",
    "groupLabel",
    "ruler",
    "rulerMargin",
    "rulerTick",
    "statusBar",
    "selection",
    "menu",
    "balloon",
    "markupArea",
    "markupEdge",
    "pageShadow",
];

struct StoredHostTheme {
    theme: HostTheme,
    def: Option<Arc<ThemeDef>>,
}

static HOST_THEME: RwLock<Option<StoredHostTheme>> = RwLock::new(None);
static HOST_THEME_GENERATION: AtomicU64 = AtomicU64::new(0);

fn host_theme() -> std::sync::RwLockReadGuard<'static, Option<StoredHostTheme>> {
    // A panic while the theme was written leaves a whole value: each write replaces it at once.
    HOST_THEME.read().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Monotonic generation bumped on every `set_host_theme` call, so UI fonts and cached themes
/// refresh whenever the host sets or clears its theme.
pub fn host_theme_generation() -> u64 {
    HOST_THEME_GENERATION.load(Ordering::Relaxed)
}

/// The host page's theme, if one was set.
pub fn current_host_theme() -> Option<HostTheme> {
    host_theme().as_ref().map(|stored| stored.theme.clone())
}

/// The framework `ThemeDef` carrying the host's colours and corner radius, if the host set any.
pub fn current_host_theme_def() -> Option<Arc<ThemeDef>> {
    host_theme().as_ref().and_then(|stored| stored.def.clone())
}

/// The appearance the host page asks for, which wins over the interface theme setting.
pub fn host_appearance() -> Option<Appearance> {
    host_theme().as_ref().and_then(|stored| stored.theme.appearance)
}

/// Whether the community button may show: a host page can hide it.
pub fn host_shows_community() -> bool {
    host_theme().as_ref().and_then(|stored| stored.theme.community).unwrap_or(true)
}

/// Whether the Share button may show: a host page can hide it.
pub fn host_shows_share() -> bool {
    host_theme().as_ref().and_then(|stored| stored.theme.share).unwrap_or(true)
}

/// Whether the host page hides the ribbon tab `name`. Accepts "Zotero" as a legacy alias for the
/// renamed "Citations" tab so existing hosts keep hiding it.
pub fn host_hides_tab(name: &str) -> bool {
    host_theme().as_ref().is_some_and(|stored| {
        stored
            .theme
            .hidden_tabs
            .iter()
            .any(|tab| tab.eq_ignore_ascii_case(name) || (name.eq_ignore_ascii_case("Citations") && tab.eq_ignore_ascii_case("Zotero")))
    })
}

/// Replace the host page's theme; `None` returns to WordCraft's own look.
pub fn set_host_theme(theme: Option<HostTheme>) {
    let generation = HOST_THEME_GENERATION.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
    let stored = theme.map(|theme| {
        let def = theme.to_theme_def(generation);
        StoredHostTheme { theme, def }
    });
    *HOST_THEME.write().unwrap_or_else(std::sync::PoisonError::into_inner) = stored;
}

impl HostTheme {
    /// Read a host theme from its JSON form:
    /// `{"light"?: {token: "#rrggbb" | "#rrggbbaa"}, "dark"?: {..}, "radius"?: 0-16,
    /// "appearance"?: "light|dark|system", "community"?: bool, "share"?: bool,
    /// "hideTabs"?: [string], "font"|"fontMedium"|"fontSemibold"?: base64}`.
    /// Unknown tokens and malformed colours are refused, so a host learns of a typo at once.
    pub fn from_json(v: &serde_json::Value) -> Result<HostTheme, String> {
        let obj = v.as_object().ok_or("the host theme must be an object")?;
        for key in obj.keys() {
            if !["light", "dark", "radius", "appearance", "community", "share", "hideTabs", "font", "fontMedium", "fontSemibold"]
                .contains(&key.as_str())
            {
                return Err(format!("unknown host theme field {key:?}"));
            }
        }
        let palette = |key: &str| -> Result<Vec<(&'static str, Color32)>, String> {
            let Some(colors) = obj.get(key) else { return Ok(Vec::new()) };
            let colors = colors.as_object().ok_or_else(|| format!("{key} must map tokens to colours"))?;
            colors
                .iter()
                .map(|(name, color)| {
                    let token = HOST_TOKENS
                        .iter()
                        .find(|token| **token == name.as_str())
                        .ok_or_else(|| format!("unknown palette token {name:?}; use one of {}", HOST_TOKENS.join(", ")))?;
                    let hex = color.as_str().ok_or_else(|| format!("{name}: the colour must be a \"#rrggbb\" string"))?;
                    let color = parse_hex(hex).ok_or_else(|| format!("{name}: {hex:?} is not a #rrggbb or #rrggbbaa colour"))?;
                    Ok((*token, color))
                })
                .collect()
        };
        let radius = match obj.get("radius") {
            None | Some(serde_json::Value::Null) => None,
            Some(r) => Some(r.as_u64().filter(|r| *r <= 16).ok_or("radius must be a whole number from 0 to 16")? as u8),
        };
        let appearance = match obj.get("appearance").and_then(serde_json::Value::as_str) {
            None => None,
            Some(code) => Some(Appearance::from_code(code).ok_or_else(|| format!("unknown appearance {code:?}; use light, dark or system"))?),
        };
        let community = obj.get("community").and_then(serde_json::Value::as_bool);
        let share = obj.get("share").and_then(serde_json::Value::as_bool);
        let hidden_tabs = match obj.get("hideTabs") {
            None | Some(serde_json::Value::Null) => Vec::new(),
            Some(tabs) => tabs
                .as_array()
                .ok_or("hideTabs must be a list of tab names")?
                .iter()
                .map(|tab| tab.as_str().map(str::to_string).ok_or("hideTabs must be a list of tab names"))
                .collect::<Result<_, _>>()?,
        };
        let font = |key: &str| obj.get(key).filter(|v| !v.is_null()).map(|v| host_font(key, v)).transpose();
        let fonts = HostFonts { regular: font("font")?, medium: font("fontMedium")?, semibold: font("fontSemibold")? };
        Ok(HostTheme { light: palette("light")?, dark: palette("dark")?, radius, appearance, community, share, hidden_tabs, fonts })
    }

    /// Build a framework `ThemeDef` carrying the host's palette and corner radius over the suite
    /// default theme. Returns `None` when the host set neither colours nor radius.
    pub fn to_theme_def(&self, generation: u64) -> Option<Arc<ThemeDef>> {
        if self.light.is_empty() && self.dark.is_empty() && self.radius.is_none() {
            return None;
        }
        let base = builtins().iter().find(|t| t.id.as_str() == SUITE_DEFAULT).or_else(|| builtins().first())?;
        let mut def = base.clone();
        if let Ok(id) = ThemeId::parse(&format!("host.theme-{generation}")) {
            def.id = id;
        }
        def.name = "Host Theme".to_string();
        def.variants = Variants::Both;
        // Clear built-in app pins so the host's own pins govern every token it sets.
        def.apps.clear();
        def.mode_seeds =
            ModeSeeds { light: mode_seed_overrides(&def.mode_seeds.light, &self.light), dark: mode_seed_overrides(&def.mode_seeds.dark, &self.dark) };
        def.pins = ModePins { light: mode_pins(&def.pins.light, &self.light, false), dark: mode_pins(&def.pins.dark, &self.dark, true) };
        if let Some(radius) = self.radius {
            let r = f32::from(radius);
            def.shape = Some(ShapeSeed {
                radius: Some(RadiusSet {
                    sm: (r * 0.5).round(),
                    control: r,
                    surface: r + 2.0,
                    window: r + 4.0,
                    panel: 0.0,
                    button: r,
                    pill: Radius::Full.px(),
                }),
                radius_offset: None,
                button_radius: Some(r),
                edge: None,
                knob: None,
                wells: None,
                grip: None,
                locator: None,
            });
        }
        Some(Arc::new(def))
    }
}

fn to_rgba(c: Color32) -> Rgba {
    Rgba(c.to_srgba_unmultiplied())
}

fn to_opaque_rgba(c: Color32) -> Rgba {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    Rgba::rgb(r, g, b)
}

fn mode_seed_overrides(base: &SeedOverrides, colors: &[(&'static str, Color32)]) -> SeedOverrides {
    let mut seeds = *base;
    if let Some((_, accent)) = colors.iter().find(|(token, _)| *token == "accent") {
        seeds.accent = Some(to_opaque_rgba(*accent));
    }
    seeds
}

fn mode_pins(base: &Pins, colors: &[(&'static str, Color32)], dark: bool) -> Pins {
    let mut pins = base.clone();
    let mut pin = |token: ColorToken, color: Color32| {
        let rgba = if token.is_opaque_surface() { to_opaque_rgba(color) } else { to_rgba(color) };
        pins.refs.remove(&token);
        pins.colors.insert(token, rgba);
    };
    let find = |name: &str| colors.iter().find(|(token, _)| *token == name).map(|(_, color)| *color);

    // Apply coarse fallbacks first, then more specific tokens so explicit choices win.
    if let Some(c) = find("markupArea") {
        pin(ColorToken::SurfaceCanvas, c);
        pin(ColorToken::SurfaceBackground, c);
    }
    if let Some(c) = find("canvas") {
        pin(ColorToken::SurfaceCanvas, c);
        pin(ColorToken::SurfaceBackground, c);
    }
    if let Some(c) = find("panel") {
        pin(ColorToken::SurfacePanel, c);
        pin(ColorToken::SurfaceChrome, c);
    }
    if let Some(c) = find("ribbon") {
        pin(ColorToken::SurfacePanel, c);
    }
    if let Some(c) = find("statusBar") {
        pin(ColorToken::SurfaceChrome, c);
    }
    if let Some(c) = find("tabStrip") {
        pin(ColorToken::SurfaceChrome, c);
    }
    if let Some(c) = find("titleBar") {
        pin(ColorToken::SurfaceChrome, c);
    }
    if let Some(c) = find("balloon") {
        pin(ColorToken::SurfaceRaised, c);
    }
    if let Some(c) = find("menu") {
        pin(ColorToken::SurfaceRaised, c);
    }
    if let Some(c) = find("input") {
        pin(ColorToken::SurfaceSunken, c);
    }
    if let Some(c) = find("pageShadow") {
        pin(ColorToken::SurfaceShadow, c);
    }
    if let Some(c) = find("icon") {
        pin(ColorToken::TextNormal, c);
    }
    if let Some(c) = find("text") {
        pin(ColorToken::TextNormal, c);
    }
    if let Some(c) = find("rulerTick").or_else(|| find("groupLabel")).or_else(|| find("textDim")) {
        pin(ColorToken::TextMuted, c);
        pin(ColorToken::TextFaint, c);
    }
    if let Some(c) = find("textDisabled") {
        pin(ColorToken::TextDisabled, c);
    }
    if let Some(c) = find("markupEdge").or_else(|| find("border")) {
        pin(ColorToken::LineDivider, c);
        pin(ColorToken::LineBorder, c);
    }
    if let Some(c) = find("borderStrong") {
        pin(ColorToken::LineBorder, c);
    }
    if let Some(c) = find("inputBorder") {
        pin(ColorToken::LineControlEdge, c);
    }
    if let Some(c) = find("hover") {
        pin(ColorToken::StateHover, c);
    }
    if let Some(c) = find("pressed") {
        pin(ColorToken::StatePressed, c);
    }
    if let Some(accent) = find("accent") {
        pin(ColorToken::AccentSolid, accent);
        pin(ColorToken::AccentStrong, accent);
        pin(ColorToken::AccentBorder, accent);
        pin(ColorToken::LineFocus, accent);
        if find("selection").is_none() {
            let sel = Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), if dark { 0x55 } else { 0x48 });
            pin(ColorToken::StateTextSelection, sel);
        }
        if find("onAccent").is_none() {
            let on = if luminance(accent) > 0.4 { Color32::from_rgb(0x11, 0x11, 0x11) } else { Color32::WHITE };
            pin(ColorToken::TextOnAccent, on);
            pin(ColorToken::TextOnStrong, on);
        }
    }
    if let Some(c) = find("checked") {
        pin(ColorToken::AccentSoft, c);
        pin(ColorToken::StateSelected, c);
    }
    if let Some(c) = find("accentText") {
        pin(ColorToken::AccentText, c);
        pin(ColorToken::AccentOnSoft, c);
    }
    if let Some(c) = find("onAccent") {
        pin(ColorToken::TextOnAccent, c);
        pin(ColorToken::TextOnStrong, c);
    }
    if let Some(c) = find("selection") {
        pin(ColorToken::StateTextSelection, c);
    }
    pins
}

/// `#rrggbb` or `#rrggbbaa`.
fn parse_hex(hex: &str) -> Option<Color32> {
    let digits = hex.trim().strip_prefix('#')?;
    if !digits.is_ascii() || !(digits.len() == 6 || digits.len() == 8) {
        return None;
    }
    let byte = |i: usize| digits.get(i..i + 2).and_then(|pair| u8::from_str_radix(pair, 16).ok());
    let alpha = if digits.len() == 8 { byte(6)? } else { 0xFF };
    Some(Color32::from_rgba_unmultiplied(byte(0)?, byte(2)?, byte(4)?, alpha))
}

/// Relative luminance, 0 (black) to 1 (white).
fn luminance(c: Color32) -> f32 {
    let channel = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
}

#[cfg(test)]
mod tests {
    use craft::theme::{Appearance as ThemeAppearance, Contrast, Mode, Rgba, Size, TextSize, resolve};

    use super::*;

    fn host(v: serde_json::Value) -> HostTheme {
        HostTheme::from_json(&v).unwrap()
    }

    fn resolve_mode(def: &ThemeDef, mode: Mode) -> craft::theme::Tokens {
        resolve(
            def,
            ThemeAppearance { mode, contrast: Contrast::Standard, size: Size::Compact, text_size: TextSize::Small, reduced_motion: false },
            None,
        )
        .tokens
    }

    #[test]
    fn a_host_palette_paints_over_ours_in_its_own_appearance_only() {
        let theme = host(serde_json::json!({
            "light": {"ribbon": "#fae5d6", "text": "#1a1a1a"},
            "dark": {"ribbon": "#202020"}
        }));
        let def = theme.to_theme_def(1).unwrap();
        let light = resolve_mode(&def, Mode::Light);
        assert_eq!(light.color.surface.panel, Rgba::rgb(0xFA, 0xE5, 0xD6));
        assert_eq!(light.color.text.normal, Rgba::rgb(0x1A, 0x1A, 0x1A));
        let dark = resolve_mode(&def, Mode::Dark);
        assert_eq!(dark.color.surface.panel, Rgba::rgb(0x20, 0x20, 0x20));
        assert_ne!(dark.color.text.normal, Rgba::rgb(0x1A, 0x1A, 0x1A), "the light palette never reaches the dark one");
    }

    #[test]
    fn an_accent_alone_tints_what_follows_from_it_unless_given() {
        let def = host(serde_json::json!({"light": {"accent": "#885f1a"}})).to_theme_def(2).unwrap();
        let tokens = resolve_mode(&def, Mode::Light);
        assert_eq!(tokens.color.accent.solid, Rgba::rgb(0x88, 0x5F, 0x1A));
        let [r, g, b, _] = tokens.color.state.text_selection.0;
        assert!([r, g, b].iter().zip([0x88_u8, 0x5F, 0x1A]).all(|(got, want)| got.abs_diff(want) <= 2));
        assert_eq!(tokens.color.text.on_strong, Rgba::WHITE);

        let def_override = host(serde_json::json!({"light": {"accent": "#885f1a", "onAccent": "#000000"}})).to_theme_def(3).unwrap();
        let tokens_override = resolve_mode(&def_override, Mode::Light);
        assert_eq!(tokens_override.color.text.on_strong, Rgba::BLACK);
    }

    #[test]
    fn host_radius_applies_to_resolved_shape_tokens() {
        let def = host(serde_json::json!({"radius": 8})).to_theme_def(4).unwrap();
        let tokens = resolve_mode(&def, Mode::Light);
        assert_eq!(tokens.shape.radius.control, 8.0);
        assert_eq!(tokens.shape.radius.button, 8.0);
        assert_eq!(tokens.shape.radius.surface, 10.0);
        assert_eq!(tokens.shape.radius.window, 12.0);
    }

    #[test]
    fn a_host_theme_refuses_what_it_does_not_know() {
        let refused = |v: serde_json::Value| HostTheme::from_json(&v).unwrap_err();
        assert!(refused(serde_json::json!({"light": {"ribon": "#ffffff"}})).contains("ribon"));
        assert!(refused(serde_json::json!({"light": {"ribbon": "white"}})).contains("ribbon"));
        assert!(refused(serde_json::json!({"light": {"ribbon": "#ffff"}})).contains("ribbon"));
        assert!(refused(serde_json::json!({"radius": 40})).contains("radius"));
        assert!(refused(serde_json::json!({"appearance": "sepia"})).contains("sepia"));
        assert!(refused(serde_json::json!({"colours": {}})).contains("colours"));
        assert!(refused(serde_json::json!({"font": "not base64!"})).contains("font"));
        let theme = host(serde_json::json!({
            "light": {"selection": "#885f1a48"},
            "radius": 8,
            "appearance": "dark",
            "community": false,
            "share": false,
            "hideTabs": ["Zotero"]
        }));
        assert_eq!(theme.light, vec![("selection", Color32::from_rgba_unmultiplied(0x88, 0x5F, 0x1A, 0x48))]);
        assert_eq!(theme.radius, Some(8));
        assert_eq!(theme.appearance, Some(Appearance::Dark));
        assert_eq!((theme.community, theme.share), (Some(false), Some(false)));
        assert_eq!(theme.hidden_tabs, vec!["Zotero".to_string()]);
    }

    #[test]
    fn zotero_hide_tab_alias_hides_citations() {
        let theme = host(serde_json::json!({"hideTabs": ["Zotero", "Developer"]}));
        set_host_theme(Some(theme));
        assert!(host_hides_tab("Citations"));
        assert!(host_hides_tab("Developer"));
        assert!(!host_hides_tab("Home"));
        set_host_theme(None);
    }

    #[test]
    fn a_host_font_is_unwrapped_and_checked() {
        let ttf = include_bytes!("../../../../assets/fonts/Inter-Regular.ttf");
        let theme = host(serde_json::json!({"font": wordcraft_engine::cmd::insert::base64_encode(ttf)}));
        assert_eq!(theme.fonts.regular.as_deref().map(Vec::len), Some(ttf.len()));
        assert!(theme.fonts.medium.is_none());
        let junk = wordcraft_engine::cmd::insert::base64_encode(b"not a font at all");
        assert!(HostTheme::from_json(&serde_json::json!({"font": junk})).is_err());
    }
}
