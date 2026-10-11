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
    TimedOut,
    Interrupted,
}

impl State {
    pub fn finished(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::TimedOut | Self::Interrupted
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::Running => "Running",
            Self::Succeeded => "Succeeded",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
            Self::TimedOut => "Timed out",
            Self::Interrupted => "Interrupted",
        }
    }

    /// freedesktop.org notification category for the final job state.
    /// Queued and Running use the notification daemon's default styling.
    pub fn notification_category(self) -> Option<&'static str> {
        match self {
            Self::Succeeded => Some("success"),
            Self::Failed | Self::TimedOut => Some("failure"),
            Self::Cancelled | Self::Interrupted => Some("warning"),
            Self::Queued | Self::Running => None,
        }
    }
}

/// Structured metadata is separate from raw Fish jobs for backwards compatibility.
/// This is NOT authorization: it is persisted only after the daemon validates
/// the corresponding Execarium v1 document and its own execution policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredSpec {
    pub schema: u64,
    pub shell: String,
    pub cwd: String,
    pub privilege: String,
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub specification: Option<StructuredSpec>,
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
    /// A complete Execarium YAML document, accepted only by submit_spec.
    #[serde(default)]
    pub spec_yaml: Option<String>,
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

#[cfg(test)]
mod notification_category_tests {
    use super::State;

    #[test]
    fn maps_every_state_to_the_expected_mako_category() {
        assert_eq!(State::Queued.notification_category(), None);
        assert_eq!(State::Running.notification_category(), None);
        assert_eq!(State::Succeeded.notification_category(), Some("success"));
        assert_eq!(State::Failed.notification_category(), Some("failure"));
        assert_eq!(State::TimedOut.notification_category(), Some("failure"));
        assert_eq!(State::Cancelled.notification_category(), Some("warning"));
        assert_eq!(State::Interrupted.notification_category(), Some("warning"));
    }
}
