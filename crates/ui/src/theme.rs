//! Shared color schemes for every Nozhik window.
//!
//! The source of truth is `crates/ui/assets/color-schemes.json`. Paint code reads
//! the active scheme through the functions in this module. Corner radius and
//! splitter width stay shared so layout does not change with the palette.
use egui::{Color32, Context, FontId, Stroke, TextFormat, Visuals};
use serde::Deserialize;
use std::{cell::RefCell, sync::Arc, sync::OnceLock};

/// Logical corner radius shared by controls and windows.
pub const RADIUS: u8 = 8;
/// Width of visible draggable panel separators in logical pixels.
pub const SPLITTER: f32 = 5.0;

/// Built-in schemes shown in Settings. Ids match `Config.theme`.
pub const CHOICES: &[(&str, &str)] = &[
    ("dark", "New UI Dark"),
    ("light", "New UI Light"),
    ("catppuccin", "Catppuccin"),
];

/// Problem-card tone. Fills and icons come from the active scheme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeverityTone {
    /// Informational notice.
    Info,
    /// Warning that does not block the task.
    Warning,
    /// Failed operation.
    Error,
    /// Security block or a stopped worker.
    Critical,
}

#[derive(Clone, Debug)]
struct Scheme {
    id: String,
    name: String,
    light: bool,
    disabled_alpha: f32,
    chrome: Color32,
    panel: Color32,
    background: Color32,
    header: Color32,
    surface: Color32,
    input: Color32,
    border: Color32,
    border_strong: Color32,
    hover: Color32,
    selected: Color32,
    primary: Color32,
    primary_text: Color32,
    on_selected: Color32,
    text: Color32,
    text_strong: Color32,
    muted: Color32,
    disabled: Color32,
    success: Color32,
    error: Color32,
    warning: Color32,
    info: Color32,
    destructive: Color32,
    diff_hunk: Color32,
    diff_add: Color32,
    diff_delete: Color32,
    caret: Color32,
    chip: Color32,
    chip_on_selected: Color32,
    chip_ink: Color32,
    line_number: Color32,
    icon_ink: Color32,
    link: Color32,
    openapi: Color32,
    button: Color32,
    syntax: Syntax,
    ansi: [Color32; 16],
    http: Http,
    severity: Severity,
}

#[derive(Clone, Copy, Debug)]
struct Syntax {
    keyword: Color32,
    string: Color32,
    comment: Color32,
    function: Color32,
    r#type: Color32,
    number: Color32,
    variable: Color32,
    tag: Color32,
    property: Color32,
    selector: Color32,
    constant: Color32,
    css_color: Color32,
    css_value: Color32,
    heading: Color32,
    strong: Color32,
    emphasis: Color32,
    raw: Color32,
}

#[derive(Clone, Copy, Debug)]
struct Http {
    get: Color32,
    post: Color32,
    put: Color32,
    patch: Color32,
    delete: Color32,
    head: Color32,
    options: Color32,
    other: Color32,
}

