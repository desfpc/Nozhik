//! Session problems for one open project, and the toasts that announce them.
//!
//! Problems live only in this project session. They are not written to disk.
//! A repeated source, title and detail is recorded once. The Problems tool
//! groups them by category; only Project exists today. A new problem also
//! opens a bottom-right toast that fades after a few seconds.

use crate::theme;
use egui::{
    Align2, Color32, Context, CornerRadius, FontId, Frame, Margin, RichText, Stroke, Ui, Vec2,
};
use std::collections::HashSet;
use std::time::Duration;

/// How long a toast stays fully visible before it fades, in seconds.
const VISIBLE_SECS: f32 = 3.0;
/// Fade-out after the hold, in seconds.
const FADE_SECS: f32 = 0.5;
/// Fade used when the user closes a toast.
const CLOSE_FADE_SECS: f32 = 0.2;
/// Toasts drawn at once. The rest wait in a “N more” row and do not age yet.
const MAX_VISIBLE_TOASTS: usize = 3;
/// Distinct problems kept for one project session.
const MAX_PROBLEMS: usize = 100;
/// Largest frame delta applied to toast timers, in seconds.
const MAX_FRAME_SECS: f32 = 0.25;

/// Where a problem is grouped in the Problems tool window.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ProblemCategory {
    /// Settings and other project-wide failures, such as Git, database and SSH.
    Project,
}

impl ProblemCategory {
    /// Categories drawn as expandable tabs, in display order.
    pub(crate) const ALL: [Self; 1] = [Self::Project];

    /// Short label painted on the tab.
    #[must_use]
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Project => "Project",
        }
    }
}

/// How strongly a problem should interrupt the user.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Severity {
    /// Neutral information. Dark card, blue icon.
    #[allow(
        dead_code,
        reason = "The toast scale includes information; Project has no informational source yet"
    )]
    Info,
    /// Recoverable warning. Warm card, orange icon.
    Warning,
    /// Failed operation. Red-tinted card.
    Error,
    /// Security block or a stopped worker. Red card.
    Critical,
}

impl Severity {
    const fn tone(self) -> theme::SeverityTone {
        match self {
            Self::Info => theme::SeverityTone::Info,
            Self::Warning => theme::SeverityTone::Warning,
            Self::Error => theme::SeverityTone::Error,
            Self::Critical => theme::SeverityTone::Critical,
        }
    }

    /// Card fill from the active color scheme.
    #[must_use]
    pub(crate) fn fill(self) -> Color32 {
        theme::severity_fill(self.tone())
    }

    /// Circle behind the severity glyph.
    #[must_use]
    pub(crate) fn icon(self) -> Color32 {
        theme::severity_icon(self.tone())
    }

    /// Glyph inside the severity circle.
    #[must_use]
    pub(crate) const fn glyph(self) -> &'static str {
        match self {
            Self::Info => "i",
            Self::Warning | Self::Error | Self::Critical => "!",
        }
    }

    /// Badge color for a category that contains this severity.
    #[must_use]
    pub(crate) fn badge(self) -> Color32 {
        self.icon()
    }
}

/// One failure observed during the project session.
#[derive(Clone, Debug)]
pub struct Incident {
    /// Stable identity. The same source, title and detail is stored once.
    pub source: String,
    /// Group tab. Further categories can be added without changing the log.
    pub category: ProblemCategory,
    /// Visual weight of the toast and the list row.
    pub severity: Severity,
    /// Short heading.
    pub title: String,
    /// Full message. Empty when the title is enough.
    pub detail: String,
}

/// Owned incident for the Project tab.
#[must_use]
pub fn incident(
    source: impl Into<String>,
    severity: Severity,
    title: impl Into<String>,
    detail: impl Into<String>,
) -> Incident {
    Incident {
        source: source.into(),
        category: ProblemCategory::Project,
        severity,
        title: title.into(),
        detail: detail.into(),
    }
}

/// Critical when a worker or id space has stopped; otherwise an error.
#[must_use]
pub fn severity_of(message: &str) -> Severity {
    if message.contains("stopped") || message.contains("exhausted") {
        Severity::Critical
    } else {
        Severity::Error
    }
}

