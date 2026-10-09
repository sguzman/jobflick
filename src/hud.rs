use crate::protocol::{Job, Request, State, STOPPING_NOTE};
use crate::{copy_clipboard, send};
use anyhow::{anyhow, Context, Result};
use eframe::egui;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::process::Command;
use std::time::{Duration, Instant};

// A scoped, named Hyprland rule is installed *before* creating the window.
// This keeps the HUD out of the tiling tree from its first frame, without
// changing the user's Hyprland configuration or touching other applications.
// Never read an entire potentially huge compiler/test log into the GUI.
const PREVIEW_LIMIT_BYTES: u64 = 8 * 1024;

fn preview_output(job: &Job) -> String {
    let mut prefix = String::new();
    if let Some(note) = &job.note {
        prefix.push_str(note);
        prefix.push('\n');
    }

    let output = (|| -> Result<(String, bool)> {
        let mut file = File::open(&job.log_path)?;
        let len = file.metadata()?.len();
        let offset = len.saturating_sub(PREVIEW_LIMIT_BYTES);
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = Vec::with_capacity((len - offset) as usize);
        file.take(PREVIEW_LIMIT_BYTES).read_to_end(&mut bytes)?;
        // The start of a truncated tail can split a multibyte UTF-8 character.
        let text = String::from_utf8_lossy(&bytes).into_owned();
        Ok((text, offset > 0))
    })();

    match output {
        Ok((text, truncated)) if !text.is_empty() => {
            if truncated {
                prefix.push_str("… earlier output omitted …\n");
            }
            prefix.push_str(&text);
        }
        Ok(_) => {
            if prefix.is_empty() {
                prefix.push_str(match job.state {
                    State::Queued => "Waiting for an execution slot.",
                    State::Running => "Running; no output yet.",
                    _ => "No output was captured.",
                });
            }
        }
        Err(error) => {
            if job.state.finished() {
                prefix.push_str(&format!("Saved output unavailable: {error:#}"));
            } else if prefix.is_empty() {
                prefix.push_str(match job.state {
                    State::Queued => "Waiting for an execution slot.",
                    State::Running => "Running; no output yet.",
                    _ => "Output not yet available.",
                });
            }
        }
    }
    prefix
}

const HYPRLAND_FLOAT_RULE: &str = r#"hl.window_rule({ name = "jobflick-hud-overlay", match = { class = "^io[.]github[.]sguzman[.]jobflick$" }, float = true, center = true, size = { 720, 480 } })"#;

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
            .with_transparent(false)
            .with_inner_size([720.0, 480.0])
            .with_max_inner_size([720.0, 480.0])
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
            let mut visuals = egui::Visuals::dark();
            visuals.panel_fill = egui::Color32::from_rgb(20, 22, 26);
            visuals.window_fill = egui::Color32::from_rgb(20, 22, 26);
            cc.egui_ctx.set_visuals(visuals);
            Ok(Box::new(Hud {
                message: overlay_warning.unwrap_or_default(),
                ..Hud::default()
            }))
        }),
    )
    .map_err(|error| anyhow!("Cannot open Jobflick HUD: {error}"))
}

// Resolve selection by durable job ID instead of row position. New jobs sort
// ahead of older ones; retaining an index alone could copy/consume a different job.
fn selection_index(jobs: &[Job], selected_id: Option<&str>) -> Option<usize> {
    if jobs.is_empty() {
        return None;
    }
    Some(
        selected_id
            .and_then(|id| jobs.iter().position(|job| job.id == id))
            .unwrap_or(0),
    )
}

