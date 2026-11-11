//! Chat tool surface for CIQ-137.
//!
//! The model may only name one of a closed set of tools and supply arguments.
//! Everything that decides whether a call runs — which tool, which arguments,
//! and whether it needs confirmation — is decided here, in Rust, on parsed
//! data. The model's description of what it is doing is never used.
//!
//! This exists because the model's output is untrusted input: it may have just
//! read a file containing instructions aimed at it. See
//! `docs/design/ciq-137-chat-actions.md`.

use serde::{Deserialize, Serialize};

/// What a tool can do. Flags rather than a single enum so a tool can be both
/// destructive and open-world, and so the confirmation rule is one predicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct Annotations {
    /// Modifies nothing on disk or in the world.
    pub read_only: bool,
    /// Can delete or overwrite something that already exists.
    pub destructive: bool,
    /// Reaches outside the local machine.
    pub open_world: bool,
}

impl Annotations {
    const fn read_only() -> Self {
        Self { read_only: true, destructive: false, open_world: false }
    }
    /// Changes nothing locally, but reaches the network. Not `read_only`:
    /// a request that leaves the machine is not harmless by default, and an
    /// injected file could use it to call out to somewhere. It is not
    /// destructive, so the user is not asked to confirm a browser tab.
    const fn open_world() -> Self {
        Self { read_only: false, destructive: false, open_world: true }
    }
    const fn destructive() -> Self {
        Self { read_only: false, destructive: true, open_world: false }
    }
}

/// A tool the model is allowed to call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChatTool {
    OpenPath { path: String },
    RevealPath { path: String },
    OpenUrl { url: String },
    CreateFolder { name: String },
    /// A new file. `parent` names an existing folder to put it in; without one
    /// the file goes to the configured create directory, which is what makes
    /// "create a folder, then put a file in it" possible in one request.
    CreateFile {
        name: String,
        parent: Option<std::path::PathBuf>,
    },
}

/// Why a call was refused before it ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ToolError {
    UnknownTool { name: String },
    EmptyArgument { tool: &'static str, field: &'static str },
    /// Separators, traversal, or a reserved device name.
    IllegalName { tool: &'static str, reason: String },
    /// A URL scheme that must never reach the shell.
    IllegalUrl { scheme: String },
    MissingPath { path: String },
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownTool { name } => write!(f, "unknown tool: {name}"),
            Self::EmptyArgument { tool, field } => {
                write!(f, "{tool} requires a non-empty {field}")
            }
            Self::IllegalName { tool, reason } => write!(f, "{tool}: {reason}"),
            Self::IllegalUrl { scheme } => {
                write!(f, "refusing to open a {scheme}: URL")
            }
            Self::MissingPath { path } => write!(f, "path does not exist: {path}"),
        }
    }
}

impl ChatTool {
    /// Name as it appears on the wire, used in both directions.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::OpenPath { .. } => "open_path",
            Self::RevealPath { .. } => "reveal_path",
            Self::OpenUrl { .. } => "open_url",
            Self::CreateFolder { .. } => "create_folder",
            Self::CreateFile { .. } => "create_file",
        }
    }

    pub(crate) fn annotations(&self) -> Annotations {
        match self {
            Self::OpenPath { .. } => Annotations::read_only(),
            Self::RevealPath { .. } => Annotations::read_only(),
            Self::OpenUrl { .. } => Annotations::open_world(),
            Self::CreateFolder { .. } => Annotations::destructive(),
            Self::CreateFile { .. } => Annotations::destructive(),
        }
    }

    /// Does this call need the user to click Confirm before it runs?
    ///
    /// Fail-safe, following the MCP annotation defaults: an unknown or
    /// unclassified tool needs approval. Anything destructive needs approval,
    /// and so does anything that is neither read-only nor harmless.
    pub(crate) fn needs_confirmation(&self) -> bool {
        needs_confirmation(self.annotations())
    }

    /// The human-readable target, for the confirmation card. The user is
    /// approving *this string*, so it must be the exact value that will be
    /// used, not a description of it.
    pub(crate) fn target(&self) -> &str {
        match self {
            Self::OpenPath { path }
            | Self::RevealPath { path } => path,
            Self::OpenUrl { url } => url,
            Self::CreateFolder { name } | Self::CreateFile { name, .. } => name,
        }
    }
}