#[derive(Clone, Debug)]
struct Problem {
    id: u64,
    key: String,
    category: ProblemCategory,
    severity: Severity,
    title: String,
    detail: String,
}

#[derive(Clone, Debug)]
struct Toast {
    problem_id: u64,
    /// Remaining full-opacity seconds. `None` until the toast is actually shown.
    hold: Option<f32>,
    /// `0` is opaque. `1` has finished fading and is removed.
    fade: f32,
    dismissed: bool,
}

/// Problems and toasts for a single project session.
#[derive(Debug)]
pub struct ProblemLog {
    next_id: u64,
    problems: Vec<Problem>,
    seen: HashSet<String>,
    expanded: HashSet<ProblemCategory>,
    toasts: Vec<Toast>,
    last_time: Option<f64>,
}

impl Default for ProblemLog {
    fn default() -> Self {
        let mut expanded = HashSet::new();
        expanded.insert(ProblemCategory::Project);
        Self {
            next_id: 1,
            problems: Vec::new(),
            seen: HashSet::new(),
            expanded,
            toasts: Vec::new(),
            last_time: None,
        }
    }
}

impl ProblemLog {
    /// Records a new incident. Returns whether a toast was opened.
    pub(crate) fn report(&mut self, incident: Incident) -> bool {
        let Incident {
            source,
            category,
            severity,
            title,
            detail,
        } = incident;
        let title = title.trim().to_owned();
        let detail = detail.trim().to_owned();
        if title.is_empty() && detail.is_empty() {
            return false;
        }
        let key = format!(
            "{}\u{0}{}\u{0}{}\u{0}{}",
            category.label(),
            source,
            title,
            detail
        );
        if !self.seen.insert(key.clone()) {
            return false;
        }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.problems.insert(
            0,
            Problem {
                id,
                key,
                category,
                severity,
                title,
                detail,
            },
        );
        self.toasts.push(Toast {
            problem_id: id,
            hold: None,
            fade: 0.0,
            dismissed: false,
        });
        while self.problems.len() > MAX_PROBLEMS {
            if let Some(old) = self.problems.pop() {
                self.seen.remove(&old.key);
                self.toasts.retain(|toast| toast.problem_id != old.id);
            }
        }
        true
    }

    /// Records each distinct incident. Empty details are ignored.
    pub(crate) fn observe(&mut self, incidents: impl IntoIterator<Item = Incident>) {
        for incident in incidents {
            self.report(incident);
        }
    }

    /// Problems currently listed under `category`.
    #[must_use]
    pub(crate) fn count(&self, category: ProblemCategory) -> usize {
        self.problems
            .iter()
            .filter(|problem| problem.category == category)
            .count()
    }

    /// Highest severity in the category, if it has any problem.
    #[must_use]
    pub(crate) fn worst(&self, category: ProblemCategory) -> Option<Severity> {
        self.problems
            .iter()
            .filter(|problem| problem.category == category)
            .map(|problem| problem.severity)
            .max()
    }

    /// Whether this status-bar string repeats a problem and should stay hidden.
    #[must_use]
    pub(crate) fn hides_status(&self, status: &str) -> bool {
        let status = status.trim();
        if status.is_empty() {
            return false;
        }
        self.problems.iter().any(|problem| {
            let detail = problem.detail.trim();
            !detail.is_empty()
                && (status == detail || (detail.len() >= 8 && status.ends_with(detail)))
        })
    }

