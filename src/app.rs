use crate::{
    agent::{AgentEvent, Command},
    config::{Config, PermissionMode},
    tools,
};

/// A slash command as shown in the completions palette and `/help`.
#[derive(Debug, Clone, Copy)]
pub struct SlashCommand {
    pub name: &'static str,
    pub args: &'static str,
    pub description: &'static str,
    pub aliases: &'static [&'static str],
}

impl SlashCommand {
    pub fn label(&self) -> String {
        if self.args.is_empty() {
            self.name.to_string()
        } else {
            format!("{} {}", self.name, self.args)
        }
    }

    pub fn is_named(&self, text: &str) -> bool {
        self.name == text || self.aliases.contains(&text)
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Completion {
    Command(&'static SlashCommand),
    Model(&'static str),
}

impl Completion {
    pub fn label(self) -> String {
        match self {
            Self::Command(command) => command.label(),
            Self::Model(model) => model.to_string(),
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Command(command) => command.description,
            Self::Model(_) => "Sarvam model",
        }
    }

    pub fn is_model(self) -> bool {
        matches!(self, Self::Model(_))
    }

    pub fn is_named(self, text: &str) -> bool {
        match self {
            Self::Command(command) => command.is_named(text),
            Self::Model(_) => false,
        }
    }
}

pub const AVAILABLE_MODELS: &[&str] = &[
    "sarvam-105b",
    "sarvam-105b-conversations",
    "deepseekv4.1-flash-1017",
    "glm5.3",
    "gemma4",
];

pub const COMMANDS: &[SlashCommand] = &[
    SlashCommand {
        name: "/help",
        args: "",
        description: "show commands and key bindings",
        aliases: &[],
    },
    SlashCommand {
        name: "/model",
        args: "<name>",
        description: "switch the model",
        aliases: &[],
    },
    SlashCommand {
        name: "/mode",
        args: "<ask|read-only|yolo>",
        description: "switch permission mode",
        aliases: &[],
    },
    SlashCommand {
        name: "/reset",
        args: "",
        description: "clear conversation and transcript",
        aliases: &[],
    },
    SlashCommand {
        name: "/clear",
        args: "",
        description: "clear the screen only",
        aliases: &[],
    },
    SlashCommand {
        name: "/tools",
        args: "",
        description: "list available tools",
        aliases: &[],
    },
    SlashCommand {
        name: "/transcript",
        args: "",
        description: "show the transcript path",
        aliases: &[],
    },
    SlashCommand {
        name: "/exit",
        args: "",
        description: "quit sarvam-code",
        aliases: &["/quit", "/q"],
    },
];

/// Commands that fuzzy-match the current input. Empty unless the input is a
/// single slash token (no arguments yet).
pub fn command_matches(input: &str) -> Vec<&'static SlashCommand> {
    let Some(query) = input.strip_prefix('/') else {
        return Vec::new();
    };
    if query.chars().any(char::is_whitespace) {
        return Vec::new();
    }

    let mut scored: Vec<(i32, &'static SlashCommand)> = COMMANDS
        .iter()
        .filter_map(|command| score_command(query, command).map(|score| (score, command)))
        .collect();
    // Stable sort so an empty query keeps the registry order and equal scores
    // stay in their declared order.
    scored.sort_by_key(|left| std::cmp::Reverse(left.0));
    scored.into_iter().map(|(_, command)| command).collect()
}

fn model_matches(input: &str) -> Option<Vec<&'static str>> {
    let query = input.strip_prefix("/model ")?;
    let query = query.trim();
    let mut scored: Vec<(i32, &'static str)> = AVAILABLE_MODELS
        .iter()
        .filter_map(|model| score_name(query, model).map(|score| (score, *model)))
        .collect();
    scored.sort_by_key(|left| std::cmp::Reverse(left.0));
    Some(scored.into_iter().map(|(_, model)| model).collect())
}

fn score_command(query: &str, command: &SlashCommand) -> Option<i32> {
    let mut best = score_name(query, command.name.trim_start_matches('/'));
    for alias in command.aliases {
        if let Some(score) = score_name(query, alias.trim_start_matches('/')) {
            let score = score - 20;
            best = Some(best.map_or(score, |current| current.max(score)));
        }
    }
    best
}

fn score_name(query: &str, candidate: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let query = query.to_ascii_lowercase();
    let candidate = candidate.to_ascii_lowercase();

    if candidate == query {
        return Some(1000);
    }
    if candidate.starts_with(&query) {
        return Some(600 - candidate.len() as i32);
    }
    if let Some(position) = candidate.find(&query) {
        return Some(400 - position as i32 - candidate.len() as i32);
    }

    // Subsequence match: /tls -> tools
    let mut remaining = candidate.chars();
    let mut gaps = 0;
    for wanted in query.chars() {
        loop {
            match remaining.next() {
                Some(character) if character == wanted => break,
                Some(_) => gaps += 1,
                None => return None,
            }
        }
    }
    Some(200 - gaps)
}

pub fn help_text() -> String {
    let mut text = String::from("Commands\n");
    for command in COMMANDS {
        text.push_str(&format!(
            "  {:<24} {}\n",
            command.label(),
            command.description
        ));
    }
    text.push_str("\nType / to browse commands with fuzzy matching.\n");
    text.push_str("\nKeys\n");
    text.push_str("  Enter              send\n");
    text.push_str("  Mouse wheel        scroll the conversation\n");
    text.push_str("  PgUp / PgDn        scroll the conversation a page\n");
    text.push_str("  Shift+Up / Down    scroll the conversation three lines\n");
    text.push_str("  Ctrl+Home / End    jump to the top / follow the bottom\n");
    text.push_str("  Ctrl+O             expand or collapse tool output\n");
    text.push_str("  Up / Down          select a slash command suggestion\n");
    text.push_str("  Tab                complete the selected slash command\n");
    text.push_str("  Esc                cancel the current turn, or clear the input\n");
    text.push_str("  Ctrl+C             quit");
    text
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    Running,
    Ok,
    Error,
}

#[derive(Debug)]
pub struct ToolItem {
    pub id: String,
    pub name: String,
    pub args: String,
    pub output: String,
    pub status: ToolStatus,
}

#[derive(Debug)]
pub struct AssistantItem {
    pub reasoning: String,
    pub text: String,
    pub streaming: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum NoticeKind {
    Info,
    Warn,
    Error,
}

#[derive(Debug)]
pub enum Item {
    User(String),
    Assistant(AssistantItem),
    Tool(ToolItem),
    Notice(NoticeKind, String),
}

#[derive(Debug, Clone)]
pub struct PermissionModal {
    pub id: u64,
    pub tool: String,
    pub summary: String,
    pub details: String,
}

pub struct App {
    pub model: String,
    pub mode: PermissionMode,
    pub workspace: String,
    pub transcript: String,
    pub items: Vec<Item>,
    pub input: Input,
    pub busy: bool,
    pub spinner: usize,
    /// Lines scrolled up from the bottom. 0 means follow new output.
    pub scroll: usize,
    /// Last rendered total line count, used to anchor the viewport when
    /// the reader is scrolled up and new output arrives.
    pub last_total_lines: usize,
    pub viewport_height: usize,
    pub expand_tools: bool,
    pub permission: Option<PermissionModal>,
    pub status: Option<String>,
    pub quit: bool,
    /// Highlighted row in the slash-command palette.
    pub completion_index: usize,
}

impl App {
    pub fn new(cfg: &Config) -> Self {
        Self {
            model: cfg.model.clone(),
            mode: cfg.permission_mode,
            workspace: cfg.workspace.display().to_string(),
            transcript: cfg.transcript_path.display().to_string(),
            items: Vec::new(),
            input: Input::new(),
            busy: false,
            spinner: 0,
            scroll: 0,
            last_total_lines: 0,
            viewport_height: 20,
            expand_tools: false,
            permission: None,
            status: None,
            quit: false,
            completion_index: 0,
        }
    }

    pub fn on_agent_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::TurnStarted => {
                self.busy = true;
                self.permission = None;
                self.status = None;
                self.finalize_assistants();
            }
            AgentEvent::TextDelta(text) => self.push_text(text),
            AgentEvent::ReasoningDelta(text) => self.push_reasoning(text),
            AgentEvent::ToolStarted { id, name, args } => {
                self.finalize_assistants();
                self.items.push(Item::Tool(ToolItem {
                    id,
                    name,
                    args,
                    output: String::new(),
                    status: ToolStatus::Running,
                }));
            }
            AgentEvent::ToolFinished { id, ok, output } => {
                let status = if ok {
                    ToolStatus::Ok
                } else {
                    ToolStatus::Error
                };
                let existing = self.items.iter_mut().rev().find_map(|item| match item {
                    Item::Tool(tool) if tool.id == id => Some(tool),
                    _ => None,
                });
                if let Some(tool) = existing {
                    tool.status = status;
                    tool.output = output;
                } else {
                    self.items.push(Item::Tool(ToolItem {
                        id,
                        name: "tool".to_string(),
                        args: String::new(),
                        output,
                        status,
                    }));
                }
            }
            AgentEvent::PermissionRequest {
                id,
                tool,
                summary,
                details,
            } => {
                self.finalize_assistants();
                self.permission = Some(PermissionModal {
                    id,
                    tool,
                    summary,
                    details,
                });
                self.status = Some("waiting for approval".to_string());
            }
            AgentEvent::ModeChanged(mode) => {
                self.mode = mode;
                self.status = Some(format!("permission mode: {mode}"));
            }
            AgentEvent::ModelChanged(model) => {
                self.model = model;
                self.status = Some(format!("model: {}", self.model));
            }
            AgentEvent::Notice(text) => {
                self.finalize_assistants();
                self.notice(NoticeKind::Info, text);
            }
            AgentEvent::Reset => {
                self.items.clear();
                self.busy = false;
                self.permission = None;
                self.scroll = 0;
            }
            AgentEvent::TurnFinished => {
                self.busy = false;
                self.permission = None;
                self.status = None;
                self.finalize_assistants();
            }
            AgentEvent::TurnFailed(reason) => {
                self.busy = false;
                self.permission = None;
                self.status = None;
                self.finalize_assistants();
                for item in &mut self.items {
                    if let Item::Tool(tool) = item
                        && tool.status == ToolStatus::Running
                    {
                        tool.status = ToolStatus::Error;
                        if tool.output.is_empty() {
                            tool.output = "cancelled".to_string();
                        }
                    }
                }
                if reason == "cancelled" {
                    self.notice(NoticeKind::Warn, "turn cancelled".to_string());
                } else {
                    self.notice(NoticeKind::Error, reason);
                }
            }
        }
    }

    /// Handles Enter. Returns a command for the agent, if any.
    pub fn submit(&mut self) -> Option<Command> {
        let raw = self.input.take();
        self.completion_index = 0;
        let text = raw.trim().to_string();
        if text.is_empty() {
            return None;
        }
        if text.starts_with('/') {
            return self.slash_command(&text);
        }
        self.input.remember(&text);
        self.items.push(Item::User(text.clone()));
        self.busy = true;
        self.scroll = 0;
        self.status = None;
        Some(Command::Prompt(text))
    }

    fn slash_command(&mut self, text: &str) -> Option<Command> {
        let mut parts = text.split_whitespace();
        let name = parts.next().unwrap_or("");
        let rest = parts.collect::<Vec<_>>().join(" ");
        match name {
            "/help" | "?" => {
                self.notice(NoticeKind::Info, help_text());
                None
            }
            "/quit" | "/exit" | "/q" => {
                self.quit = true;
                None
            }
            "/reset" => {
                self.items.clear();
                self.scroll = 0;
                self.permission = None;
                self.busy = false;
                Some(Command::Reset)
            }
            "/clear" => {
                self.items.clear();
                self.scroll = 0;
                None
            }
            "/tools" => {
                self.notice(NoticeKind::Info, tools::help_text());
                None
            }
            "/transcript" => {
                self.notice(NoticeKind::Info, format!("transcript: {}", self.transcript));
                None
            }
            "/mode" => match rest.parse::<PermissionMode>() {
                Ok(mode) => {
                    self.mode = mode;
                    self.status = Some(format!("permission mode: {mode}"));
                    Some(Command::SetMode(mode))
                }
                Err(error) => {
                    self.notice(NoticeKind::Error, error.to_string());
                    None
                }
            },
            "/model" => {
                if rest.is_empty() {
                    self.notice(NoticeKind::Error, "usage: /model <name>".to_string());
                    None
                } else {
                    self.model = rest.clone();
                    Some(Command::SetModel(rest))
                }
            }
            other => {
                self.notice(
                    NoticeKind::Error,
                    format!("unknown command: {other} (try /help)"),
                );
                None
            }
        }
    }

    pub fn notice(&mut self, kind: NoticeKind, text: String) {
        self.items.push(Item::Notice(kind, text));
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.notice(NoticeKind::Error, text.into());
    }

    pub fn toggle_expand(&mut self) {
        self.expand_tools = !self.expand_tools;
        self.status = Some(if self.expand_tools {
            "tool output expanded".to_string()
        } else {
            "tool output collapsed".to_string()
        });
    }

    pub fn scroll_up(&mut self, amount: usize) {
        self.scroll = self.scroll.saturating_add(amount);
    }

    pub fn scroll_down(&mut self, amount: usize) {
        self.scroll = self.scroll.saturating_sub(amount);
    }

    pub fn scroll_to_top(&mut self) {
        // Clamped to the real content height on the next draw.
        self.scroll = usize::MAX;
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll = 0;
    }

    pub fn completion_matches(&self) -> Vec<Completion> {
        if let Some(models) = model_matches(&self.input.text) {
            return models.into_iter().map(Completion::Model).collect();
        }
        command_matches(&self.input.text)
            .into_iter()
            .map(Completion::Command)
            .collect()
    }

    pub fn completion_open(&self) -> bool {
        !self.completion_matches().is_empty()
    }

    pub fn selected_completion(&self) -> Option<Completion> {
        let matches = self.completion_matches();
        if matches.is_empty() {
            return None;
        }
        matches
            .get(self.completion_index.min(matches.len() - 1))
            .copied()
    }

    /// Replace the input with the highlighted command or model.
    pub fn complete_selected(&mut self) -> bool {
        let Some(command) = self.selected_completion() else {
            return false;
        };
        match command {
            Completion::Command(command) => self.input.set(format!("{} ", command.name)),
            Completion::Model(model) => self.input.set(format!("/model {model}")),
        }
        self.completion_index = 0;
        true
    }

    pub fn completion_move(&mut self, delta: isize) {
        let count = self.completion_matches().len();
        if count == 0 {
            return;
        }
        let index =
            (self.completion_index.min(count - 1) as isize + delta).rem_euclid(count as isize);
        self.completion_index = index as usize;
    }

    pub fn reset_completion(&mut self) {
        self.completion_index = 0;
    }

    fn push_text(&mut self, text: String) {
        if let Some(Item::Assistant(assistant)) = self.items.last_mut()
            && assistant.streaming
        {
            assistant.text.push_str(&text);
            return;
        }
        self.finalize_assistants();
        self.items.push(Item::Assistant(AssistantItem {
            reasoning: String::new(),
            text,
            streaming: true,
        }));
    }

    fn push_reasoning(&mut self, text: String) {
        if let Some(Item::Assistant(assistant)) = self.items.last_mut()
            && assistant.streaming
        {
            assistant.reasoning.push_str(&text);
            return;
        }
        self.finalize_assistants();
        self.items.push(Item::Assistant(AssistantItem {
            reasoning: text,
            text: String::new(),
            streaming: true,
        }));
    }

    /// Stop every spinner and drop assistant blocks that produced no visible text.
    fn finalize_assistants(&mut self) {
        self.items.retain_mut(|item| {
            if let Item::Assistant(assistant) = item {
                assistant.streaming = false;
                !(assistant.text.trim().is_empty() && assistant.reasoning.trim().is_empty())
            } else {
                true
            }
        });
    }
}

pub struct Input {
    pub text: String,
    /// Byte offset into `text`, always on a char boundary.
    pub cursor: usize,
    history: Vec<String>,
    history_pos: usize,
    draft: String,
}

impl Input {
    fn new() -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            history: Vec::new(),
            history_pos: 0,
            draft: String::new(),
        }
    }

    pub fn insert_char(&mut self, character: char) {
        self.text.insert(self.cursor, character);
        self.cursor += character.len_utf8();
    }

    /// Single-line input: pasted newlines/tabs become spaces.
    pub fn insert_str(&mut self, text: &str) {
        let cleaned: String = text
            .chars()
            .map(|c| {
                if matches!(c, '\n' | '\r' | '\t') || c.is_control() {
                    ' '
                } else {
                    c
                }
            })
            .collect();
        self.text.insert_str(self.cursor, &cleaned);
        self.cursor += cleaned.len();
    }

    pub fn backspace(&mut self) {
        if let Some((index, _)) = self.text[..self.cursor].char_indices().next_back() {
            self.text.remove(index);
            self.cursor = index;
        }
    }

    pub fn delete(&mut self) {
        if self.cursor < self.text.len() {
            self.text.remove(self.cursor);
        }
    }

    pub fn left(&mut self) {
        if let Some((index, _)) = self.text[..self.cursor].char_indices().next_back() {
            self.cursor = index;
        }
    }

    pub fn right(&mut self) {
        if let Some(character) = self.text[self.cursor..].chars().next() {
            self.cursor += character.len_utf8();
        }
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.text.len();
    }

    pub fn kill_word(&mut self) {
        let before = &self.text[..self.cursor];
        let word_end = before.trim_end_matches(char::is_whitespace).len();
        let start = before[..word_end]
            .rfind(char::is_whitespace)
            .map(|index| {
                index
                    + before[index..]
                        .chars()
                        .next()
                        .map(char::len_utf8)
                        .unwrap_or(0)
            })
            .unwrap_or(0);
        self.text.drain(start..self.cursor);
        self.cursor = start;
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.history_pos = self.history.len();
        self.draft.clear();
    }

    pub fn take(&mut self) -> String {
        let text = std::mem::take(&mut self.text);
        self.cursor = 0;
        self.history_pos = self.history.len();
        self.draft.clear();
        text
    }

    pub fn remember(&mut self, text: &str) {
        if self.history.last().is_none_or(|last| last != text) {
            self.history.push(text.to_string());
            if self.history.len() > 100 {
                self.history.remove(0);
            }
        }
        self.history_pos = self.history.len();
    }

    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        if self.history_pos == self.history.len() {
            self.draft = self.text.clone();
        }
        if self.history_pos > 0 {
            self.history_pos -= 1;
            self.text = self.history[self.history_pos].clone();
            self.cursor = self.text.len();
        }
    }