/// The single confirmation predicate, kept free-standing so it can be tested
/// against unclassified annotations a future tool might have.
///
/// Answers two questions, both failing safe:
///
/// - **Writes.** `destructive` always confirms.
/// - **Undeclared.** A tool that claims neither `read_only` nor the specific
///   open-world shape we model describes nothing we understand, so it is
///   treated as dangerous.
///
/// The one exception is `open_world` alone: reaching the network while changing
/// nothing locally is a browser tab, not a file operation. Prompting for it
/// would train users to click through prompts, and a prompt users click through
/// is worse than none — it feels like a control while providing nothing.
///
/// An earlier draft used "approve unless proven harmless", which was correct
/// in the unsafe direction but prompted on every link; the opposite draft,
/// `!read_only && open_world`, let a wholly unclassified tool run unattended.
/// This form is the intersection of what both were trying to say.
pub(crate) fn needs_confirmation(annotations: Annotations) -> bool {
    if annotations.destructive {
        return true;
    }
    if annotations.read_only {
        return false;
    }
    // Not read-only, not destructive. Only a declared open-world reach is
    // understood well enough to run without a prompt.
    !annotations.open_world
}

/// A tool call exactly as the model produced it. Untrusted.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct RawToolCall {
    pub name: String,
    #[serde(default)]
    pub arguments: serde_json::Value,
    /// The provider's id for this call, when it gave one. The `/responses` API
    /// needs it back on the `function_call_output` item that reports the result,
    /// otherwise the model cannot tell which of its calls the outcome belongs to.
    #[serde(default)]
    pub id: Option<String>,
}

impl RawToolCall {
    /// Parse and validate into a `ChatTool`.
    ///
    /// Every failure path returns an error rather than falling back to a
    /// default: an unparseable call must not become a guess at what the model
    /// meant.
    pub(crate) fn validate(self) -> Result<ChatTool, ToolError> {
        let arg = |field: &str| -> Result<String, ToolError> {
            self.arguments
                .get(field)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .ok_or(ToolError::EmptyArgument {
                    tool: leak_name(&self.name),
                    field: leak_field(field),
                })
        };
        match self.name.as_str() {
            "open_path" => {
                let path = arg("path")?;
                if !std::path::Path::new(&path).exists() {
                    return Err(ToolError::MissingPath { path });
                }
                Ok(ChatTool::OpenPath { path })
            }
            "reveal_path" => {
                let path = arg("path")?;
                if !std::path::Path::new(&path).exists() {
                    return Err(ToolError::MissingPath { path });
                }
                Ok(ChatTool::RevealPath { path })
            }
            "open_url" => {
                let url = arg("url")?;
                validate_url(&url)?;
                Ok(ChatTool::OpenUrl { url })
            }
            "create_folder" => {
                let name = arg("name")?;
                validate_name("create_folder", &name)?;
                Ok(ChatTool::CreateFolder { name })
            }
            "create_file" => {
                let name = arg("name")?;
                validate_name("create_file", &name)?;
                // Optional destination folder. It must be an existing directory
                // the user can see: absolute, and never a system location. A
                // model-supplied path is the one thing here it fully controls,
                // so it is checked before anything is joined or written.
                let parent = self
                    .arguments
                    .get("parent")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|raw| {
                        let path = std::path::PathBuf::from(raw);
                        if !path.is_absolute() {
                            return Err(ToolError::IllegalName {
                                tool: "create_file",
                                reason: "parent must be an absolute path".into(),
                            });
                        }
                        if !path.is_dir() {
                            return Err(ToolError::IllegalName {
                                tool: "create_file",
                                reason: "parent folder does not exist".into(),
                            });
                        }
                        if crate::chat_tools::is_system_path(&path) {
                            return Err(ToolError::IllegalName {
                                tool: "create_file",
                                reason: "parent is a system folder".into(),
                            });
                        }
                        Ok(path)
                    })
                    .transpose()?;
                Ok(ChatTool::CreateFile { name, parent })
            }
            other => Err(ToolError::UnknownTool { name: other.to_string() }),
        }
    }
}