    /// Expandable category tabs and their notification rows.
    pub(crate) fn show_panel(&mut self, ui: &mut Ui) {
        ui.heading("Problems");
        ui.separator();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for category in ProblemCategory::ALL {
                    self.show_category(ui, category);
                }
            });
    }

    /// Bottom-right toasts. Hover pauses the timer. The close control dismisses one toast.
    pub(crate) fn show_toasts(&mut self, ctx: &Context) {
        self.toasts.retain(|toast| toast.fade < 1.0);
        let now = ctx.input(|input| input.time);
        let dt = self.advance_clock(now);
        let active = self.active_ids();
        if active.is_empty() {
            return;
        }
        // Newest toasts stay on screen. Older ones wait and do not age yet.
        let hidden = active.len().saturating_sub(MAX_VISIBLE_TOASTS);
        let visible: Vec<u64> = active.into_iter().skip(hidden).collect();
        let mut dismissed = Vec::new();
        let mut hovered = Vec::new();
        egui::Area::new(egui::Id::new("problem_toasts"))
            .anchor(Align2::RIGHT_BOTTOM, Vec2::new(-16.0, -40.0))
            .order(egui::Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.set_min_width(320.0);
                for id in &visible {
                    let Some(problem) = self
                        .problems
                        .iter()
                        .find(|problem| problem.id == *id)
                        .cloned()
                    else {
                        continue;
                    };
                    let fade = self
                        .toasts
                        .iter()
                        .find(|toast| toast.problem_id == *id)
                        .map_or(0.0, |toast| toast.fade);
                    let (is_hovered, close) = paint_toast(ui, &problem, fade);
                    if is_hovered {
                        hovered.push(*id);
                    }
                    if close {
                        dismissed.push(*id);
                    }
                    ui.add_space(8.0);
                }
                if hidden > 0 {
                    more_bar(ui, hidden);
                }
            });
        for id in &visible {
            self.tick_toast(*id, dt, hovered.contains(id), dismissed.contains(id));
        }
        self.request_toast_frames(ctx);
    }

    fn show_category(&mut self, ui: &mut Ui, category: ProblemCategory) {
        let count = self.count(category);
        let expanded = self.expanded.contains(&category);
        let badge = self
            .worst(category)
            .map_or_else(theme::muted, Severity::badge);
        let mut toggle = false;
        ui.horizontal(|ui| {
            let arrow = if expanded { "▼" } else { "▶" };
            toggle |= ui
                .selectable_label(false, format!("{arrow} {}", category.label()))
                .clicked();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                toggle |= badge_label(ui, count, badge).clicked();
            });
        });
        if toggle {
            self.toggle(category);
        }
        if !self.expanded.contains(&category) {
            ui.add_space(4.0);
            return;
        }
        let rows: Vec<Problem> = self
            .problems
            .iter()
            .filter(|problem| problem.category == category)
            .cloned()
            .collect();
        if rows.is_empty() {
            ui.weak("No problems in this session.");
        }
        for problem in rows {
            paint_problem(ui, &problem);
            ui.add_space(6.0);
        }
    }

    fn toggle(&mut self, category: ProblemCategory) {
        if !self.expanded.remove(&category) {
            self.expanded.insert(category);
        }
    }

    fn advance_clock(&mut self, now: f64) -> f32 {
        let dt = self
            .last_time
            .map_or(0.0, |previous| frame_delta(previous, now));
        self.last_time = Some(now);
        dt
    }

    fn active_ids(&self) -> Vec<u64> {
        self.toasts
            .iter()
            .filter(|toast| toast.fade < 1.0)
            .map(|toast| toast.problem_id)
            .collect()
    }

    fn tick_toast(&mut self, id: u64, dt: f32, hovered: bool, dismissed: bool) {
        let Some(toast) = self.toasts.iter_mut().find(|toast| toast.problem_id == id) else {
            return;
        };
        if dismissed {
            toast.dismissed = true;
        }
        if toast.hold.is_none() {
            toast.hold = Some(VISIBLE_SECS);
        }
        if toast.dismissed {
            toast.fade = (toast.fade + dt / CLOSE_FADE_SECS).min(1.0);
            return;
        }
        if hovered {
            return;
        }
        let Some(hold) = toast.hold.as_mut() else {
            return;
        };
        if *hold > 0.0 {
            *hold = (*hold - dt).max(0.0);
        } else {
            toast.fade = (toast.fade + dt / FADE_SECS).min(1.0);
        }
    }

    fn request_toast_frames(&self, ctx: &Context) {
        let mut fading = false;
        let mut next_hold: Option<f32> = None;
        for toast in &self.toasts {
            if toast.fade > 0.0 && toast.fade < 1.0 {
                fading = true;
            }
            if let Some(hold) = toast.hold
                && hold > 0.0
                && !toast.dismissed
            {
                next_hold = Some(next_hold.map_or(hold, |current| current.min(hold)));
            }
        }
        if fading {
            ctx.request_repaint_after(Duration::from_millis(16));
        } else if let Some(hold) = next_hold {
            ctx.request_repaint_after(Duration::from_secs_f32(hold.min(MAX_FRAME_SECS)));
        }
    }
}

