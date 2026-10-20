use serde::{Deserialize, Serialize};

/// Largest Fish command contained in a validated Execarium document.
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

/// Versioned metadata persisted after Execarium validation and consumer policy.
/// Optional only for legacy completed records; new jobs always require it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredSpec {
    pub schema: u64,
    pub shell: String,
    pub cwd: String,
    pub privilege: String,
    pub timeout_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<execarium::v3::Environment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    /// Display name authored in Execarium schema 4; absent for earlier schemas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Opaque caller-generated tab identity, separate from the YAML specification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_tab_id: Option<String>,
    /// Stable visual identity; old saved jobs derive their color from the ID.
    #[serde(default)]
    pub color: String,
    /// Schema-1/3 script, or schema-2/3 first-step display summary.
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub specification: Option<StructuredSpec>,
    /// Schema-2/3 independent steps, absent from scripts and legacy records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commands: Option<Vec<String>>,
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

    pub fn display_name(&self) -> String {
        self.name.clone().unwrap_or_else(|| self.summary())
    }

    pub fn visual_color(&self) -> String {
        if self.color.is_empty() { color_for_id(&self.id) } else { self.color.clone() }
    }

    pub fn summary(&self) -> String {
        let first = self
            .command
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("(empty)");
        let first = first.trim();
        if first.chars().count() > 75 {
            format!("{}…", first.chars().take(74).collect::<String>())
        } else {
            first.to_owned()
        }
    }
}

/// Give each job a repeatable, high-contrast colored label without changing
/// its status color. Hue comes from all UUID bytes, not its short display ID.
pub fn color_for_id(id: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in id.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let hue = (hash % 360) as f32;
    let c = 0.66f32 * (1.0 - (2.0 * 0.63 - 1.0).abs());
    let x = c * (1.0 - (((hue / 60.0) % 2.0) - 1.0).abs());
    let m = 0.63f32 - c / 2.0;
    let (r, g, b) = match (hue / 60.0) as u32 {
        0 => (c, x, 0.0), 1 => (x, c, 0.0), 2 => (0.0, c, x),
        3 => (0.0, x, c), 4 => (x, 0.0, c), _ => (c, 0.0, x),
    };
    format!("#{:02X}{:02X}{:02X}",
        ((r + m) * 255.0).round() as u8,
        ((g + m) * 255.0).round() as u8,
        ((b + m) * 255.0).round() as u8)
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Request {
    pub action: String,
    #[serde(default)]
    /// Legacy raw command field: never accepted for new submissions.
    pub command: Option<String>,
    /// A complete Execarium YAML document, mandatory for all new submissions.
    #[serde(default)]
    pub spec_yaml: Option<String>,
    #[serde(default)]
    pub origin_tab_id: Option<String>,
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
        Self {
            ok: true,
            message: message.into(),
            ..Self::default()
        }
    }
    pub fn failure(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            message: message.into(),
            ..Self::default()
        }
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

#[cfg(test)]
mod origin_metadata_tests {
    use super::*;
    #[test]
    fn color_is_stable_and_valid() {
        let a = color_for_id("4e7b05e1-d921-4f91-9d27-19d3b2e31101");
        assert_eq!(a, color_for_id("4e7b05e1-d921-4f91-9d27-19d3b2e31101"));
        assert!(a.starts_with('#') && a.len() == 7);
    }
    #[test]
    fn request_origin_is_optional_for_cli_compatibility() {
        let request: Request = serde_json::from_str(r#"{"action":"submit_spec","spec_yaml":"schema: 1"}"#).unwrap();
        assert!(request.origin_tab_id.is_none());
    }
}