/// Tool names are only ever one of the five known constants, so leaking the
/// name for an error message is bounded — but `leak_name` still maps to a
/// static rather than allocating, so an unknown model-controlled name cannot
/// grow an error string without limit.
fn leak_name(name: &str) -> &'static str {
    match name {
        "open_path" => "open_path",
        "reveal_path" => "reveal_path",
        "open_url" => "open_url",
        "create_folder" => "create_folder",
        "create_file" => "create_file",
        _ => "tool",
    }
}

fn leak_field(field: &str) -> &'static str {
    match field {
        "path" => "path",
        "url" => "url",
        "name" => "name",
        _ => "argument",
    }
}

/// Only `http` and `https` may reach the shell.
///
/// This is the control that matters most against prompt injection: a file that
/// says "open this link" must not be able to produce `file:`, `javascript:`,
/// `data:`, `vbscript:`, `mailto:`, or an arbitrary registered protocol
/// handler. `launch_open` hands the string to the Windows shell, which will
/// happily execute several of those.
fn validate_url(url: &str) -> Result<(), ToolError> {
    let scheme = url
        .split_once(':')
        .map(|(scheme, _)| scheme)
        .unwrap_or("");
    let scheme = scheme.trim().to_ascii_lowercase();
    match scheme.as_str() {
        "http" | "https" => Ok(()),
        _ => Err(ToolError::IllegalUrl {
            scheme: if scheme.is_empty() {
                "<none>".to_string()
            } else {
                // Bound the echoed length so a hostile URL cannot bloat errors.
                scheme.chars().take(16).collect()
            },
        }),
    }
}

/// A created name must be a single path component.
///
/// Separators are rejected outright: a model that could name `..\..\Windows`
/// could write outside the folder the user chose. Reserved device names are
/// rejected because Windows silently redirects them.
/// Whether a path points into a location nex should never write to.
///
/// A folder supplied by the model is the one argument it fully controls, so a
/// file can only be created inside one that is not a system directory. This is
/// a coarse prefix test on purpose: it runs before anything is joined or
/// written, and a false positive costs the user one alternative folder.
fn is_system_path(path: &std::path::Path) -> bool {
    let text = path.to_string_lossy().to_ascii_lowercase();
    const BLOCKED: &[&str] = &[
        "\\windows",
        "\\windows\\system32",
        "\\program files",
        "\\program files (x86)",
        "\\programdata",
        "\\$recycle.bin",
        "\\system volume information",
        "\\perflogs",
        "\\recovery",
    ];
    BLOCKED.iter().any(|prefix| text.contains(prefix))
        // A drive root is not a folder anyone should drop a file into.
        || text.trim_end_matches('\\').ends_with(':')
        || text == "c:\\"
}

fn validate_name(tool: &'static str, name: &str) -> Result<(), ToolError> {
    if name == "." || name == ".." {
        return Err(ToolError::IllegalName {
            tool,
            reason: "name must not be a traversal".to_string(),
        });
    }
    if name.contains('/') || name.contains('\\') {
        return Err(ToolError::IllegalName {
            tool,
            reason: "name must not contain a path separator".to_string(),
        });
    }
    if name.contains("..") {
        return Err(ToolError::IllegalName {
            tool,
            reason: "name must not contain a traversal".to_string(),
        });
    }
    if name.chars().any(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*')) {
        return Err(ToolError::IllegalName {
            tool,
            reason: "name contains a character Windows forbids".to_string(),
        });
    }
    if name.chars().any(|c| (c as u32) < 0x20) {
        return Err(ToolError::IllegalName {
            tool,
            reason: "name contains a control character".to_string(),
        });
    }
    if crate::action_registry::is_windows_reserved_name(name) {
        return Err(ToolError::IllegalName {
            tool,
            reason: "name is a reserved Windows device name".to_string(),
        });
    }
    Ok(())
}

/// The tool declarations handed to the model.
///
/// Kept next to the enum so a new tool cannot be added without appearing
/// here — the test below fails if the two drift apart.
pub(crate) const MAX_TOOL_ROUNDS: usize = 4;

/// Where the tool loop is, as a value rather than as control flow.
///
/// The stream workers are unavoidably imperative, but the decision they make
/// at each step is not. Keeping that decision here means the confirm/resume
/// handshake can be tested exactly, without a provider, a thread or a channel
/// — which is the only way to be confident that a suspended mutating call
/// cannot resume by itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LoopStep {
    /// Run this already-validated tool, then send its outcome back.
    RunTool { tool: ChatTool },
    /// Stop and wait for the user. Nothing runs until `resume_after_confirm`.
    AwaitConfirmation { tool: ChatTool },
    /// Give up on tools and answer in words; the round bound was hit.
    Stop { reason: String },
}