impl crate::NozhikUi {
    /// Records one session problem and opens its toast when it is new.
    pub(crate) fn note_problem(
        &mut self,
        source: &str,
        title: &str,
        severity: Severity,
        detail: &str,
    ) {
        self.problems
            .report(incident(source, severity, title, detail));
    }

    /// Records a problem and keeps the same text on `status` for existing callers.
    /// The status bar hides it once the problem exists.
    pub(crate) fn note_status(
        &mut self,
        source: &str,
        title: &str,
        severity: Severity,
        detail: impl Into<String>,
    ) {
        let detail = detail.into();
        self.note_problem(source, title, severity, &detail);
        self.status = detail;
    }

    /// Copies current subsystem failures into the session log.
    pub(crate) fn observe_session_problems(&mut self) {
        let ai = self.editor.ai.problem().map(str::to_owned);
        let mut incidents = Vec::new();
        incidents.extend(self.vcs.incidents());
        incidents.extend(self.database.incidents());
        incidents.extend(self.remote.incidents());
        incidents.extend(self.docker.incidents());
        incidents.extend(self.deployment.incidents());
        incidents.extend(self.settings_dialog.incidents());
        incidents.extend(self.project_controls.incidents());
        if let Some(error) = &self.integration.save_error {
            incidents.push(incident("save", severity_of(error), "Save", error));
        }
        if let Some(error) = &self.integration.project_error {
            incidents.push(incident(
                "new-project",
                severity_of(error),
                "New project",
                error,
            ));
        }
        if let Some(panel) = &self.terminal {
            incidents.extend(panel.incidents());
        }
        if let Some(detail) = ai {
            incidents.push(incident(
                "ai-inline",
                Severity::Error,
                "AI completion",
                detail,
            ));
        }
        self.problems.observe(incidents);
    }
}

fn frame_delta(previous: f64, now: f64) -> f32 {
    let delta = (now - previous).clamp(0.0, f64::from(MAX_FRAME_SECS));
    // The quarter-second clamp keeps this millisecond count inside `u16`.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "delta is clamped to 250 milliseconds before the cast"
    )]
    let millis = (delta * 1000.0).round() as u16;
    f32::from(millis) / 1000.0
}

fn card_frame(fill: Color32) -> Frame {
    Frame::new()
        .fill(fill)
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(Margin::same(12))
        .stroke(Stroke::new(1.0_f32, theme::border()))
}

fn paint_severity(ui: &mut Ui, severity: Severity) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(18.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.circle_filled(rect.center(), 8.0, severity.icon());
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        severity.glyph(),
        FontId::proportional(12.0),
        theme::primary_text(),
    );
}

fn paint_problem(ui: &mut Ui, problem: &Problem) {
    card_frame(problem.severity.fill()).show(ui, |ui| {
        ui.horizontal(|ui| {
            paint_severity(ui, problem.severity);
            ui.vertical(|ui| {
                ui.add(egui::Label::new(RichText::new(&problem.title).strong()).wrap());
                if !problem.detail.is_empty() && problem.detail != problem.title {
                    ui.add(
                        egui::Label::new(RichText::new(&problem.detail).color(theme::text()))
                            .wrap(),
                    );
                }
            });
        });
    });
}

