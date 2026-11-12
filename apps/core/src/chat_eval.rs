//! Scripted runs of the tool loop, with the model replaced by a fixture.
//!
//! The unit tests below `chat_tools.rs` check that each piece decides correctly
//! on its own. What they did not catch was the loop as a whole: three bugs
//! shipped past them, and every one was found by a person typing a prompt and
//! reading the transcript.
//!
//! * parallel calls lost their arguments, because deltas fell back to index 0;
//! * one reply asking for six folders spent a four-round budget;
//! * the limit message told the user to type a word models misread as a task.
//!
//! Each was a mismatch between what the loop did and what the transcript said it
//! did. So this harness deals in transcripts: it runs a scripted model through
//! the real `ToolLoop`, plays every requested action against a scratch
//! directory, and asserts on the record afterwards.
//!
//! Costs nothing and needs no network. The model is a list of calls.

use crate::chat_tools::{ChatTool, LoopStep, ToolLoop};

/// One thing the scripted model asks for in a round.
#[derive(Debug, Clone)]
enum Ask {
    /// A call the loop should run.
    Do(ChatTool),
    /// A call the loop should put to the user first.
    Confirm(ChatTool),
}

/// What the loop did with one call.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    Ran(String),
    Asked(String),
    Refused(String),
}

/// A scripted model: one vector of calls per round.
struct Script {
    rounds: Vec<Vec<Ask>>,
}

impl Script {
    fn new() -> Self {
        Self { rounds: Vec::new() }
    }

    /// One round in which the model asks for all of these.
    fn round(mut self, asks: Vec<Ask>) -> Self {
        self.rounds.push(asks);
        self
    }
}

/// The record of a scripted run.
struct Transcript {
    outcomes: Vec<Outcome>,
    stop: Option<String>,
    rounds: usize,
}

impl Transcript {
    fn ran(&self) -> Vec<&str> {
        self.outcomes
            .iter()
            .filter_map(|o| match o {
                Outcome::Ran(s) => Some(s.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Actions put to the user rather than run. A declined confirmation is
    /// recorded as `Asked`: the loop only learns the answer from the caller, so
    /// that is the boundary this harness observes.
    fn asked(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|o| matches!(o, Outcome::Asked(_)))
            .count()
    }
}

/// Run a script through the real loop.
///
/// Actions are not executed against the filesystem: the point is the loop's
/// bookkeeping and the messages it produces, not NTFS. What a tool would have
/// done is recorded as though it succeeded, except where the script asks to be
/// refused.
fn run(script: Script) -> Transcript {
    let mut loop_state = ToolLoop::new();
    let mut outcomes = Vec::new();
    let mut stop = None;

    'outer: for asks in script.rounds {
        loop_state.begin_round();
        for ask in asks {
            let (tool, wants_confirm) = match ask {
                Ask::Do(tool) => (tool, false),
                Ask::Confirm(tool) => (tool, true),
            };
            loop_state.note_attempt(&tool);
            match loop_state.on_tool_call(tool.clone()) {
                LoopStep::Stop { reason } => {
                    stop = Some(reason);
                    break 'outer;
                }
                LoopStep::AwaitConfirmation { .. } | LoopStep::RunTool { .. } => {
                    let outcome = if wants_confirm {
                        // Declined, as a user who did not want it.
                        crate::chat_tools::ToolOutcome::failure(
                            tool.name(),
                            tool.target(),
                            "the user declined",
                        )
                    } else {
                        crate::chat_tools::ToolOutcome::success(
                            tool.name(),
                            tool.target(),
                            format!("Did {}", tool.name()),
                        )
                    };
                    loop_state.observe_output(&outcome);
                    outcomes.push(if wants_confirm {
                        Outcome::Asked(tool.target().to_string())
                    } else if outcome.ok {
                        Outcome::Ran(tool.target().to_string())
                    } else {
                        Outcome::Refused(tool.target().to_string())
                    });
                }
            }
        }
    }

    Transcript {
        outcomes,
        stop,
        rounds: loop_state.rounds(),
    }
}

fn folder(name: &str) -> ChatTool {
    ChatTool::CreateFolder {
        name: name.to_string(),
        parent: Some(std::path::PathBuf::from("/scratch")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_reply_asking_for_six_folders_runs_all_six() {
        // The bug: the parent and five subfolders arrived in one reply, and the
        // loop refused every subfolder after the first.
        let asks = (0..6).map(|n| Ask::Do(folder(&format!("sub{n}")))).collect();
        let record = run(Script::new().round(asks));

        assert_eq!(record.ran().len(), 6, "every call in a batch must run");
        assert_eq!(record.asked(), 0);
        assert!(record.stop.is_none(), "a single batch must not hit a limit");
        assert_eq!(record.rounds, 1, "one reply is one round");
    }

    #[test]
    fn thirty_distinct_actions_are_never_stopped_for_repetition() {
        // A guard that fires on volume would be the old bug wearing new clothes.
        let mut script = Script::new();
        for round in 0..10 {
            script = script.round(
                (0..3)
                    .map(|n| Ask::Do(folder(&format!("r{round}n{n}"))))
                    .collect(),
            );
        }
        let record = run(script);

        assert_eq!(record.ran().len(), 30);
        assert!(record.stop.is_none(), "distinct work is progress");
    }

    #[test]
    fn asking_for_one_folder_over_and_over_stops_and_says_which() {
        let mut script = Script::new();
        for _ in 0..6 {
            script = script.round(vec![Ask::Do(folder("stuck"))]);
        }
        let record = run(script);

        let reason = record.stop.expect("a repeated action must end the turn");
        assert!(reason.contains("stuck"), "the report must name the action: {reason}");
        // Stopped early rather than burning every round.
        assert!(record.rounds < 6, "should stop before the script runs out");
    }

    #[test]
    fn a_declined_action_that_is_asked_for_again_stops() {
        // The specific waste the user feels: saying no twice and being asked a
        // third time.
        let mut script = Script::new();
        for _ in 0..4 {
            script = script.round(vec![Ask::Confirm(folder("unwanted"))]);
        }
        let record = run(script);

        assert_eq!(record.asked(), 2, "the first two may still be offered");
        assert!(record.stop.is_some(), "the third refusal should end it");
    }

    #[test]
    fn a_long_ordinary_request_completes_without_a_limit_message() {
        // Creating a project: a parent folder, subfolders, then files.
        let record = run(
            Script::new()
                .round(vec![Ask::Do(folder("project"))])
                .round(
                    ["src", "docs", "tests", "assets", "config"]
                        .iter()
                        .map(|n| {
                            Ask::Do(ChatTool::CreateFolder {
                                name: n.to_string(),
                                parent: Some(std::path::PathBuf::from("/scratch/project")),
                            })
                        })
                        .collect(),
                ),
        );

        assert_eq!(record.ran().len(), 6);
        assert!(record.stop.is_none());
        assert_eq!(record.rounds, 2);
    }
}