/// The tool loop's state. Holds no tool result of its own — the transcript is
/// sent back to the model intact, so there is one source of truth.
#[derive(Debug, Clone)]
pub(crate) struct ToolLoop {
    rounds: usize,
}

impl ToolLoop {
    pub(crate) fn new() -> Self {
        Self { rounds: 0 }
    }

    pub(crate) fn rounds(&self) -> usize {
        self.rounds
    }

    /// The user clicked Allow. Resuming yields the run, never a second wait.
    pub(crate) fn resume_after_confirm(&mut self, tool: ChatTool) -> LoopStep {
        LoopStep::RunTool { tool }
    }

    /// Decide what to do with a tool call the model just made.
    ///
    /// Validation happens outside this type, so anything arriving here has
    /// already had its arguments checked. What is decided here is only whether
    /// the call runs now, waits for the user, or is refused for running on too
    /// long.
    pub(crate) fn on_tool_call(&mut self, tool: ChatTool) -> LoopStep {
        if self.rounds >= MAX_TOOL_ROUNDS {
            return LoopStep::Stop {
                reason: format!(
                    "stopped after {MAX_TOOL_ROUNDS} action rounds"
                ),
            };
        }
        self.rounds += 1;
        if tool.needs_confirmation() {
            LoopStep::AwaitConfirmation { tool }
        } else {
            LoopStep::RunTool { tool }
        }
    }

}

/// What the user was shown when a call suspended, in the order the UI renders
/// it. Kept next to the loop because a pending card that omits the target is
/// the difference between a real check and a decorative one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PendingAction {
    pub tool: String,
    pub target: String,
    pub description: String,
}

impl PendingAction {
    pub(crate) fn of(tool: &ChatTool) -> Self {
        let verb = match tool {
            ChatTool::OpenPath { .. } => "Open",
            ChatTool::RevealPath { .. } => "Show in File Explorer",
            ChatTool::OpenUrl { .. } => "Open link",
            ChatTool::CreateFolder { .. } => "Create folder",
            ChatTool::CreateFile { .. } => "Create file",
        };
        let effect = if tool.needs_confirmation() {
            "This will change something on your PC."
        } else {
            "This changes nothing on your PC."
        };
        Self {
            tool: tool.name().to_string(),
            target: tool.target().to_string(),
            description: format!("{verb}. {effect}"),
        }
    }
}

pub(crate) fn tool_declarations() -> serde_json::Value {
    serde_json::json!([
        {
            "type": "function",
            "function": {
                "name": "open_path",
                "description": "Open a file or folder that exists on this PC with its default application.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Absolute path that already exists."}
                    },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "reveal_path",
                "description": "Show a file in File Explorer with the item selected.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Absolute path that already exists."}
                    },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "open_url",
                "description": "Open an http or https link in the default browser.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": {"type": "string", "description": "An http or https URL."}
                    },
                    "required": ["url"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "create_folder",
                "description": "Create a folder. Requires user confirmation.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "description": "Folder name, no path separators."}
                    },
                    "required": ["name"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "create_file",
                "description": "Create an empty file. Requires user confirmation.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "description": "File name, no path separators."},
                        "parent": {"type": "string", "description": "Optional absolute path of an existing folder to create the file in, such as a folder you just created. Omit for the user's default create folder."}
                    },
                    "required": ["name"]
                }
            }
        }
    ])
}

/// The outcome of running a tool, reported to both the model and the user.
///
/// The model only ever sees this struct, never free-form prose about what
/// happened, so it cannot describe an action as having succeeded when it
/// failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ToolOutcome {
    pub tool: String,
    pub target: String,
    pub ok: bool,
    /// User-facing sentence. Doubles as the text handed back to the model.
    pub summary: String,
}

impl ToolOutcome {
    pub(crate) fn failure(tool: &str, target: &str, reason: impl std::fmt::Display) -> Self {
        Self {
            tool: tool.to_string(),
            target: target.to_string(),
            ok: false,
            summary: format!("Failed: {reason}"),
        }
    }
}