/// Returns whether the pointer is over the toast and whether close was pressed.
fn paint_toast(ui: &mut Ui, problem: &Problem, fade: f32) -> (bool, bool) {
    let mut close = false;
    let mut hovered = false;
    let previous = ui.opacity();
    // Full opacity stays as normal text shapes. A layer is only needed while fading.
    if fade > 0.0 {
        ui.set_opacity((previous * (1.0_f32 - fade)).clamp(0.0_f32, 1.0_f32));
    }
    ui.push_id(problem.id, |ui| {
        let card = card_frame(problem.severity.fill()).show(ui, |ui| {
            ui.horizontal(|ui| {
                paint_severity(ui, problem.severity);
                ui.vertical(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        let button = ui
                            .add(
                                egui::Button::new(
                                    RichText::new("×").size(16.0).color(theme::text_strong()),
                                )
                                .frame(false),
                            )
                            .on_hover_text("Close");
                        close = button.clicked();
                        hovered |= button.hovered();
                        ui.add(
                            egui::Label::new(
                                RichText::new(&problem.title)
                                    .strong()
                                    .color(theme::severity_on()),
                            )
                            .wrap(),
                        );
                    });
                    if !problem.detail.is_empty() && problem.detail != problem.title {
                        ui.add(
                            egui::Label::new(RichText::new(&problem.detail).color(theme::text()))
                                .wrap(),
                        );
                    }
                });
            });
        });
        hovered |= card.response.hovered();
    });
    if fade > 0.0 {
        ui.set_opacity(previous);
    }
    (hovered, close)
}

fn more_bar(ui: &mut Ui, hidden: usize) {
    Frame::new()
        .fill(theme::surface())
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new(format!("{hidden} more")).color(theme::muted()));
            });
        });
}

