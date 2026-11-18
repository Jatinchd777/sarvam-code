use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};

use crate::{
    api::{self, ToolCall},
    config::{Config, PermissionMode},
    tools,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Always,
    Deny,
}

#[derive(Debug)]
pub enum Command {
    Prompt(String),
    Reset,
    SetMode(PermissionMode),
    SetModel(String),
    Shutdown,
}

#[derive(Debug)]
pub enum AgentEvent {
    TurnStarted,
    TextDelta(String),
    ReasoningDelta(String),
    ToolStarted {
        id: String,
        name: String,
        args: String,
    },
    ToolFinished {
        id: String,
        ok: bool,
        output: String,
    },
    PermissionRequest {
        id: u64,
        tool: String,
        summary: String,
        details: String,
    },
    ModeChanged(PermissionMode),
    ModelChanged(String),
    Notice(String),
    Reset,
    TurnFinished,
    TurnFailed(String),
}

#[derive(Clone)]
pub struct Canceller {
    tx: watch::Sender<bool>,
}

impl Canceller {
    pub fn cancel(&self) {
        let _ = self.tx.send(true);
    }
}

pub struct AgentHandle {
    pub commands: mpsc::Sender<Command>,
    pub events: mpsc::UnboundedReceiver<AgentEvent>,
    pub permissions: mpsc::Sender<(u64, Decision)>,
    pub cancel: Canceller,
}

