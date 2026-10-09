use serde::{Deserialize, Serialize};

/// Largest Fish command that can be submitted through any interface.
pub const MAX_COMMAND_BYTES: usize = 128 * 1024;

/// UI text for an accepted running-job cancellation request.
pub const STOPPING_NOTE: &str = "Cancellation requested; stopping process group";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Interrupted,
}

impl State {
    pub fn finished(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::Running => "Running",
            Self::Succeeded => "Succeeded",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
            Self::Interrupted => "Interrupted",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub command: String,
    pub state: State,
    pub submitted_at: u64,
    pub started_at: Option<u64>,
    pub completed_at: Option<u64>,
    pub exit_code: Option<i32>,
    pub consumed: bool,
    pub log_path: String,
    pub note: Option<String>,
}

impl Job {
    /// Queued cancellation has no worker and therefore no output log.
    pub fn no_output_expected(&self) -> bool {
        self.state == State::Cancelled && self.started_at.is_none()
    }

    pub fn short_id(&self) -> &str {
        &self.id[..self.id.len().min(8)]
    }

    pub fn summary(&self) -> String {
        let first = self.command.lines().find(|line| !line.trim().is_empty()).unwrap_or("(empty)");
        let first = first.trim();
        if first.chars().count() > 75 {
            format!("{}…", first.chars().take(74).collect::<String>())
        } else {
            first.to_owned()
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Request {
    pub action: String,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub all: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    pub message: String,
    #[serde(default)]
    pub jobs: Vec<Job>,
    #[serde(default)]
    pub job: Option<Job>,
    #[serde(default)]
    pub report: Option<String>,
}

impl Response {
    pub fn success(message: impl Into<String>) -> Self {
        Self { ok: true, message: message.into(), ..Self::default() }
    }
    pub fn failure(message: impl Into<String>) -> Self {
        Self { ok: false, message: message.into(), ..Self::default() }
    }
}
