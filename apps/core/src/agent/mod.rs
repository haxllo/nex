use serde::Serialize;

pub(crate) mod approvals;
pub(crate) mod r#loop;
pub(crate) mod store;
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<String>,
    },
    #[serde(rename = "agentApproval")]
    Approval {
        call_id: String,
        tool: String,
        args_summary: String,
    },
    #[serde(rename = "agentDone")]
    Done {
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<String>,
    },
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
            run_id: None,
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
            run_id: None,
        }
    }

    /// Attach the owning run id (additive: the UI ignores unknown fields,
    /// and the field is skipped when unset so old shapes are unchanged).
    pub(crate) fn with_run_id(self, id: impl Into<String>) -> Self {
        let id = id.into();
        match self {
            Self::Step {
                step,
                tool,
                state,
                detail,
                ..
            } => Self::Step {
                step,
                tool,
                state,
                detail,
                run_id: Some(id),
            },
            Self::Approval { .. } => self,
            Self::Done { summary, .. } => Self::Done {
                summary,
                run_id: Some(id),
            },
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
        // Additive run_id: absent by default so old shapes are unchanged.
        assert!(v.get("run_id").is_none());
    }

    #[test]
    fn run_id_attaches_to_step_and_done_only() {
        let v = serde_json::to_value(AgentEvent::step(1, "fs_list", "done", "x").with_run_id("r1")).unwrap();
        assert_eq!(v["run_id"], "r1");
        assert_eq!(v["tool"], "fs_list");
        let v = serde_json::to_value(AgentEvent::done("ok").with_run_id("r1")).unwrap();
        assert_eq!(v["run_id"], "r1");
        // Approval cards are transient: run_id is a no-op there.
        let v = serde_json::to_value(AgentEvent::approval("c1", "shell_exec", "x").with_run_id("r1")).unwrap();
        assert!(v.get("run_id").is_none());
    }
}
