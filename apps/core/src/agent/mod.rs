use serde::Serialize;

pub(crate) mod tools;

/// Step/approval/done events streamed to the chat view as JSON.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "t")]
pub(crate) enum AgentEvent {
    #[serde(rename = "agentStep")]
    Step {
        step: u64,
        tool: String,
        state: String,
        detail: String,
    },
    #[serde(rename = "agentApproval")]
    Approval {
        call_id: String,
        tool: String,
        args_summary: String,
    },
    #[serde(rename = "agentDone")]
    Done { summary: String },
}

impl AgentEvent {
    pub(crate) fn step(
        step: u64,
        tool: impl Into<String>,
        state: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self::Step {
            step,
            tool: tool.into(),
            state: state.into(),
            detail: detail.into(),
        }
    }

    pub(crate) fn approval(
        call_id: impl Into<String>,
        tool: impl Into<String>,
        args_summary: impl Into<String>,
    ) -> Self {
        Self::Approval {
            call_id: call_id.into(),
            tool: tool.into(),
            args_summary: args_summary.into(),
        }
    }

    pub(crate) fn done(summary: impl Into<String>) -> Self {
        Self::Done {
            summary: summary.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_event_shapes_match_ui_contract() {
        let e = AgentEvent::step(1, "fs_list", "running", "listed 4 entries");
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["t"], "agentStep");
        assert_eq!(v["step"], 1);
        assert_eq!(v["tool"], "fs_list");
        assert_eq!(v["state"], "running");
        assert_eq!(v["detail"], "listed 4 entries");
    }

    #[test]
    fn approval_event_shape_matches_ui_contract() {
        let e = AgentEvent::approval("call-1", "shell_exec", "echo hi");
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["t"], "agentApproval");
        assert_eq!(v["call_id"], "call-1");
        assert_eq!(v["tool"], "shell_exec");
        assert_eq!(v["args_summary"], "echo hi");
    }

    #[test]
    fn done_event_shape_matches_ui_contract() {
        let e = AgentEvent::done("listed files");
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["t"], "agentDone");
        assert_eq!(v["summary"], "listed files");
    }
}