#[derive(Clone, Copy, Debug)]
struct Severity {
    info_fill: Color32,
    warning_fill: Color32,
    error_fill: Color32,
    critical_fill: Color32,
    info: Color32,
    warning: Color32,
    error: Color32,
    critical: Color32,
    on: Color32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SchemeFile {
    schemes: Vec<RawScheme>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawScheme {
    id: String,
    name: String,
    appearance: String,
    disabled_alpha: f32,
    chrome: String,
    panel: String,
    background: String,
    header: String,
    surface: String,
    input: String,
    border: String,
    border_strong: String,
    hover: String,
    selected: String,
    primary: String,
    primary_text: String,
    on_selected: String,
    text: String,
    text_strong: String,
    muted: String,
    disabled: String,
    success: String,
    error: String,
    warning: String,
    info: String,
    destructive: String,
    diff_hunk: String,
    diff_add: String,
    diff_delete: String,
    caret: String,
    chip: String,
    chip_on_selected: String,
    chip_ink: String,
    line_number: String,
    icon_ink: String,
    link: String,
    openapi: String,
    button: String,
    syntax: RawSyntax,
    ansi: [String; 16],
    http: RawHttp,
    severity: RawSeverity,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSyntax {
    keyword: String,
    string: String,
    comment: String,
    function: String,
    r#type: String,
    number: String,
    variable: String,
    tag: String,
    property: String,
    selector: String,
    constant: String,
    css_color: String,
    css_value: String,
    heading: String,
    strong: String,
    emphasis: String,
    raw: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHttp {
    get: String,
    post: String,
    put: String,
    patch: String,
    delete: String,
    head: String,
    options: String,
    other: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSeverity {
    info_fill: String,
    warning_fill: String,
    error_fill: String,
    critical_fill: String,
    info: String,
    warning: String,
    error: String,
    critical: String,
    on: String,
}

thread_local! {
    static ACTIVE: RefCell<Scheme> = RefCell::new(resolve("dark"));
}

fn schemes() -> &'static [Scheme] {
    static SCHEMES: OnceLock<Vec<Scheme>> = OnceLock::new();
    SCHEMES.get_or_init(|| {
        parse_schemes(include_str!("../assets/color-schemes.json")).unwrap_or_default()
    })
}

fn resolve(id: &str) -> Scheme {
    let loaded = schemes();
    loaded
        .iter()
        .find(|scheme| scheme.id == id)
        .or_else(|| loaded.iter().find(|scheme| scheme.id == "dark"))
        .or_else(|| loaded.first())
        .cloned()
        .unwrap_or_else(fallback_scheme)
}

fn read<T>(reader: impl FnOnce(&Scheme) -> T) -> T {
    ACTIVE.with(|slot| reader(&slot.borrow()))
}

/// Selects a built-in scheme for this thread. Unknown ids use New UI Dark.
pub fn install(id: &str) {
    ACTIVE.with(|slot| *slot.borrow_mut() = resolve(id));
}

/// Label of a theme id. Unknown ids use the standard scheme name.
#[must_use]
pub fn display_name(id: &str) -> String {
    let loaded = read(|scheme| (scheme.id.clone(), scheme.name.clone()));
    if loaded.0 == id {
        return loaded.1;
    }
    CHOICES
        .iter()
        .find(|(choice, _)| *choice == id)
        .map_or_else(|| "New UI Dark".to_owned(), |(_, name)| (*name).to_owned())
}

/// Active scheme id.
#[must_use]
pub fn active_id() -> String {
    read(|scheme| scheme.id.clone())
}

macro_rules! color {
    ($fn:ident, $field:ident, $doc:literal) => {
        #[doc = $doc]
        #[must_use]
        pub fn $fn() -> Color32 {
            read(|scheme| scheme.$field)
        }
    };
}

color!(
    chrome,
    chrome,
    "Outer toolbar, activity rail and the gutters between tool windows."
);
color!(panel, panel, "Tool-window and sidebar background.");
color!(
    background,
    background,
    "Editor, diff context and terminal background."
);
color!(
    header,
    header,
    "Section headers and toolbars inside a tool window."
);
color!(surface, surface, "Raised cards, popups and list wells.");
color!(input, input, "Text field background.");
color!(border, border, "Dividers and control outlines.");
color!(
    border_strong,
    border_strong,
    "Hovered or focused control outline."
);
color!(hover, hover, "Hover fill for rows and buttons.");
color!(
    accent,
    selected,
    "Selection fill for lists, text and pressed controls."
);
color!(primary, primary, "Primary action fill.");
color!(
    primary_text,
    primary_text,
    "Text and icons on a primary or destructive fill."
);
color!(on_selected, on_selected, "Text on top of a selection fill.");
color!(text, text, "Primary text.");
color!(
    text_strong,
    text_strong,
    "Emphasized text, such as the active tab title."
);
color!(muted, muted, "Secondary text.");
color!(
    disabled,
    disabled,
    "Custom-painted controls that cannot be used."
);
color!(success, success, "Success and added content.");
color!(error, error, "Errors and deleted content.");
color!(warning, warning, "Warnings and modified content.");
color!(info, info, "Navigation and renamed content.");
color!(destructive, destructive, "Destructive button background.");
color!(diff_hunk, diff_hunk, "Diff hunk header background.");
color!(diff_add, diff_add, "Diff addition background.");
color!(diff_delete, diff_delete, "Diff deletion background.");
color!(caret, caret, "Terminal caret overlay.");
color!(chip, chip, "Tag and badge fill.");
color!(
    chip_on_selected,
    chip_on_selected,
    "Tag fill drawn on a selected card."
);
color!(chip_ink, chip_ink, "Tag text drawn on `chip_on_selected`.");
color!(line_number, line_number, "Gutter line numbers.");
color!(
    icon_ink,
    icon_ink,
    "Glyph drawn inside a saturated icon circle."
);
color!(link, link, "Links and the matched completion prefix.");
color!(openapi, openapi, "`OpenAPI` file mark.");
color!(button, button, "Resting button fill.");

/// Copies `color` with a straight alpha.
#[must_use]
pub fn with_alpha(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

/// Label that stays readable on [`primary`] and [`destructive`] fills.
#[must_use]
pub fn on_primary(label: impl Into<String>) -> egui::RichText {
    egui::RichText::new(label.into()).color(primary_text())
}

/// Base terminal palette. Extended ANSI colors stay protocol-defined.
#[must_use]
pub fn ansi() -> [(u8, u8, u8); 16] {
    read(|scheme| {
        let mut palette = [(0, 0, 0); 16];
        for (slot, color) in palette.iter_mut().zip(scheme.ansi) {
            *slot = (color.r(), color.g(), color.b());
        }
        palette
    })
}

/// Fill behind a problem card.
#[must_use]
pub fn severity_fill(tone: SeverityTone) -> Color32 {
    read(|scheme| match tone {
        SeverityTone::Info => scheme.severity.info_fill,
        SeverityTone::Warning => scheme.severity.warning_fill,
        SeverityTone::Error => scheme.severity.error_fill,
        SeverityTone::Critical => scheme.severity.critical_fill,
    })
}

/// Icon color for a problem tone.
#[must_use]
pub fn severity_icon(tone: SeverityTone) -> Color32 {
    read(|scheme| match tone {
        SeverityTone::Info => scheme.severity.info,
        SeverityTone::Warning => scheme.severity.warning,
        SeverityTone::Error => scheme.severity.error,
        SeverityTone::Critical => scheme.severity.critical,
    })
}

/// Text on a problem card.
#[must_use]
pub fn severity_on() -> Color32 {
    read(|scheme| scheme.severity.on)
}

/// HTTP method color. Unknown methods use the scheme's neutral method color.
#[must_use]
pub fn http_method(method: &str) -> Color32 {
    read(|scheme| match method {
        "GET" => scheme.http.get,
        "POST" => scheme.http.post,
        "PUT" => scheme.http.put,
        "PATCH" => scheme.http.patch,
        "DELETE" => scheme.http.delete,
        "HEAD" => scheme.http.head,
        "OPTIONS" => scheme.http.options,
        _ => scheme.http.other,
    })
}

/// Maps Tree-sitter captures onto the active scheme's syntax palette.
#[must_use]
pub fn syntax_color(token: &str) -> Color32 {
    read(|scheme| syntax_token(&scheme.syntax, scheme.text, token))
}

/// Proportional bold face used by the Markdown preview.
#[must_use]
pub fn bold_family() -> egui::FontFamily {
    egui::FontFamily::Name("Noto Sans Bold".into())
}

/// Editor text face for one syntax token: color, italics, strike and code background.
#[must_use]
pub fn syntax_format(
    token: &str,
    color: Color32,
    font_size: f32,
    proportional: bool,
) -> TextFormat {
    let emphasis = matches!(
        token,
        "comment" | "markup.emphasis" | "markup.strong_emphasis"
    );
    let raw = token == "markup.raw";
    TextFormat {
        font_id: if proportional {
            FontId::proportional(font_size)
        } else {
            FontId::monospace(font_size)
        },
        color,
        italics: emphasis,
        background: if raw { chip() } else { Color32::TRANSPARENT },
        strikethrough: if token == "markup.strike" {
            Stroke::new(1.0_f32, color)
        } else {
            Stroke::NONE
        },
        ..Default::default()
    }
}

fn syntax_token(syntax: &Syntax, fallback: Color32, token: &str) -> Color32 {
    match token {
        "yaml.key" => return syntax.keyword,
        "yaml.anchor" | "css.selector" | "css.function" => return syntax.selector,
        "css.property" => return syntax.property,
        "css.color" => return syntax.css_color,
        "css.value" => return syntax.css_value,
        "markup.heading" => return syntax.heading,
        "markup.strong" | "markup.strong_emphasis" => return syntax.strong,
        "markup.emphasis" => return syntax.emphasis,
        "markup.raw" => return syntax.raw,
        "markup.link" => return syntax.function,
        "markup.strike" | "markup.quote" => return syntax.comment,
        "markup.list" | "markup.rule" => return syntax.constant,
        _ => {}
    }
    match token.split('.').next().unwrap_or(token) {
        "keyword" => syntax.keyword,
        "tag" => syntax.tag,
        "string" => syntax.string,
        "comment" => syntax.comment,
        "function" | "constructor" => syntax.function,
        "type" | "module" => syntax.r#type,
        "number" => syntax.number,
        "constant" => syntax.constant,
        "variable" | "property" => syntax.variable,
        _ => fallback,
    }
}

/// Installs New UI Dark and the shared desktop spacing.
pub fn apply(ctx: &Context) {
    apply_id(ctx, "dark");
}

/// Installs `id` and applies its colors to `ctx`. Unknown ids use New UI Dark.
pub fn apply_id(ctx: &Context, id: &str) {
    install(id);
    ensure_fonts(ctx);
    ctx.set_visuals(visuals());
    let button_padding = egui::vec2(9.0, f32::from(CONTROL_PAD_Y));
    ctx.style_mut(|style| {
        style.interaction.resize_grab_radius_side = SPLITTER;
        style.spacing.button_padding = button_padding;
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    });
    // Fonts are not readable until the first pass. The hook runs after that pass
    // starts, so `interact_size` matches a button before any widget is built.
    // `horizontal` and `ComboBox` size their row from that value.
    install_control_height(ctx);
}

fn install_control_height(ctx: &Context) {
    let id = egui::Id::new("nozhik-control-height");
    if ctx.data(|data| data.get_temp::<bool>(id).unwrap_or(false)) {
        return;
    }
    ctx.on_begin_pass(
        "nozhik-control-height",
        Arc::new(|ctx| {
            let control_height =
                f32::mul_add(f32::from(CONTROL_PAD_Y), 2.0, button_row_height(ctx));
            ctx.style_mut(|style| style.spacing.interact_size.y = control_height);
        }),
    );
    ctx.data_mut(|data| data.insert_temp(id, true));
}

/// Vertical button padding, in logical pixels. Single-line fields use the same value.
pub(crate) const CONTROL_PAD_Y: i8 = 6;

fn button_row_height(ctx: &Context) -> f32 {
    let font = egui::TextStyle::Button.resolve(&ctx.style());
    ctx.fonts(|fonts| fonts.row_height(&font))
}

fn visuals() -> Visuals {
    let (light, disabled_alpha) = read(|scheme| (scheme.light, scheme.disabled_alpha));
    let mut visuals = if light {
        Visuals::light()
    } else {
        Visuals::dark()
    };
    visuals.dark_mode = !light;
    visuals.override_text_color = Some(text());
    visuals.weak_text_color = Some(muted());
    visuals.panel_fill = chrome();
    visuals.window_fill = panel();
    visuals.extreme_bg_color = input();
    visuals.faint_bg_color = panel();
    visuals.code_bg_color = surface();
    visuals.hyperlink_color = link();
    visuals.warn_fg_color = warning();
    visuals.error_fg_color = error();
    visuals.selection.bg_fill = accent();
    visuals.selection.stroke = Stroke::new(1.0_f32, on_selected());
    visuals.disabled_alpha = disabled_alpha;
    visuals.text_cursor.stroke.color = text();
    visuals.menu_corner_radius = egui::CornerRadius::same(RADIUS);
    visuals.window_corner_radius = egui::CornerRadius::same(RADIUS);
    let radius = egui::CornerRadius::same(RADIUS);
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = panel();
    widgets.noninteractive.weak_bg_fill = panel();
    widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, border());
    widgets.noninteractive.fg_stroke.color = disabled();
    widgets.noninteractive.corner_radius = radius;
    widgets.inactive.bg_fill = button();
    widgets.inactive.weak_bg_fill = button();
    widgets.inactive.bg_stroke = Stroke::new(1.0_f32, border());
    widgets.inactive.fg_stroke.color = text();
    widgets.inactive.corner_radius = radius;
    widgets.hovered.bg_fill = hover();
    widgets.hovered.weak_bg_fill = hover();
    widgets.hovered.bg_stroke = Stroke::new(1.0_f32, border_strong());
    widgets.hovered.fg_stroke.color = text();
    widgets.hovered.corner_radius = radius;
    widgets.active.bg_fill = accent();
    widgets.active.weak_bg_fill = accent();
    widgets.active.bg_stroke = Stroke::new(1.0_f32, primary());
    widgets.active.fg_stroke.color = on_selected();
    widgets.active.corner_radius = radius;
    widgets.open.bg_fill = surface();
    widgets.open.weak_bg_fill = hover();
    widgets.open.bg_stroke = Stroke::new(1.0_f32, border());
    widgets.open.fg_stroke.color = text();
    widgets.open.corner_radius = radius;
    visuals
}

/// Shared dock surface. The outer margin keeps rounded corners visible.
pub fn dock_frame(fill: Color32) -> egui::Frame {
    egui::Frame::new()
        .fill(fill)
        .corner_radius(RADIUS)
        .inner_margin(8)
        .outer_margin(egui::Margin::same(2))
}

/// Compact rail surrounding the work area.
pub fn rail_frame() -> egui::Frame {
    egui::Frame::new().fill(chrome()).inner_margin(4)
}

#[derive(Clone, Copy)]
struct FontsReady;

fn ensure_fonts(ctx: &Context) {
    let id = egui::Id::new("nozhik-theme-fonts");
    if ctx.data(|data| data.get_temp::<FontsReady>(id).is_some()) {
        return;
    }
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "JetBrains Mono".into(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/JetBrainsMono-Regular.ttf"
        ))),
    );
    fonts.families.insert(
        egui::FontFamily::Name("JetBrains Mono".into()),
        vec!["JetBrains Mono".into()],
    );
    fonts.font_data.insert(
        "Noto Sans Symbols 2".into(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/NotoSansSymbols2-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        "Noto Emoji".into(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/NotoEmoji.ttf"
        ))),
    );
    let mut terminal = vec![
        "JetBrains Mono".into(),
        "Noto Sans Symbols 2".into(),
        "Noto Emoji".into(),
    ];
    terminal.extend(
        fonts
            .families
            .get(&egui::FontFamily::Monospace)
            .into_iter()
            .flatten()
            .cloned(),
    );
    fonts
        .families
        .insert(egui::FontFamily::Name("Terminal".into()), terminal);
    fonts.font_data.insert(
        "Noto Sans Bold".into(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/NotoSans-Bold.ttf"
        ))),
    );
    // Ubuntu-Light has no black circle or arrow, so proportional labels such as
    // "● Connected" and "Mouse → agent" were drawing missing-glyph squares.
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        let stack = fonts.families.entry(family).or_default();
        for name in ["Noto Sans Symbols 2", "JetBrains Mono"] {
            if !stack.iter().any(|item| item == name) {
                stack.push(name.to_owned());
            }
        }
    }
    let mut bold = vec!["Noto Sans Bold".into()];
    if let Some(stack) = fonts.families.get(&egui::FontFamily::Proportional) {
        for name in stack {
            if name != "Noto Sans Bold" {
                bold.push(name.clone());
            }
        }
    }
    fonts
        .families
        .insert(egui::FontFamily::Name("Noto Sans Bold".into()), bold);
    ctx.set_fonts(fonts);
    ctx.data_mut(|data| data.insert_temp(id, FontsReady));
}