    pub fn history_next(&mut self) {
        if self.history_pos < self.history.len() {
            self.history_pos += 1;
            if self.history_pos == self.history.len() {
                self.text = self.draft.clone();
            } else {
                self.text = self.history[self.history_pos].clone();
            }
            self.cursor = self.text.len();
        }
    }

    /// Replace the input text and put the cursor at the end.
    pub fn set(&mut self, text: String) {
        self.cursor = text.len();
        self.text = text;
        self.history_pos = self.history.len();
        self.draft.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn config() -> Config {
        Config {
            api_key: "test".to_string(),
            api_url: "http://localhost".to_string(),
            model: "test-model".to_string(),
            effort: None,
            permission_mode: PermissionMode::Ask,
            max_steps: 100,
            max_tokens: 1000,
            bash_timeout_secs: 5,
            max_output: 8000,
            http_timeout_secs: 30,
            temperature: 0.2,
            workspace: PathBuf::from("/tmp/workspace"),
            state_dir: PathBuf::from("/tmp/state"),
            transcript_path: PathBuf::from("/tmp/state/history.json"),
        }
    }

    fn names(query: &str) -> Vec<&'static str> {
        command_matches(query)
            .into_iter()
            .map(|command| command.name)
            .collect()
    }

    #[test]
    fn bare_slash_lists_every_command() {
        assert_eq!(
            names("/"),
            COMMANDS.iter().map(|c| c.name).collect::<Vec<_>>()
        );
        assert!(command_matches("").is_empty());
    }

