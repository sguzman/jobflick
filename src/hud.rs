use crate::protocol::{Job, Request, State};
use crate::{copy_clipboard, send};
use anyhow::{anyhow, Context, Result};
use std::process::Command;
use eframe::egui;
use std::time::{Duration, Instant};

// A scoped, named Hyprland rule is installed *before* creating the window.
// This keeps the HUD out of the tiling tree from its first frame, without
// changing the user's Hyprland configuration or touching other applications.
const HYPRLAND_FLOAT_RULE: &str = r#"hl.window_rule({ name = "jobflick-hud-overlay", match = { class = "^io[.]github[.]sguzman[.]jobflick$" }, float = true, center = true, size = { 760, 520 } })"#;

fn prepare_hyprland_overlay() -> Result<()> {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
        return Ok(());
    }
    let output = Command::new("hyprctl")
        .arg("eval")
        .arg(HYPRLAND_FLOAT_RULE)
        .output()
        .context("run hyprctl eval for Jobflick HUD")?;
    if !output.status.success() {
        return Err(anyhow!(
            "Hyprland rejected Jobflick overlay rule: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

pub fn open() -> Result<()> {
    let overlay_warning = prepare_hyprland_overlay().err().map(|error| format!("{error:#}"));
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Jobflick")
            .with_app_id("io.github.sguzman.jobflick")
            .with_inner_size([760.0, 520.0])
            .with_max_inner_size([760.0, 520.0])
            .with_resizable(false)
            .with_min_inner_size([480.0, 340.0])
            .with_decorations(false)
            .with_always_on_top(),
        ..Default::default()
    };
    eframe::run_native(
        "Jobflick",
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Ok(Box::new(Hud {
                message: overlay_warning.unwrap_or_default(),
                ..Hud::default()
            }))
        }),
    )
    .map_err(|error| anyhow!("Cannot open Jobflick HUD: {error}"))
}

struct Hud {
    jobs: Vec<Job>,
    search: String,
    selection: usize,
    show_consumed: bool,
    last_refresh: Option<Instant>,
    focus_search: bool,
    message: String,
}

impl Default for Hud {
    fn default() -> Self {
        Self {
            jobs: Vec::new(),
            search: String::new(),
            selection: 0,
            show_consumed: false,
            last_refresh: None,
            focus_search: true,
            message: String::new(),
        }
    }
}

impl Hud {
    fn refresh(&mut self) {
        let reply = send(Request {
            action: "list".into(),
            all: self.show_consumed,
            ..Request::default()
        });
        match reply {
            Ok(reply) => self.jobs = reply.jobs,
            Err(error) => self.message = format!("Cannot load jobs: {error:#}"),
        }
        self.last_refresh = Some(Instant::now());
    }

    fn matches(&self, job: &Job) -> bool {
        let query = self.search.trim().to_lowercase();
        query.is_empty()
            || job.command.to_lowercase().contains(&query)
            || job.id.contains(&query)
            || job.state.label().to_lowercase().contains(&query)
    }

