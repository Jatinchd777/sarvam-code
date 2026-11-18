use crate::{
    agent::{AgentEvent, Command},
    config::{Config, PermissionMode},
    tools,
};

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
                self.notice(NoticeKind::Info, HELP.to_string());
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
}

const HELP: &str = "\
Commands
  /help              show this help
  /model <name>      switch model
  /mode <mode>       ask | read-only | yolo
  /reset             clear the conversation and transcript
  /clear             clear the screen only
  /tools             list available tools
  /transcript        show the transcript path
  /exit, /quit       quit

Keys
  Enter              send
  Mouse wheel        scroll the conversation
  PgUp / PgDn        scroll the conversation a page
  Shift+Up / Down    scroll the conversation three lines
  Ctrl+Home / End    jump to the top / follow the bottom
  Ctrl+O             expand or collapse tool output
  Esc                cancel the current turn, or clear the input
  Ctrl+C             quit";