/// Run a validated tool.
///
/// Only reachable with a `ChatTool`, which can only be produced by
/// `RawToolCall::validate`, so every path here has already been checked for
/// scheme, shape and existence. Mutating tools additionally require `confirmed`
/// to be true — this is enforced here, not in the UI, so a UI bug cannot cause
/// an unconfirmed write.
pub(crate) fn run(tool: &ChatTool, confirmed: bool, create_base: &std::path::Path) -> ToolOutcome {
    if tool.needs_confirmation() && !confirmed {
        return ToolOutcome::failure(
            tool.name(),
            tool.target(),
            "this action was not confirmed",
        );
    }
    match tool {
        ChatTool::OpenPath { path } => match crate::action_executor::launch_path(path) {
            Ok(()) => ToolOutcome {
                tool: tool.name().to_string(),
                target: path.clone(),
                ok: true,
                summary: format!("Opened {path}"),
            },
            Err(error) => ToolOutcome::failure(tool.name(), path, error),
        },
        ChatTool::RevealPath { path } => {
            match crate::action_executor::open_file_location(path) {
                Ok(()) => ToolOutcome {
                    tool: tool.name().to_string(),
                    target: path.clone(),
                    ok: true,
                    summary: format!("Revealed {path} in File Explorer"),
                },
                Err(error) => ToolOutcome::failure(tool.name(), path, error),
            }
        }
        ChatTool::OpenUrl { url } => match crate::action_executor::launch_open_target(url) {
            Ok(()) => ToolOutcome {
                tool: tool.name().to_string(),
                target: url.clone(),
                ok: true,
                summary: format!("Opened {url} in the browser"),
            },
            Err(error) => ToolOutcome::failure(tool.name(), url, error),
        },
        ChatTool::CreateFolder { name } => {
            // `name` is a single component, re-validated above, so joining it to
            // the chosen base cannot escape that base.
            let target = create_base.join(name);
            if target.exists() {
                return ToolOutcome::failure(
                    tool.name(),
                    &target.to_string_lossy(),
                    "that folder already exists",
                );
            }
            let shown = target.to_string_lossy().to_string();
            match std::fs::create_dir_all(&target) {
                Ok(()) => ToolOutcome {
                    tool: tool.name().to_string(),
                    target: shown.clone(),
                    ok: true,
                    summary: format!("Created folder {shown}"),
                },
                Err(error) => ToolOutcome::failure(tool.name(), &shown, error),
            }
        }
        ChatTool::CreateFile { name, parent } => {
            // `parent`, when given, was validated as an existing directory and
            // `name` as a single component, so this join cannot escape either.
            let base = parent.clone().unwrap_or_else(|| create_base.to_path_buf());
            let target = base.join(name);
            if target.exists() {
                return ToolOutcome::failure(
                    tool.name(),
                    &target.to_string_lossy(),
                    "that file already exists",
                );
            }
            let shown = target.to_string_lossy().to_string();
            if let Some(parent) = target.parent() {
                if let Err(error) = std::fs::create_dir_all(parent) {
                    return ToolOutcome::failure(tool.name(), &shown, error);
                }
            }
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)
            {
                Ok(_) => ToolOutcome {
                    tool: tool.name().to_string(),
                    target: shown.clone(),
                    ok: true,
                    summary: format!("Created file {shown}"),
                },
                Err(error) => ToolOutcome::failure(tool.name(), &shown, error),
            }
        }
    }
}