struct Hud {
    jobs: Vec<Job>,
    search: String,
    selection: usize,
    selected_id: Option<String>,
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
            selected_id: None,
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
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // The first frame and compositor background are fully opaque.
        egui::Rgba::from_rgb(20.0 / 255.0, 22.0 / 255.0, 26.0 / 255.0).to_array()
    }

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
        self.selection = selection_index(&filtered, self.selected_id.as_deref()).unwrap_or(0);

        if ctx.input(|input| input.key_pressed(egui::Key::ArrowDown)) {
            self.selection = (self.selection + 1).min(filtered.len().saturating_sub(1));
        }
        if ctx.input(|input| input.key_pressed(egui::Key::ArrowUp)) {
            self.selection = self.selection.saturating_sub(1);
        }
        self.selected_id = filtered.get(self.selection).map(|job| job.id.clone());

        let mut copy_action = None;
        if ctx.input(|input| input.key_pressed(egui::Key::Enter)) {
            copy_action = Some(!ctx.input(|input| input.modifiers.shift));
        }
        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(egui::Color32::from_rgb(20, 22, 26))
                    .inner_margin(18.0),
            )
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
                    self.selected_id = None;
                }
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height(130.0)
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
                                self.selected_id = Some(job.id.clone());
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
                    ui.add_space(6.0);
                    ui.weak("Recent output");
                    egui::Frame::default()
                        .fill(egui::Color32::from_rgb(30, 33, 39))
                        .inner_margin(egui::Margin::same(8))
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("jobflick-output-preview")
                                .max_height(125.0)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(preview_output(job)).monospace()
                                        ).wrap()
                                    );
                                });
                        });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.add_enabled(job.state.finished(), egui::Button::new("Copy & consume  ↵")).clicked() {
                            copy_action = Some(true);
                        }
                        if ui.add_enabled(job.state.finished(), egui::Button::new("Copy only  Shift+↵")).clicked() {
                            copy_action = Some(false);
                        }
                        let can_cancel = matches!(job.state, State::Queued | State::Running);
                        let stopping = job.state == State::Running
                            && job.note.as_deref() == Some(STOPPING_NOTE);
                        let label = if stopping { "Stopping…" } else if job.state == State::Running {
                            "Stop running"
                        } else {
                            "Cancel queued"
                        };
                        if ui.add_enabled(can_cancel && !stopping, egui::Button::new(label)).clicked() {
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
                ui.weak("↑↓ select  ·  Enter copy & consume  ·  Shift+Enter keep  ·  Esc close");
            });

        if let (Some(consume), Some(job)) = (copy_action, filtered.get(self.selection)) {
            self.copy(job, consume, ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_handles_missing_log_and_job_note() {
        let job = Job {
            id: "preview-test".into(),
            command: "missing-command".into(),
            state: State::Failed,
            submitted_at: 0,
            started_at: Some(1),
            completed_at: Some(2),
            exit_code: Some(127),
            consumed: false,
            log_path: "/this-jobflick-test-file-should-not-exist".into(),
            note: Some("worker could not launch".into()),
        };
        let preview = preview_output(&job);
        assert!(preview.contains("worker could not launch"));
        assert!(preview.contains("Saved output unavailable:"));
    }

    #[test]
    fn preview_reads_tail_without_loading_full_log() {
        use std::fs;
        use std::io::Write;
        let path = std::env::temp_dir().join(format!("jobflick-preview-{}.log", uuid::Uuid::new_v4()));
        let mut file = File::create(&path).unwrap();
        file.write_all("x".repeat((PREVIEW_LIMIT_BYTES + 40) as usize).as_bytes()).unwrap();
        file.write_all(b"failure: missing command").unwrap();
        drop(file);
        let job = Job {
            id: "preview-test".into(),
            command: "test".into(),
            state: State::Failed,
            submitted_at: 0,
            started_at: Some(1),
            completed_at: Some(2),
            exit_code: Some(127),
            consumed: false,
            log_path: path.display().to_string(),
            note: None,
        };
        let output = preview_output(&job);
        fs::remove_file(&path).unwrap();
        assert!(output.starts_with("… earlier output omitted …\n"));
        assert!(output.ends_with("failure: missing command"));
        assert!(output.len() < PREVIEW_LIMIT_BYTES as usize + 100);
    }
    fn example_job(id: &str) -> Job {
        Job {
            id: id.into(),
            command: format!("echo {id}"),
            state: State::Succeeded,
            submitted_at: 1,
            started_at: Some(1),
            completed_at: Some(2),
            exit_code: Some(0),
            consumed: false,
            log_path: "/dev/null".into(),
            note: None,
        }
    }

    #[test]
    fn selection_tracks_job_when_newer_jobs_arrive() {
        let initial = vec![example_job("older"), example_job("oldest")];
        let selected = initial[1].id.clone();
        let refreshed = vec![example_job("new"), initial[0].clone(), initial[1].clone()];
        assert_eq!(selection_index(&refreshed, Some(&selected)), Some(2));
        assert_eq!(refreshed[selection_index(&refreshed, Some(&selected)).unwrap()].id, "oldest");
    }

    #[test]
    fn vanished_selection_falls_back_to_first_matching_job() {
        let filtered = vec![example_job("visible")];
        assert_eq!(selection_index(&filtered, Some("filtered-out")), Some(0));
        assert_eq!(selection_index(&filtered, None), Some(0));
        assert_eq!(selection_index(&[], Some("visible")), None);
    }

    #[test]
    fn hud_clear_is_opaque() {
        let color = <Hud as eframe::App>::clear_color(&Hud::default(), &egui::Visuals::dark());
        assert_eq!(color[3], 1.0);
    }
}