fn badge_label(ui: &mut Ui, count: usize, color: Color32) -> egui::Response {
    Frame::new()
        .fill(color)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(6, 1))
        .show(ui, |ui| {
            ui.label(
                RichText::new(count.to_string())
                    .size(12.0)
                    .color(theme::primary_text()),
            )
        })
        .response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(detail: &str) -> Incident {
        incident("git-settings", Severity::Error, "Git settings", detail)
    }

    #[test]
    fn session_log_keeps_distinct_problems_once() {
        let mut log = ProblemLog::default();
        assert!(log.report(sample("authentication failed")));
        assert!(!log.report(sample("authentication failed")));
        assert!(log.report(sample("repository not found")));
        assert_eq!(log.count(ProblemCategory::Project), 2);
        assert_eq!(log.worst(ProblemCategory::Project), Some(Severity::Error));
        assert!(log.expanded.contains(&ProblemCategory::Project));
        assert!(log.hides_status("authentication failed"));
        assert!(log.hides_status("Git: authentication failed"));
        assert!(!log.hides_status("Saved README.md"));
    }

    #[test]
    fn critical_fill_is_redder_than_info() {
        let info = Severity::Info.fill();
        let critical = Severity::Critical.fill();
        assert!(critical.r() > critical.g());
        assert!(critical.r() > critical.b());
        assert!(critical.r() > info.r());
        assert!(Severity::Critical > Severity::Error);
        assert_eq!(Severity::Info.glyph(), "i");
        assert_eq!(Severity::Critical.glyph(), "!");
    }

    #[test]
    fn session_cap_drops_the_oldest_problem() {
        let mut log = ProblemLog::default();
        for index in 0..MAX_PROBLEMS {
            assert!(log.report(sample(&format!("problem {index}"))));
        }
        assert!(!log.report(sample("problem 0")));
        assert!(log.report(sample("newest")));
        assert_eq!(log.count(ProblemCategory::Project), MAX_PROBLEMS);
        assert!(
            log.problems
                .iter()
                .any(|problem| problem.detail == "newest")
        );
        assert!(
            log.problems
                .iter()
                .all(|problem| problem.detail != "problem 0")
        );
    }

    #[test]
    fn category_tab_shows_count_and_expands_to_the_message() {
        let mut log = ProblemLog::default();
        assert!(log.report(incident(
            "database",
            Severity::Critical,
            "Database",
            "password rejected",
        )));
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 600.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| log.show_panel(ui));
            },
        );
        let text = collected_text(&output);
        assert!(text.contains("Problems"), "{text}");
        assert!(text.contains("Project"), "{text}");
        assert!(text.contains('1'), "{text}");
        assert!(text.contains("password rejected"), "{text}");
        log.toggle(ProblemCategory::Project);
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| log.show_panel(ui));
        });
        let collapsed = collected_text(&output);
        assert!(collapsed.contains("Project"), "{collapsed}");
        assert!(!collapsed.contains("password rejected"), "{collapsed}");
    }

    #[test]
    fn toast_fades_after_a_few_seconds_and_close_dismisses_it() {
        let mut log = ProblemLog::default();
        assert!(log.report(sample("authentication failed")));
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut time = 10.0;
        // An anchored area learns its size on the first frame and paints on the next.
        let _warm = toast_frame(&mut log, &ctx, time);
        time += 0.05;
        let first = toast_frame(&mut log, &ctx, time);
        let shown = collected_text(&first);
        assert!(shown.contains("Git settings"), "{shown}");
        assert!(shown.contains("authentication failed"), "{shown}");
        assert!(shown.contains('×'), "{shown}");
        assert_eq!(log.toasts.len(), 1);
        for _ in 0..16 {
            time += 0.25;
            let _output = toast_frame(&mut log, &ctx, time);
        }
        assert!(
            log.toasts.is_empty(),
            "toast should leave after the hold and fade, left {:?}",
            log.toasts
        );
        assert_eq!(log.count(ProblemCategory::Project), 1);

        assert!(log.report(incident("ssh", Severity::Warning, "SSH", "host is offline",)));
        let opened = toast_frame(&mut log, &ctx, time);
        let opened_text = collected_text(&opened);
        let close = text_position(&opened, "×");
        assert!(
            close.is_some(),
            "close control missing from toast text:\n{opened_text}"
        );
        let close = close.unwrap_or(egui::Pos2::ZERO);
        time += 0.05;
        let _pressed = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 800.0),
                )),
                time: Some(time),
                events: vec![
                    egui::Event::PointerMoved(close),
                    egui::Event::PointerButton {
                        pos: close,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                ..Default::default()
            },
            |ctx| log.show_toasts(ctx),
        );
        time += 0.05;
        let _released = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 800.0),
                )),
                time: Some(time),
                events: vec![egui::Event::PointerButton {
                    pos: close,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| log.show_toasts(ctx),
        );
        assert!(
            log.toasts.iter().any(|toast| toast.dismissed),
            "close control should start dismissing the toast"
        );
        for _ in 0..4 {
            time += 0.25;
            let _output = toast_frame(&mut log, &ctx, time);
        }
        assert!(
            log.toasts.is_empty(),
            "a closed toast finishes fading, left {:?}",
            log.toasts
        );
        assert!(
            log.problems
                .iter()
                .any(|problem| problem.detail == "host is offline"),
            "closing a toast keeps the session problem"
        );
    }

    fn toast_frame(log: &mut ProblemLog, ctx: &egui::Context, time: f64) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 800.0),
                )),
                time: Some(time),
                ..Default::default()
            },
            |ctx| log.show_toasts(ctx),
        )
    }

    fn collected_text(output: &egui::FullOutput) -> String {
        let mut text = String::new();
        for shape in &output.shapes {
            collect_shape(&shape.shape, &mut text);
        }
        text
    }

    fn collect_shape(shape: &egui::Shape, text: &mut String) {
        match shape {
            egui::Shape::Text(painted) => {
                text.push_str(painted.galley.text());
                text.push('\n');
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_shape(shape, text);
                }
            }
            _ => {}
        }
    }

    fn text_position(output: &egui::FullOutput, label: &str) -> Option<egui::Pos2> {
        fn find(shape: &egui::Shape, label: &str) -> Option<egui::Pos2> {
            match shape {
                egui::Shape::Text(painted) if painted.galley.text() == label => {
                    Some(painted.pos + painted.galley.size() * 0.5)
                }
                egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, label)),
                _ => None,
            }
        }
        output
            .shapes
            .iter()
            .find_map(|shape| find(&shape.shape, label))
    }
}