fn parse_schemes(source: &str) -> Result<Vec<Scheme>, String> {
    let file: SchemeFile = serde_json::from_str(source).map_err(|error| error.to_string())?;
    if file.schemes.is_empty() {
        return Err("color scheme file has no schemes".into());
    }
    let mut schemes = Vec::with_capacity(file.schemes.len());
    for raw in file.schemes {
        let scheme = convert_scheme(raw)?;
        if schemes
            .iter()
            .any(|existing: &Scheme| existing.id == scheme.id)
        {
            return Err(format!("duplicate color scheme {}", scheme.id));
        }
        schemes.push(scheme);
    }
    Ok(schemes)
}

fn convert_syntax(raw: &RawSyntax) -> Result<Syntax, String> {
    Ok(Syntax {
        keyword: hex_color(&raw.keyword)?,
        string: hex_color(&raw.string)?,
        comment: hex_color(&raw.comment)?,
        function: hex_color(&raw.function)?,
        r#type: hex_color(&raw.r#type)?,
        number: hex_color(&raw.number)?,
        variable: hex_color(&raw.variable)?,
        tag: hex_color(&raw.tag)?,
        property: hex_color(&raw.property)?,
        selector: hex_color(&raw.selector)?,
        constant: hex_color(&raw.constant)?,
        css_color: hex_color(&raw.css_color)?,
        css_value: hex_color(&raw.css_value)?,
        heading: hex_color(&raw.heading)?,
        strong: hex_color(&raw.strong)?,
        emphasis: hex_color(&raw.emphasis)?,
        raw: hex_color(&raw.raw)?,
    })
}