pub fn spawn(cfg: Config, system_prompt: String) -> Result<AgentHandle> {
    let client = reqwest::Client::builder()
        .user_agent(format!("sarvam-code/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .build()
        .context("cannot initialize the HTTP client")?;

    let (cmd_tx, cmd_rx) = mpsc::channel(16);
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let (perm_tx, perm_rx) = mpsc::channel(4);
    let (cancel_tx, cancel_rx) = watch::channel(false);

    let agent = Agent {
        mode: cfg.permission_mode,
        history: vec![json!({"role": "system", "content": system_prompt.clone()})],
        cfg,
        system_prompt,
        client,
        tools: tools::definitions(),
        events: event_tx,
        permissions: perm_rx,
        cancel_rx,
        cancel_tx: cancel_tx.clone(),
        next_request_id: 1,
        save_error_reported: false,
    };
    tokio::spawn(agent.run(cmd_rx));

    Ok(AgentHandle {
        commands: cmd_tx,
        events: event_rx,
        permissions: perm_tx,
        cancel: Canceller { tx: cancel_tx },
    })
}

struct Agent {
    cfg: Config,
    system_prompt: String,
    mode: PermissionMode,
    history: Vec<Value>,
    client: reqwest::Client,
    tools: Value,
    events: mpsc::UnboundedSender<AgentEvent>,
    permissions: mpsc::Receiver<(u64, Decision)>,
    cancel_rx: watch::Receiver<bool>,
    cancel_tx: watch::Sender<bool>,
    next_request_id: u64,
    save_error_reported: bool,
}

enum ToolRun {
    Done { output: String },
    Cancelled,
}

impl Agent {
    async fn run(mut self, mut commands: mpsc::Receiver<Command>) {
        while let Some(command) = commands.recv().await {
            match command {
                Command::Prompt(text) => {
                    let _ = self.cancel_tx.send(false);
                    self.run_turn(&text).await;
                }
                Command::Reset => {
                    let _ = self.cancel_tx.send(false);
                    self.history = vec![json!({
                        "role": "system",
                        "content": self.system_prompt.clone()
                    })];
                    self.save();
                    self.emit(AgentEvent::Reset);
                    self.emit(AgentEvent::Notice("history cleared".to_string()));
                }
                Command::SetMode(mode) => {
                    self.mode = mode;
                    self.emit(AgentEvent::ModeChanged(mode));
                }
                Command::SetModel(model) => {
                    self.cfg.model = model.clone();
                    self.emit(AgentEvent::ModelChanged(model));
                }
                Command::Shutdown => break,
            }
        }
    }

    async fn run_turn(&mut self, input: &str) {
        self.emit(AgentEvent::TurnStarted);
        self.history.push(json!({"role": "user", "content": input}));
        self.save();

        let mut step = 0;
        while step < self.cfg.max_steps {
            step += 1;

            if self.is_cancelled() {
                self.emit(AgentEvent::TurnFailed("cancelled".to_string()));
                return;
            }

            let sender = self.events.clone();
            let mut on_delta = move |delta: api::Delta| {
                let event = match delta {
                    api::Delta::Text(text) => AgentEvent::TextDelta(text),
                    api::Delta::Reasoning(text) => AgentEvent::ReasoningDelta(text),
                };
                let _ = sender.send(event);
            };

            let reply = match api::stream_chat(
                &self.client,
                &self.cfg,
                &self.history,
                &self.tools,
                &mut self.cancel_rx,
                &mut on_delta,
            )
            .await
            {
                Ok(reply) => reply,
                Err(error) => {
                    let message = if self.is_cancelled() {
                        "cancelled".to_string()
                    } else {
                        format!("{error:#}")
                    };
                    self.emit(AgentEvent::TurnFailed(message));
                    return;
                }
            };

            self.append_assistant(&reply);

            if reply.tool_calls.is_empty() {
                if reply.finish_reason == "length" {
                    self.emit(AgentEvent::Notice(
                        "reply hit the token limit; asking the model to continue".to_string(),
                    ));
                    self.history.push(json!({
                        "role": "user",
                        "content": "Your previous reply was cut off by the token limit. Continue from where you stopped."
                    }));
                    self.save();
                    continue;
                }
                if reply.content.trim().is_empty() && reply.reasoning.trim().is_empty() {
                    self.emit(AgentEvent::Notice(
                        "the model returned an empty response".to_string(),
                    ));
                }
                self.emit(AgentEvent::TurnFinished);
                return;
            }

            for index in 0..reply.tool_calls.len() {
                if self.is_cancelled() {
                    self.append_tool_errors(&reply.tool_calls[index..], "error: cancelled by user");
                    self.emit(AgentEvent::TurnFailed("cancelled".to_string()));
                    return;
                }

                let call = reply.tool_calls[index].clone();
                match self.run_tool(&call).await {
                    ToolRun::Done { output } => {
                        self.history.push(json!({
                            "role": "tool",
                            "tool_call_id": call.id,
                            "content": output
                        }));
                        self.save();
                    }
                    ToolRun::Cancelled => {
                        self.append_tool_errors(
                            &reply.tool_calls[index..],
                            "error: cancelled by user",
                        );
                        self.emit(AgentEvent::TurnFailed("cancelled".to_string()));
                        return;
                    }
                }
            }
        }

        self.emit(AgentEvent::Notice(format!(
            "step limit ({}) reached; stopping this turn",
            self.cfg.max_steps
        )));
        self.emit(AgentEvent::TurnFinished);
    }

    async fn run_tool(&mut self, call: &ToolCall) -> ToolRun {
        self.emit(AgentEvent::ToolStarted {
            id: call.id.clone(),
            name: call.name.clone(),
            args: call.arguments.clone(),
        });

        let args: Value = match serde_json::from_str(&call.arguments) {
            Ok(args) => args,
            Err(error) => {
                let output = format!(
                    "error: arguments were not valid JSON ({error}); send the tool call again with valid JSON"
                );
                self.emit(AgentEvent::ToolFinished {
                    id: call.id.clone(),
                    ok: false,
                    output: output.clone(),
                });
                return ToolRun::Done { output };
            }
        };

        if tools::needs_approval(&call.name) {
            let (summary, details) = tools::approval_preview(&call.name, &args);
            match self.ask_permission(&call.name, &summary, &details).await {
                Decision::Allow => {}
                Decision::Always => {
                    self.mode = PermissionMode::Yolo;
                    self.emit(AgentEvent::ModeChanged(self.mode));
                }
                Decision::Deny => {
                    let output = format!("error: user denied {}", call.name);
                    self.emit(AgentEvent::ToolFinished {
                        id: call.id.clone(),
                        ok: false,
                        output: output.clone(),
                    });
                    return ToolRun::Done { output };
                }
            }
        }

        let context = tools::ToolContext {
            workspace: &self.cfg.workspace,
            state_dir: &self.cfg.state_dir,
            bash_timeout: Duration::from_secs(self.cfg.bash_timeout_secs),
        };

        let result = tools::execute(&call.name, &args, &context, &mut self.cancel_rx).await;
        match result {
            Ok(output) => {
                let output = tools::truncate_output(&output, self.cfg.max_output);
                self.emit(AgentEvent::ToolFinished {
                    id: call.id.clone(),
                    ok: true,
                    output: output.clone(),
                });
                ToolRun::Done { output }
            }
            Err(error) if error.is::<tools::Cancelled>() => ToolRun::Cancelled,
            Err(error) => {
                let output =
                    tools::truncate_output(&format!("error: {error:#}"), self.cfg.max_output);
                self.emit(AgentEvent::ToolFinished {
                    id: call.id.clone(),
                    ok: false,
                    output: output.clone(),
                });
                ToolRun::Done { output }
            }
        }
    }

    async fn ask_permission(&mut self, tool: &str, summary: &str, details: &str) -> Decision {
        match self.mode {
            PermissionMode::Yolo => return Decision::Allow,
            PermissionMode::ReadOnly => return Decision::Deny,
            PermissionMode::Ask => {}
        }

        let id = self.next_request_id;
        self.next_request_id += 1;
        self.emit(AgentEvent::PermissionRequest {
            id,
            tool: tool.to_string(),
            summary: summary.to_string(),
            details: tools::truncate_output(details, 6000),
        });

        let permissions = &mut self.permissions;
        let cancel = &mut self.cancel_rx;
        loop {
            tokio::select! {
                received = permissions.recv() => match received {
                    Some((incoming, decision)) if incoming == id => return decision,
                    Some(_) => continue,
                    None => return Decision::Deny,
                },
                _ = wait_cancelled(cancel) => return Decision::Deny,
            }
        }
    }

    fn append_assistant(&mut self, reply: &api::Reply) {
        let mut message = json!({"role": "assistant"});
        message["content"] = if reply.content.is_empty() {
            if reply.tool_calls.is_empty() {
                json!("")
            } else {
                Value::Null
            }
        } else {
            json!(reply.content)
        };
        if !reply.tool_calls.is_empty() {
            let calls: Vec<Value> = reply
                .tool_calls
                .iter()
                .map(|call| {
                    json!({
                        "id": call.id,
                        "type": "function",
                        "function": {
                            "name": call.name,
                            "arguments": call.arguments
                        }
                    })
                })
                .collect();
            message["tool_calls"] = Value::Array(calls);
        }
        self.history.push(message);
        self.save();
    }

    fn append_tool_errors(&mut self, calls: &[ToolCall], message: &str) {
        for call in calls {
            self.history.push(json!({
                "role": "tool",
                "tool_call_id": call.id,
                "content": message
            }));
        }
        self.save();
    }

    fn is_cancelled(&self) -> bool {
        *self.cancel_rx.borrow()
    }

    fn emit(&self, event: AgentEvent) {
        let _ = self.events.send(event);
    }

    fn save(&mut self) {
        if let Err(error) = crate::session::save_history(&self.cfg.transcript_path, &self.history)
            && !self.save_error_reported
        {
            self.save_error_reported = true;
            self.emit(AgentEvent::Notice(format!(
                "cannot save transcript: {error:#}"
            )));
        }
    }
}

async fn wait_cancelled(rx: &mut watch::Receiver<bool>) {
    loop {
        if *rx.borrow() {
            return;
        }
        if rx.changed().await.is_err() {
            return;
        }
    }
}