    fn copy(&mut self, job: &Job, consume: bool, ctx: &egui::Context) {
        if !job.state.finished() {
            self.message = "This job is not finished yet".into();
            return;
        }
        let result = (|| -> Result<()> {
            let reply = send(Request {
                action: "show".into(),
                id: Some(job.id.clone()),
                ..Request::default()
            })?;
            let report = reply.report.ok_or_else(|| anyhow!("Job report was empty"))?;
            copy_clipboard(&report)?;
            if consume {
                send(Request {
                    action: "consume".into(),
                    id: Some(job.id.clone()),
                    ..Request::default()
                })?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Err(error) => self.message = format!("Copy failed: {error:#}"),
        }
    }
}

impl eframe::App for Hud {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.last_refresh.is_none_or(|t| t.elapsed() >= Duration::from_millis(400)) {
            self.refresh();
        }
        ctx.request_repaint_after(Duration::from_millis(400));
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        let filtered: Vec<Job> = self.jobs.iter()
            .filter(|job| self.matches(job))
            .cloned()
            .collect();
        self.selection = self.selection.min(filtered.len().saturating_sub(1));

        if ctx.input(|input| input.key_pressed(egui::Key::ArrowDown)) {
            self.selection = (self.selection + 1).min(filtered.len().saturating_sub(1));
        }
        if ctx.input(|input| input.key_pressed(egui::Key::ArrowUp)) {
            self.selection = self.selection.saturating_sub(1);
        }

        let mut copy_action = None;
        if ctx.input(|input| input.key_pressed(egui::Key::Enter)) {
            copy_action = Some(!ctx.input(|input| input.modifiers.shift));
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::default().inner_margin(18.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("JOBFLICK");
                    ui.add_space(12.0);
                    let running = self.jobs.iter().filter(|j| j.state == State::Running).count();
                    let queued = self.jobs.iter().filter(|j| j.state == State::Queued).count();
                    let finished = self.jobs.iter().filter(|j| j.state.finished()).count();
                    ui.weak(format!("{running} running  ·  {queued} queued  ·  {finished} finished"));
                });
                ui.add_space(8.0);
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text("Search commands, states, or job IDs...")
                        .desired_width(f32::INFINITY)
                );
                if self.focus_search {
                    response.request_focus();
                    self.focus_search = false;
                }
                ui.add_space(5.0);
                if ui.checkbox(&mut self.show_consumed, "Include consumed history").changed() {
                    self.refresh();
                    self.selection = 0;
                }
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if filtered.is_empty() {
                            ui.weak("No matching jobs");
                        }
                        for (i, job) in filtered.iter().enumerate() {
                            let selected = i == self.selection;
                            let title = format!(
                                "{}  {}  {}",
                                job.short_id(),
                                job.state.label(),
                                job.summary()
                            );
                            let label = if selected {
                                egui::RichText::new(title).strong()
                            } else {
                                egui::RichText::new(title)
                            };
                            let response = ui.selectable_label(selected, label);
                            if response.clicked() {
                                self.selection = i;
                            }
                            if job.consumed {
                                ui.weak("    Archived result");
                            }
                        }
                    });
                ui.separator();
                if let Some(job) = filtered.get(self.selection) {
                    ui.horizontal(|ui| {
                        let color = match job.state {
                            State::Succeeded => egui::Color32::LIGHT_GREEN,
                            State::Failed | State::Interrupted => egui::Color32::LIGHT_RED,
                            State::Running => egui::Color32::LIGHT_BLUE,
                            State::Queued => egui::Color32::YELLOW,
                            State::Cancelled => egui::Color32::GRAY,
                        };
                        ui.colored_label(color, job.state.label());
                        if let Some(code) = job.exit_code {
                            ui.weak(format!("exit {code}"));
                        }
                        if let (Some(start), Some(end)) = (job.started_at, job.completed_at) {
                            ui.weak(format!("{}s", (end.saturating_sub(start)) / 1000));
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.add_enabled(job.state.finished(), egui::Button::new("Copy & consume  ↵")).clicked() {
                            copy_action = Some(true);
                        }
                        if ui.add_enabled(job.state.finished(), egui::Button::new("Copy only  Shift+↵")).clicked() {
                            copy_action = Some(false);
                        }
                        if job.state == State::Queued && ui.button("Cancel queued").clicked() {
                            match send(Request {
                                action: "cancel".into(),
                                id: Some(job.id.clone()),
                                ..Request::default()
                            }) {
                                Ok(_) => self.refresh(),
                                Err(error) => self.message = format!("{error:#}"),
                            }
                        }
                    });
                } else {
                    ui.weak("Select a job to copy its report when finished");
                }
                if !self.message.is_empty() {
                    ui.colored_label(egui::Color32::LIGHT_RED, &self.message);
                }
                ui.weak("↑↓ navigate  ·  Enter copy & consume  ·  Shift+Enter copy only  ·  Esc close");
            });

        if let (Some(consume), Some(job)) = (copy_action, filtered.get(self.selection)) {
            self.copy(job, consume, ctx);
        }
    }
}