fn convert_scheme(raw: RawScheme) -> Result<Scheme, String> {
    if raw.id.trim().is_empty() || raw.name.trim().is_empty() {
        return Err("color scheme id and name must be nonempty".into());
    }
    let light = match raw.appearance.as_str() {
        "light" => true,
        "dark" => false,
        _ => return Err(format!("unsupported appearance for {}", raw.id)),
    };
    if !raw.disabled_alpha.is_finite() || !(0.0..=1.0).contains(&raw.disabled_alpha) {
        return Err(format!(
            "disabled_alpha for {} must be between 0 and 1",
            raw.id
        ));
    }
    let ansi = raw
        .ansi
        .iter()
        .map(|color| hex_color(color))
        .collect::<Result<Vec<_>, _>>()?
        .try_into()
        .map_err(|_| format!("ANSI palette for {} must contain 16 colors", raw.id))?;
    Ok(Scheme {
        id: raw.id,
        name: raw.name,
        light,
        disabled_alpha: raw.disabled_alpha,
        chrome: hex_color(&raw.chrome)?,
        panel: hex_color(&raw.panel)?,
        background: hex_color(&raw.background)?,
        header: hex_color(&raw.header)?,
        surface: hex_color(&raw.surface)?,
        input: hex_color(&raw.input)?,
        border: hex_color(&raw.border)?,
        border_strong: hex_color(&raw.border_strong)?,
        hover: hex_color(&raw.hover)?,
        selected: hex_color(&raw.selected)?,
        primary: hex_color(&raw.primary)?,
        primary_text: hex_color(&raw.primary_text)?,
        on_selected: hex_color(&raw.on_selected)?,
        text: hex_color(&raw.text)?,
        text_strong: hex_color(&raw.text_strong)?,
        muted: hex_color(&raw.muted)?,
        disabled: hex_color(&raw.disabled)?,
        success: hex_color(&raw.success)?,
        error: hex_color(&raw.error)?,
        warning: hex_color(&raw.warning)?,
        info: hex_color(&raw.info)?,
        destructive: hex_color(&raw.destructive)?,
        diff_hunk: hex_color(&raw.diff_hunk)?,
        diff_add: hex_color(&raw.diff_add)?,
        diff_delete: hex_color(&raw.diff_delete)?,
        caret: hex_color(&raw.caret)?,
        chip: hex_color(&raw.chip)?,
        chip_on_selected: hex_color(&raw.chip_on_selected)?,
        chip_ink: hex_color(&raw.chip_ink)?,
        line_number: hex_color(&raw.line_number)?,
        icon_ink: hex_color(&raw.icon_ink)?,
        link: hex_color(&raw.link)?,
        openapi: hex_color(&raw.openapi)?,
        button: hex_color(&raw.button)?,
        syntax: convert_syntax(&raw.syntax)?,
        ansi,
        http: Http {
            get: hex_color(&raw.http.get)?,
            post: hex_color(&raw.http.post)?,
            put: hex_color(&raw.http.put)?,
            patch: hex_color(&raw.http.patch)?,
            delete: hex_color(&raw.http.delete)?,
            head: hex_color(&raw.http.head)?,
            options: hex_color(&raw.http.options)?,
            other: hex_color(&raw.http.other)?,
        },
        severity: Severity {
            info_fill: hex_color(&raw.severity.info_fill)?,
            warning_fill: hex_color(&raw.severity.warning_fill)?,
            error_fill: hex_color(&raw.severity.error_fill)?,
            critical_fill: hex_color(&raw.severity.critical_fill)?,
            info: hex_color(&raw.severity.info)?,
            warning: hex_color(&raw.severity.warning)?,
            error: hex_color(&raw.severity.error)?,
            critical: hex_color(&raw.severity.critical)?,
            on: hex_color(&raw.severity.on)?,
        },
    })
}