    #[test]
    fn filters_by_prefix() {
        let matches = names("/mo");
        assert_eq!(matches, vec!["/mode", "/model"]);
        assert_eq!(names("/res"), vec!["/reset"]);
        assert!(names("/zzz").is_empty());
    }

    #[test]
    fn fuzzy_matches_subsequences() {
        assert_eq!(names("/tls").first(), Some(&"/tools"));
        assert_eq!(names("/trs").first(), Some(&"/transcript"));
    }

    #[test]
    fn aliases_match_but_display_primary_name() {
        assert_eq!(names("/q"), vec!["/exit"]);
        assert_eq!(names("/quit"), vec!["/exit"]);
    }

    #[test]
    fn arguments_and_plain_text_do_not_open_the_palette() {
        assert!(command_matches("/model some-model").is_empty());
        assert!(command_matches("/mode yolo").is_empty());
        assert!(command_matches("/ ").is_empty());
        assert!(command_matches("hello").is_empty());
        assert!(command_matches(" /help").is_empty());
    }

    #[test]
    fn completion_navigation_cycles_and_completes() {
        let mut app = App::new(&config());
        app.input.set("/mo".to_string());
        assert!(app.completion_open());
        assert!(matches!(
            app.selected_completion(),
            Some(Completion::Command(command)) if command.name == "/mode"
        ));

        app.completion_move(1);
        assert!(matches!(
            app.selected_completion(),
            Some(Completion::Command(command)) if command.name == "/model"
        ));
        app.completion_move(1);
        assert!(matches!(
            app.selected_completion(),
            Some(Completion::Command(command)) if command.name == "/mode"
        ));

        assert!(app.complete_selected());
        assert_eq!(app.input.text, "/mode ");
        app.reset_completion();
        assert_eq!(app.completion_index, 0);
    }

    #[test]
    fn model_palette_lists_and_fuzzy_filters_models() {
        let mut app = App::new(&config());
        app.input.set("/model ".to_string());
        assert_eq!(app.completion_matches().len(), AVAILABLE_MODELS.len());
        assert!(matches!(
            app.selected_completion(),
            Some(Completion::Model("sarvam-105b"))
        ));

        app.input.set("/model s105".to_string());
        assert_eq!(
            app.completion_matches()
                .iter()
                .map(|completion| completion.label())
                .collect::<Vec<_>>(),
            vec!["sarvam-105b", "sarvam-105b-conversations"]
        );
    }

    #[test]
    fn model_completion_can_be_applied() {
        let mut app = App::new(&config());
        app.input.set("/model gem".to_string());
        assert!(app.complete_selected());
        assert_eq!(app.input.text, "/model gemma4");
        assert!(matches!(
            app.submit(),
            Some(Command::SetModel(model)) if model == "gemma4"
        ));
        assert_eq!(app.model, "gemma4");
    }
}