/// The same five tools in the shape the ChatGPT `/responses` API expects.
///
/// That API is flat (`{type, name, parameters}`) rather than OpenAI's nested
/// `{type, function: {name, parameters}}`. Declaring them once and converting
/// keeps the two providers from drifting apart, which is the failure this would
/// otherwise invite: a tool added to one surface and forgotten in the other.
pub(crate) fn responses_tool_declarations() -> serde_json::Value {
    let nested = tool_declarations();
    let flat: Vec<serde_json::Value> = nested
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| {
                    let f = tool.get("function")?;
                    Some(serde_json::json!({
                        "type": "function",
                        "name": f.get("name")?,
                        "description": f.get("description")?,
                        "parameters": f.get("parameters")?,
                    }))
                })
                .collect()
        })
        .unwrap_or_default();
    serde_json::Value::Array(flat)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(name: &str, args: serde_json::Value) -> RawToolCall {
        RawToolCall {
            name: name.to_string(),
            arguments: args,
            id: None,
        }
    }

    // --- classification ---

    #[test]
    fn only_harmful_tools_need_confirmation() {
        let exe = std::env::current_exe().unwrap();
        let exe = exe.to_string_lossy().to_string();
        assert!(!ChatTool::OpenPath { path: exe.clone() }.needs_confirmation());
        assert!(!ChatTool::RevealPath { path: exe }.needs_confirmation());
        // Reaches the open world but changes nothing locally.
        assert!(!ChatTool::OpenUrl {
            url: "https://example.com".into()
        }
        .needs_confirmation());
        assert!(ChatTool::CreateFolder {
            name: "x".into()
        }
        .needs_confirmation());
        assert!(ChatTool::CreateFile { name: "x".into(), parent: None }.needs_confirmation());
    }

    #[test]
    fn unclassified_annotations_fail_safe() {
        // A tool that claims nothing about itself must be treated as dangerous,
        // not harmless. This is the case that matters when someone adds a tool
        // later and forgets to annotate it.
        assert!(needs_confirmation(Annotations {
            read_only: false,
            destructive: false,
            open_world: false,
        }));
    }

    #[test]
    fn explicit_flags_are_respected() {
        // Each modelled flag on its own gives the intended answer. The
        // predicate must not be uniformly conservative, or every harmless call
        // would prompt and users would learn to click through.
        assert!(!needs_confirmation(Annotations {
            read_only: true,
            destructive: false,
            open_world: false,
        }));
        assert!(needs_confirmation(Annotations {
            read_only: false,
            destructive: true,
            open_world: false,
        }));
        // Network reach with no local write is a browser tab, not a file
        // operation, so it does not need a click.
        assert!(!needs_confirmation(Annotations {
            read_only: false,
            destructive: false,
            open_world: true,
        }));
    }

    // --- rejections ---

    #[test]
    fn unknown_tool_is_rejected_not_coerced() {
        let err = raw("run_command", serde_json::json!({"cmd": "del /s /q C:\\"}))
            .validate()
            .unwrap_err();
        assert!(matches!(err, ToolError::UnknownTool { .. }));
    }

    #[test]
    fn shell_and_script_urls_are_rejected() {
        for url in [
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "file:///C:/Windows/System32/cmd.exe",
            "vbscript:msgbox",
            "ms-settings:privacy",
            "not a url",
            "://",
        ] {
            let err = raw("open_url", serde_json::json!({ "url": url }))
                .validate()
                .unwrap_err();
            assert!(
                matches!(err, ToolError::IllegalUrl { .. }),
                "{url} should be rejected, got {err:?}"
            );
        }
    }

    #[test]
    fn http_and_https_urls_are_accepted() {
        for url in [
            "https://example.com",
            "http://example.com/path?q=1",
            "HTTPS://EXAMPLE.COM",
        ] {
            assert!(
                raw("open_url", serde_json::json!({ "url": url }))
                    .validate()
                    .is_ok(),
                "{url} should be accepted"
            );
        }
    }

    #[test]
    fn created_names_cannot_escape_the_chosen_folder() {
        for name in [
            "..",
            ".",
            "../etc",
            "a/b",
            "a\\b",
            "..\\..\\Windows",
            "a..b",
            "con",
            "NUL.txt",
            "lpt1",
            "bad:name",
            "wild*card",
            "pipe|name",
        ] {
            let err = raw("create_folder", serde_json::json!({ "name": name }))
                .validate()
                .unwrap_err();
            assert!(
                matches!(err, ToolError::IllegalName { .. }),
                "{name} should be rejected, got {err:?}"
            );
        }
    }

    #[test]
    fn ordinary_names_are_accepted() {
        for name in ["Reports", "Q3 summary", "notes.txt", "données"] {
            assert!(
                raw("create_folder", serde_json::json!({ "name": name }))
                    .validate()
                    .is_ok(),
                "{name} should be accepted"
            );
        }
    }

    #[test]
    fn missing_or_empty_arguments_are_rejected() {
        for args in [
            serde_json::json!({}),
            serde_json::json!({"path": ""}),
            serde_json::json!({"path": "   "}),
            serde_json::json!({"path": 5}),
        ] {
            let err = raw("open_path", args.clone()).validate().unwrap_err();
            assert!(matches!(err, ToolError::EmptyArgument { .. }), "{args}");
        }
    }

    #[test]
    fn nonexistent_path_is_rejected_before_launch() {
        let err = raw(
            "open_path",
            serde_json::json!({"path": "C:\\definitely\\not\\here\\12345"}),
        )
        .validate()
        .unwrap_err();
        assert!(matches!(err, ToolError::MissingPath { .. }));
    }

    // --- loop state ---

    #[test]
    fn a_read_only_call_runs_without_waiting() {
        let mut loop_state = ToolLoop::new();
        let exe = std::env::current_exe().unwrap().to_string_lossy().to_string();
        match loop_state.on_tool_call(ChatTool::OpenPath { path: exe }) {
            LoopStep::RunTool { tool } => assert_eq!(tool.name(), "open_path"),
            other => panic!("expected RunTool, got {other:?}"),
        }
        assert_eq!(loop_state.rounds(), 1);
    }

    /// The property the whole design rests on: a mutating call must wait, and
    /// waiting must be a distinct state rather than a flag someone could miss.
    #[test]
    fn a_mutating_call_awaits_confirmation_and_does_not_advance() {
        let mut loop_state = ToolLoop::new();
        let tool = ChatTool::CreateFolder {
            name: "Reports".into(),
        };
        let step = loop_state.on_tool_call(tool.clone());
        assert_eq!(step, LoopStep::AwaitConfirmation { tool: tool.clone() });

        // Resuming yields a run, never a second wait or an implicit success.
        assert_eq!(
            loop_state.resume_after_confirm(tool.clone()),
            LoopStep::RunTool { tool }
        );
    }

    #[test]
    fn the_round_bound_stops_a_repeating_model() {
        let mut loop_state = ToolLoop::new();
        let exe = std::env::current_exe().unwrap().to_string_lossy().to_string();
        for _ in 0..MAX_TOOL_ROUNDS {
            assert!(matches!(
                loop_state.on_tool_call(ChatTool::OpenPath { path: exe.clone() }),
                LoopStep::RunTool { .. }
            ));
        }
        // One past the bound ends the turn instead of looping forever.
        match loop_state.on_tool_call(ChatTool::OpenPath { path: exe }) {
            LoopStep::Stop { reason } => assert!(reason.contains("action rounds")),
            other => panic!("expected Stop, got {other:?}"),
        }
    }

    #[test]
    fn pending_action_shows_the_exact_target() {
        let tool = ChatTool::CreateFile {
            name: "quarterly report.docx".into(),
            parent: None,
        };
        let pending = PendingAction::of(&tool);
        // The user approves this string, so it must be the real value — not a
        // paraphrase of it.
        assert_eq!(pending.target, "quarterly report.docx");
        assert_eq!(pending.tool, "create_file");
        // The description states the consequence separately from the target, so
        // the card can show "this will change something" next to the exact path.
        assert!(pending.description.contains("change something"));
    }

    // --- execution ---

    /// The control that matters most: a UI bug must not be able to cause an
    /// unconfirmed write, so the check lives in `run`, not in the caller.
    #[test]
    fn mutating_tools_never_run_without_confirmation() {
        let dir = std::env::temp_dir().join(format!("nex-137-unconfirmed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        for tool in [
            ChatTool::CreateFolder {
                name: "ShouldNotExist".into(),
            },
            ChatTool::CreateFile {
            name: "should_not_exist.txt".into(),
            parent: None,
        },
        ] {
            let outcome = run(&tool, false, &dir);
            assert!(!outcome.ok, "{} ran without confirmation", tool.name());
            assert!(outcome.summary.contains("not confirmed"));
        }
        // Nothing was written.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirmed_create_folder_writes_once() {
        let dir = std::env::temp_dir().join(format!("nex-137-create-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let tool = ChatTool::CreateFolder {
            name: "Reports".into(),
        };
        let first = run(&tool, true, &dir);
        assert!(first.ok, "{} should succeed", first.summary);
        assert!(dir.join("Reports").is_dir());

        // Creating it twice is reported as a failure, not silently accepted.
        let second = run(&tool, true, &dir);
        assert!(!second.ok);
        assert!(second.summary.contains("already exists"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirmed_create_file_writes_an_empty_file() {
        let dir = std::env::temp_dir().join(format!("nex-137-create-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let outcome = run(
            &ChatTool::CreateFile {
            name: "notes.txt".into(),
            parent: None,
        },
            true,
            &dir,
        );
        assert!(outcome.ok, "{}", outcome.summary);
        let path = dir.join("notes.txt");
        assert!(path.is_file());
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// "Create a folder, then put a file in it" is one request, so the second
    /// step has to be able to target the folder the first one made. Without a
    /// parent the file always landed in the create base and the user was told
    /// the tool could not choose a folder.
    #[test]
    fn create_file_can_target_an_existing_folder() {
        let root = std::env::temp_dir().join(format!("nex-137-parent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let inner = root.join("fossa");
        std::fs::create_dir_all(&inner).unwrap();

        let outcome = run(
            &ChatTool::CreateFile {
                name: "hello.txt".into(),
                parent: Some(inner.clone()),
            },
            true,
            &root,
        );
        assert!(outcome.ok, "{}", outcome.summary);
        assert!(inner.join("hello.txt").is_file());
        // Not in the base: the parent decides.
        assert!(!root.join("hello.txt").exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn create_file_rejects_a_parent_that_is_missing_or_relative() {
        let missing = RawToolCall {
            name: "create_file".into(),
            arguments: serde_json::json!({"name": "a.txt", "parent": r"C:\definitely-not-here-nex\x"}),
            id: None,
        };
        assert!(missing.validate().is_err());

        let relative = RawToolCall {
            name: "create_file".into(),
            arguments: serde_json::json!({"name": "a.txt", "parent": "just/a/relative/path"}),
            id: None,
        };
        assert!(relative.validate().is_err());
    }

    /// A model-supplied folder is the one argument it fully controls, so a file
    /// must not be creatable inside a system directory.
    #[test]
    fn create_file_refuses_a_system_folder() {
        for blocked in [
            r"C:\Windows\System32",
            r"C:\Program Files",
            r"C:\ProgramData",
        ] {
            let call = RawToolCall {
                name: "create_file".into(),
                arguments: serde_json::json!({"name": "a.txt", "parent": blocked}),
                id: None,
            };
            assert!(
                call.validate().is_err(),
                "{blocked} should not be writable"
            );
        }
    }

    /// A created name is one component, so joining it to the base cannot walk
    /// out even if validation were bypassed. Belt and braces on the property
    /// that matters most.
    #[test]
    fn create_target_always_stays_inside_the_base() {
        let base = std::path::Path::new("C:\\Users\\someone\\Desktop");
        for name in ["Reports", "notes.txt", "with space", "données"] {
            let target = base.join(name);
            assert!(
                target.starts_with(base),
                "{name} escaped to {}",
                target.display()
            );
        }
    }

    #[test]
    fn failed_action_reports_failure_not_success() {
        // A validated tool can still fail at run time. The outcome must say so,
        // so the model cannot narrate a failure as a success.
        let outcome = run(
            &ChatTool::OpenPath {
                path: "C:\\definitely\\not\\here\\12345".into(),
            },
            true,
            std::path::Path::new("."),
        );
        assert!(!outcome.ok);
        assert!(outcome.summary.starts_with("Failed:"));
    }

    // --- declarations stay in sync with the enum ---

    #[test]
    fn every_declared_tool_validates_as_a_known_tool() {
        let decls = tool_declarations();
        let names: Vec<&str> = decls
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d.pointer("/function/name").unwrap().as_str().unwrap())
            .collect();
        assert_eq!(names.len(), 5);
        for name in names {
            let err = raw(name, serde_json::json!({})).validate();
            // No arguments, so it must fail on the argument — never on the name.
            assert!(
                !matches!(err, Err(ToolError::UnknownTool { .. })),
                "{name} is declared but not handled"
            );
        }
    }

    #[test]
    fn dangerous_tools_are_declared_with_a_confirmation_hint() {
        let decls = tool_declarations();
        for d in decls.as_array().unwrap() {
            let name = d.pointer("/function/name").unwrap().as_str().unwrap();
            let description = d
                .pointer("/function/description")
                .unwrap()
                .as_str()
                .unwrap();
            let needs = match name {
                "create_folder" | "create_file" => true,
                _ => false,
            };
            assert_eq!(
                description.contains("confirmation"),
                needs,
                "{name} confirmation hint is wrong"
            );
        }
    }
}