fn hex_color(raw: &str) -> Result<Color32, String> {
    let hex = raw.strip_prefix('#').unwrap_or(raw);
    if !matches!(hex.len(), 6 | 8) || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("invalid color {raw}"));
    }
    let byte = |start: usize| {
        u8::from_str_radix(&hex[start..start + 2], 16).map_err(|_| format!("invalid color {raw}"))
    };
    let red = byte(0)?;
    let green = byte(2)?;
    let blue = byte(4)?;
    if hex.len() == 6 {
        Ok(Color32::from_rgb(red, green, blue))
    } else {
        Ok(Color32::from_rgba_unmultiplied(red, green, blue, byte(6)?))
    }
}

fn fallback_scheme() -> Scheme {
    let color = Color32::from_rgb(30, 31, 32);
    let syntax = Syntax {
        keyword: color,
        string: color,
        comment: color,
        function: color,
        r#type: color,
        number: color,
        variable: color,
        tag: color,
        property: color,
        selector: color,
        constant: color,
        css_color: color,
        css_value: color,
        heading: color,
        strong: color,
        emphasis: color,
        raw: color,
    };
    Scheme {
        id: "fallback".into(),
        name: "Fallback".into(),
        light: false,
        disabled_alpha: 0.5,
        chrome: color,
        panel: color,
        background: color,
        header: color,
        surface: color,
        input: color,
        border: color,
        border_strong: color,
        hover: color,
        selected: color,
        primary: color,
        primary_text: Color32::WHITE,
        on_selected: Color32::WHITE,
        text: Color32::WHITE,
        text_strong: Color32::WHITE,
        muted: color,
        disabled: color,
        success: color,
        error: color,
        warning: color,
        info: color,
        destructive: color,
        diff_hunk: color,
        diff_add: color,
        diff_delete: color,
        caret: color,
        chip: color,
        chip_on_selected: color,
        chip_ink: color,
        line_number: color,
        icon_ink: color,
        link: color,
        openapi: color,
        button: color,
        syntax,
        ansi: [color; 16],
        http: Http {
            get: color,
            post: color,
            put: color,
            patch: color,
            delete: color,
            head: color,
            options: color,
            other: color,
        },
        severity: Severity {
            info_fill: color,
            warning_fill: color,
            error_fill: color,
            critical_fill: color,
            info: color,
            warning: color,
            error: color,
            critical: color,
            on: Color32::WHITE,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CHOICES, Color32, apply_id, background, border, chip, chrome, hover, http_method, install,
        muted, panel, parse_schemes, resolve, success, syntax_color, syntax_format, text,
    };

    #[test]
    fn bundled_schemes_match_settings_choices() -> Result<(), String> {
        let schemes = parse_schemes(include_str!("../assets/color-schemes.json"))?;
        let ids: Vec<_> = schemes.iter().map(|scheme| scheme.id.as_str()).collect();
        let choice_ids: Vec<_> = CHOICES.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, choice_ids);
        for (scheme, (_, name)) in schemes.iter().zip(CHOICES) {
            assert_eq!(scheme.name, *name);
        }
        Ok(())
    }

    #[test]
    fn standard_scheme_keeps_the_previous_new_ui_dark_colors() {
        install("dark");
        assert_eq!(background(), Color32::from_rgb(30, 31, 32));
        assert_eq!(panel(), Color32::from_rgb(24, 25, 26));
        assert_eq!(border(), Color32::from_rgb(57, 59, 64));
        assert_eq!(super::accent(), Color32::from_rgb(46, 67, 110));
        assert_eq!(hover(), Color32::from_rgb(53, 55, 60));
        assert_eq!(text(), Color32::from_rgb(188, 190, 196));
        assert_eq!(muted(), Color32::from_rgb(122, 126, 133));
        assert_eq!(chrome(), Color32::from_rgb(38, 40, 42));
        assert_eq!(success(), Color32::from_rgb(90, 200, 120));
        assert_eq!(syntax_color("keyword"), Color32::from_rgb(207, 142, 109));
        assert_eq!(syntax_color("string"), Color32::from_rgb(106, 171, 115));
        assert_eq!(syntax_color("comment"), Color32::from_rgb(122, 126, 133));
        assert_eq!(
            syntax_color("markup.heading"),
            Color32::from_rgb(199, 125, 187)
        );
        assert_eq!(
            syntax_color("markup.strong"),
            Color32::from_rgb(229, 192, 123)
        );
        let emphasis = syntax_format(
            "markup.emphasis",
            syntax_color("markup.emphasis"),
            14.0,
            false,
        );
        assert!(emphasis.italics);
        assert_eq!(emphasis.background, Color32::TRANSPARENT);
        let raw = syntax_format("markup.raw", syntax_color("markup.raw"), 14.0, false);
        assert_eq!(raw.background, chip());
        assert!(
            syntax_format("markup.strike", text(), 14.0, true)
                .strikethrough
                .width
                > 0.0
        );
        assert!(syntax_format("comment", text(), 14.0, false).italics);
        assert_eq!(super::ansi()[0], (30, 31, 34));
        assert_eq!(super::ansi()[7], (188, 190, 196));
        assert_eq!(super::ansi()[15], (255, 255, 255));
        assert_eq!(http_method("GET"), Color32::from_rgb(97, 175, 254));
        assert_eq!(http_method("TRACE"), Color32::from_rgb(120, 120, 120));
    }

    #[test]
    fn light_and_catppuccin_keep_text_readable_on_their_background() {
        for id in ["dark", "light", "catppuccin"] {
            let scheme = resolve(id);
            let text_luma = luma(scheme.text);
            let background_luma = luma(scheme.background);
            assert!(
                (text_luma - background_luma).abs() > 80.0,
                "{id} text and background are too close"
            );
            assert_eq!(scheme.light, id == "light");
            if scheme.light {
                assert!(text_luma + 40.0 < background_luma);
            } else {
                assert!(background_luma + 40.0 < text_luma);
            }
            assert!((luma(scheme.primary) - luma(scheme.primary_text)).abs() > 80.0);
        }
        assert_eq!(resolve("light").background, Color32::WHITE);
        assert_eq!(
            resolve("catppuccin").background,
            Color32::from_rgb(30, 30, 46)
        );
        assert_eq!(resolve("missing").id, "dark");
    }

    #[test]
    fn apply_id_switches_the_egui_palette() {
        let ctx = egui::Context::default();
        apply_id(&ctx, "light");
        assert!(!ctx.style().visuals.dark_mode);
        assert_eq!(ctx.style().visuals.override_text_color, Some(text()));
        assert_eq!(text(), Color32::from_rgb(25, 26, 28));
        apply_id(&ctx, "dark");
        assert!(ctx.style().visuals.dark_mode);
        assert_eq!(background(), Color32::from_rgb(30, 31, 32));
    }

    #[test]
    fn rejects_unknown_fields_and_bad_colors() {
        assert!(parse_schemes(r#"{"schemes":[],"extra":1}"#).is_err());
        let broken = include_str!("../assets/color-schemes.json").replace("#1E1F20", "nope");
        assert!(parse_schemes(&broken).is_err());
    }

    fn luma(color: Color32) -> f32 {
        let [red, green, blue, _] = color.to_array();
        0.0722f32.mul_add(
            f32::from(blue),
            0.7152f32.mul_add(f32::from(green), 0.2126 * f32::from(red)),
        )
    }

    #[test]
    fn terminal_font_covers_braille_powerline_blocks_unicode_and_emoji() {
        let ctx = egui::Context::default();
        super::apply(&ctx);
        let _output = ctx.run(egui::RawInput::default(), |ctx| {
            let font = egui::FontId::new(14.0, egui::FontFamily::Name("Terminal".into()));
            ctx.fonts(|fonts| {
                for character in (0x2800..=0x28ff)
                    .filter_map(char::from_u32)
                    .chain("\u{e0a0}─│┌┐└┘█▓▒░Ж→✓🙂".chars())
                {
                    assert!(
                        fonts.has_glyph(&font, character),
                        "missing terminal glyph U+{:04X}",
                        u32::from(character)
                    );
                }
            });
        });
    }

    #[test]
    fn proportional_ui_font_covers_status_and_arrow_symbols() {
        let ctx = egui::Context::default();
        super::apply(&ctx);
        let _output = ctx.run(egui::RawInput::default(), |ctx| {
            let font = egui::FontId::proportional(14.0);
            ctx.fonts(|fonts| {
                for character in ['●', '→', '▶', '▼', '·', '✓'] {
                    assert!(
                        fonts.has_glyph(&font, character),
                        "missing proportional glyph U+{:04X} {character}",
                        u32::from(character)
                    );
                }
            });
        });
    }
}
