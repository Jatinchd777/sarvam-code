use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, AssistantItem, Item, NoticeKind, PermissionModal, ToolItem, ToolStatus};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    if area.width < 24 || area.height < 8 {
        frame.render_widget(
            Paragraph::new("terminal too small").alignment(Alignment::Center),
            area,
        );
        return;
    }

    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .split(area);

    draw_header(frame, app, chunks[0]);
    draw_chat(frame, app, chunks[1]);
    draw_input(frame, app, chunks[2]);
    draw_footer(frame, app, chunks[3]);

    if let Some(modal) = app.permission.clone() {
        draw_permission(frame, &modal);
    }
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let mode_style = match app.mode {
        crate::config::PermissionMode::Ask => Style::default().fg(Color::Yellow),
        crate::config::PermissionMode::ReadOnly => Style::default().fg(Color::Blue),
        crate::config::PermissionMode::Yolo => Style::default().fg(Color::Red),
    }
    .add_modifier(Modifier::BOLD);

    let line = Line::from(vec![
        Span::styled(
            " sarvam-code ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            app.model.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(app.mode.to_string(), mode_style),
        Span::raw("  "),
        Span::styled(
            shorten_path(&app.workspace, 60),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    frame.render_widget(Paragraph::new(line), area);

    if app.busy {
        let line = Line::from(vec![
            Span::styled(
                format!("{} ", spinner_glyph(app.spinner)),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled("working", Style::default().fg(Color::Yellow)),
            Span::raw(" "),
        ]);
        frame.render_widget(Paragraph::new(line).alignment(Alignment::Right), area);
    }
}

fn draw_chat(frame: &mut Frame, app: &mut App, area: Rect) {
    let mut lines = if app.items.is_empty() {
        welcome()
    } else {
        build_chat(app, area.width as usize)
    };

    let height = area.height as usize;
    app.viewport_height = height;
    let total = lines.len();
    // While the reader is scrolled up, keep the same content in view as new
    // output arrives instead of sliding the window out from under them.
    if app.scroll > 0 && total > app.last_total_lines {
        app.scroll = app.scroll.saturating_add(total - app.last_total_lines);
    }
    app.last_total_lines = total;
    let max_scroll = total.saturating_sub(height);
    app.scroll = app.scroll.min(max_scroll);

    let end = total.saturating_sub(app.scroll);
    let start = end.saturating_sub(height);
    let window: Vec<Line<'static>> = lines.drain(start..end).collect();

    frame.render_widget(Paragraph::new(window), area);
}

fn welcome() -> Vec<Line<'static>> {
    vec![
        Line::default(),
        Line::from(Span::styled(
            "  sarvam-code",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "  a minimal Sarvam AI coding agent",
            Style::default().fg(Color::DarkGray),
        )),
        Line::default(),
        Line::from(Span::styled(
            "  describe a task, or type /help",
            Style::default().fg(Color::Gray),
        )),
    ]
}

fn build_chat(app: &App, width: usize) -> Vec<Line<'static>> {
    let mut blocks: Vec<Vec<Line<'static>>> = Vec::new();
    for item in &app.items {
        let block = match item {
            Item::User(text) => block_text("you", Color::Cyan, text, width),
            Item::Assistant(assistant) => block_assistant(assistant, width, app.spinner),
            Item::Tool(tool) => block_tool(tool, width, app.spinner, app.expand_tools),
            Item::Notice(kind, text) => block_notice(*kind, text, width),
        };
        if !block.is_empty() {
            blocks.push(block);
        }
    }

    let mut lines = Vec::new();
    for (index, block) in blocks.into_iter().enumerate() {
        if index > 0 {
            lines.push(Line::default());
        }
        lines.extend(block);
    }
    lines
}

fn block_text(role: &str, color: Color, text: &str, width: usize) -> Vec<Line<'static>> {
    let mut out = vec![role_line(role, color, None)];
    push_markdown(&mut out, text, width.saturating_sub(3).max(10));
    out
}

fn block_assistant(assistant: &AssistantItem, width: usize, spinner: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let usable = width.saturating_sub(3).max(10);
    let spinner = assistant
        .streaming
        .then(|| format!("  {}", spinner_glyph(spinner)));

    if !assistant.reasoning.is_empty() {
        out.push(role_line("thinking", Color::DarkGray, spinner.clone()));
        push_wrapped(
            &mut out,
            &assistant.reasoning,
            usable,
            "  ",
            Style::default().fg(Color::DarkGray),
        );
    }

    if !assistant.text.is_empty() {
        out.push(role_line("sarvam", Color::Magenta, spinner));
        push_markdown(&mut out, &assistant.text, usable);
    }

    out
}

fn block_tool(tool: &ToolItem, width: usize, spinner: usize, expanded: bool) -> Vec<Line<'static>> {
    let (icon, color) = match tool.status {
        ToolStatus::Running => (spinner_glyph(spinner).to_string(), Color::Yellow),
        ToolStatus::Ok => ("✓".to_string(), Color::Green),
        ToolStatus::Error => ("✗".to_string(), Color::Red),
    };

    let mut header = vec![
        Span::styled(format!("{icon} "), Style::default().fg(color)),
        Span::styled(
            tool.name.clone(),
            Style::default()
                .fg(Color::LightBlue)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    let summary = summarize_args(&tool.name, &tool.args);
    if !summary.is_empty() {
        header.push(Span::styled(
            format!("  {summary}"),
            Style::default().fg(Color::DarkGray),
        ));
    }

    let mut out = vec![Line::from(header)];
    if tool.output.is_empty() {
        return out;
    }

    let lines: Vec<&str> = tool.output.trim_end_matches('\n').split('\n').collect();
    let limit = if expanded { 80 } else { 4 };
    let style = if tool.status == ToolStatus::Error {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::Gray)
    };

    for line in lines.iter().take(limit) {
        out.push(Line::from(vec![
            Span::styled("  │ ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                clip_cells(&line.replace('\t', "    "), width.saturating_sub(6).max(10)),
                style,
            ),
        ]));
    }
    if lines.len() > limit {
        out.push(Line::from(vec![Span::styled(
            format!(
                "  │ … {} more lines (ctrl+o)",
                lines.len().saturating_sub(limit)
            ),
            Style::default().fg(Color::DarkGray),
        )]));
    }
    out
}

fn block_notice(kind: NoticeKind, text: &str, width: usize) -> Vec<Line<'static>> {
    let text = sanitize(text);
    let (prefix, color) = match kind {
        NoticeKind::Info => ("·", Color::DarkGray),
        NoticeKind::Warn => ("!", Color::Yellow),
        NoticeKind::Error => ("!", Color::Red),
    };
    let style = Style::default().fg(color);
    let usable = width.saturating_sub(2).max(10);
    let options = textwrap::Options::new(usable).break_words(true);

    let mut out = Vec::new();
    let mut first = true;
    for raw in text.split('\n') {
        if raw.trim().is_empty() {
            out.push(Line::default());
            continue;
        }
        for chunk in textwrap::wrap(raw, options.clone()) {
            let lead = if first {
                first = false;
                format!("{prefix} ")
            } else {
                "  ".to_string()
            };
            out.push(Line::from(vec![
                Span::styled(lead, style),
                Span::styled(chunk.into_owned(), style),
            ]));
        }
    }
    out
}

fn role_line(role: &str, color: Color, suffix: Option<String>) -> Line<'static> {
    let mut spans = vec![
        Span::styled("▌ ", Style::default().fg(color)),
        Span::styled(
            role.to_string(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(suffix) = suffix {
        spans.push(Span::styled(suffix, Style::default().fg(Color::DarkGray)));
    }
    Line::from(spans)
}

fn push_markdown(out: &mut Vec<Line<'static>>, text: &str, width: usize) {
    let text = sanitize(text);
    let mut in_code = false;
    let mut raw_lines: Vec<&str> = text.split('\n').collect();
    while raw_lines.last().is_some_and(|line| line.is_empty()) {
        raw_lines.pop();
    }

    let options = textwrap::Options::new(width.max(10)).break_words(true);
    for raw in raw_lines {
        let trimmed = raw.trim_start();
        if trimmed.starts_with("```") {
            in_code = !in_code;
            out.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(trimmed.to_string(), Style::default().fg(Color::DarkGray)),
            ]));
            continue;
        }
        if raw.trim().is_empty() {
            out.push(Line::default());
            continue;
        }

        let base = if in_code {
            Style::default().fg(Color::LightGreen)
        } else if trimmed.starts_with('#') {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else if trimmed.starts_with('>') {
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC)
        } else {
            Style::default().fg(Color::White)
        };

        let expanded = raw.replace('\t', "    ");
        for chunk in textwrap::wrap(&expanded, options.clone()) {
            let chunk = chunk.into_owned();
            let mut spans = vec![Span::raw("  ")];
            if in_code {
                spans.push(Span::styled(chunk, base));
            } else {
                spans.extend(inline_spans(&chunk, base));
            }
            out.push(Line::from(spans));
        }
    }
}

fn inline_spans(text: &str, base: Style) -> Vec<Span<'static>> {
    let code_style = Style::default().fg(Color::Yellow);
    let mut spans = Vec::new();
    for (index, part) in text.split('`').enumerate() {
        if part.is_empty() {
            continue;
        }
        let style = if index % 2 == 1 { code_style } else { base };
        spans.push(Span::styled(part.to_string(), style));
    }
    if spans.is_empty() {
        spans.push(Span::raw(""));
    }
    spans
}

fn push_wrapped(
    out: &mut Vec<Line<'static>>,
    text: &str,
    width: usize,
    indent: &str,
    style: Style,
) {
    let text = sanitize(text);
    let mut raw_lines: Vec<&str> = text.split('\n').collect();
    while raw_lines.last().is_some_and(|line| line.is_empty()) {
        raw_lines.pop();
    }
    let options = textwrap::Options::new(width.max(10)).break_words(true);
    for raw in raw_lines {
        if raw.trim().is_empty() {
            out.push(Line::default());
            continue;
        }
        let expanded = raw.replace('\t', "    ");
        for chunk in textwrap::wrap(&expanded, options.clone()) {
            out.push(Line::from(vec![
                Span::raw(indent.to_string()),
                Span::styled(chunk.into_owned(), style),
            ]));
        }
    }
}

fn draw_input(frame: &mut Frame, app: &App, area: Rect) {
    let focused = !app.busy && app.permission.is_none();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(if focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        })
        .title(Line::from(Span::styled(
            " message ",
            Style::default().fg(Color::DarkGray),
        )));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width < 3 || inner.height == 0 {
        return;
    }

    let text_width = inner.width.saturating_sub(2) as usize;
    let cursor_col = UnicodeWidthStr::width(&app.input.text[..app.input.cursor]);
    let shift = cursor_col.saturating_sub(text_width.saturating_sub(1));
    let visible = slice_from_cells(&app.input.text, shift, text_width);
    let style = if focused {
        Style::default()
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let line = Line::from(vec![
        Span::styled(
            "› ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(visible, style),
    ]);
    frame.render_widget(Paragraph::new(line), inner);

    if focused {
        let x = inner
            .x
            .saturating_add(2)
            .saturating_add((cursor_col - shift) as u16)
            .min(inner.right().saturating_sub(1));
        frame.set_cursor_position(Position::new(x, inner.y));
    }
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let hint = if app.busy {
        "esc cancel · wheel/pgup scroll · ctrl+o tools · ctrl+c quit"
    } else {
        "enter send · wheel/pgup scroll · ctrl+o tools · esc clear · ctrl+c quit"
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {hint}"),
            Style::default().fg(Color::DarkGray),
        ))),
        area,
    );

    let status = if let Some(status) = &app.status {
        Some((status.clone(), Color::Yellow))
    } else if app.scroll > 0 {
        Some((
            format!("{} lines up · pgdn/end to follow", app.scroll),
            Color::DarkGray,
        ))
    } else {
        None
    };

    if let Some((text, color)) = status {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("{text} "),
                Style::default().fg(color),
            )))
            .alignment(Alignment::Right),
            area,
        );
    }
}

fn draw_permission(frame: &mut Frame, modal: &PermissionModal) {
    let area = frame.area();
    let width = ((area.width as u32 * 3) / 4)
        .clamp(30, 100)
        .min(area.width.saturating_sub(2) as u32)
        .max(20) as u16;
    let height = ((area.height as u32 * 3) / 5)
        .clamp(9, 30)
        .min(area.height.saturating_sub(2) as u32)
        .max(7) as u16;
    let rect = Rect::new(
        area.x + (area.width.saturating_sub(width)) / 2,
        area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    );

    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(Color::Yellow))
        .title(Line::from(Span::styled(
            " permission required ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )))
        .title_alignment(Alignment::Center);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let layout = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .split(inner);

    let head = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("tool: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                modal.tool.clone(),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(Span::styled(
            sanitize(&modal.summary),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
    ])
    .wrap(Wrap { trim: true });
    frame.render_widget(head, layout[0]);

    frame.render_widget(
        Paragraph::new(sanitize(&modal.details))
            .style(Style::default().fg(Color::Gray))
            .wrap(Wrap { trim: false }),
        layout[1],
    );

    let hints = Paragraph::new(Line::from(vec![
        Span::styled(
            "[y]",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" allow  "),
        Span::styled(
            "[n]",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" deny  "),
        Span::styled(
            "[a]",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" always  "),
        Span::styled("[esc]", Style::default().fg(Color::DarkGray)),
        Span::raw(" deny"),
    ]))
    .alignment(Alignment::Center)
    .wrap(Wrap { trim: true });
    frame.render_widget(hints, layout[2]);
}

fn spinner_glyph(step: usize) -> &'static str {
    SPINNER[step % SPINNER.len()]
}

fn summarize_args(name: &str, raw: &str) -> String {
    let value: serde_json::Value = match serde_json::from_str(raw) {
        Ok(value) => value,
        Err(_) => return clip_cells(&raw.replace('\n', " "), 100),
    };

    let summary = match name {
        "bash" => value
            .get("command")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string(),
        "read_file" | "list_dir" => value
            .get("path")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(".")
            .to_string(),
        "write_file" => {
            let path = value
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("?");
            let bytes = value
                .get("content")
                .and_then(serde_json::Value::as_str)
                .map(str::len)
                .unwrap_or(0);
            format!("{path} ({bytes} bytes)")
        }
        "edit_file" => {
            let path = value
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("?");
            let all = value
                .get("replace_all")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            format!("{path} ({})", if all { "replace all" } else { "replace" })
        }
        _ => value.to_string(),
    };

    clip_cells(&summary.replace('\n', " "), 100)
}

fn shorten_path(path: &str, max: usize) -> String {
    let mut path = path.to_string();
    if let Some(home) = dirs::home_dir() {
        let home = home.display().to_string();
        if let Some(rest) = path.strip_prefix(&home) {
            path = format!("~{rest}");
        }
    }
    if path.chars().count() <= max {
        return path;
    }
    let tail: String = path
        .chars()
        .rev()
        .take(max.saturating_sub(1))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("…{tail}")
}

fn clip_cells(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for character in text.chars() {
        let character = if character.is_control() {
            ' '
        } else {
            character
        };
        let width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + width > max {
            if !out.is_empty() {
                out.push('…');
            }
            break;
        }
        out.push(character);
        used += width;
    }
    out
}

/// Remove terminal control sequences from model/tool output before rendering.
fn sanitize(text: &str) -> String {
    text.chars()
        .map(|character| {
            if matches!(character, '\n' | '\t') || !character.is_control() {
                character
            } else {
                ' '
            }
        })
        .collect()
}

fn slice_from_cells(text: &str, skip: usize, width: usize) -> String {
    let mut out = String::new();
    let mut column = 0;
    let mut used = 0;

    for character in text.chars() {
        let char_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if column < skip {
            column += char_width;
            continue;
        }
        if used + char_width > width {
            break;
        }
        out.push(character);
        used += char_width;
        column += char_width;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        agent::AgentEvent,
        app::Item,
        config::{Config, PermissionMode},
    };
    use ratatui::{Terminal, backend::TestBackend};
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

    fn rendered(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<Vec<_>>()
            .join("")
    }

    fn screen_rows(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .chunks(width as usize)
            .map(|row| {
                row.iter()
                    .map(|cell| cell.symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    fn chat_rows(app: &mut App) -> Vec<String> {
        let rows = screen_rows(app, 60, 20);
        rows[1..rows.len() - 4].to_vec()
    }

    fn long_conversation() -> App {
        let mut app = App::new(&config());
        for index in 0..60 {
            app.items.push(Item::User(format!("line {index:02}")));
        }
        app
    }

    #[test]
    fn scrolls_the_conversation() {
        let mut app = long_conversation();
        let bottom = chat_rows(&mut app);
        assert!(bottom.iter().any(|row| row.contains("line 59")));
        assert!(!bottom.iter().any(|row| row.contains("line 00")));

        app.scroll_up(30);
        let middle = chat_rows(&mut app);
        assert_ne!(bottom, middle);
        assert!(!middle.iter().any(|row| row.contains("line 59")));

        app.scroll_to_top();
        let top = chat_rows(&mut app);
        assert!(top.iter().any(|row| row.contains("line 00")));

        app.scroll_to_bottom();
        let back = chat_rows(&mut app);
        assert_eq!(bottom, back);
    }

    #[test]
    fn scrolling_keeps_its_place_while_new_output_arrives() {
        let mut app = long_conversation();
        app.scroll_up(30);
        let before = chat_rows(&mut app);
        assert!(!before.iter().any(|row| row.contains("line 59")));

        app.items.push(Item::User("line 60".to_string()));
        let after = chat_rows(&mut app);
        assert_eq!(before, after);
    }

    #[test]
    fn renders_conversation_without_control_characters() {
        let mut app = App::new(&config());
        app.on_agent_event(AgentEvent::TurnStarted);
        app.on_agent_event(AgentEvent::TextDelta(
            "hello **world**\n\n```rust\nfn main() {}\n```".to_string(),
        ));
        app.on_agent_event(AgentEvent::ToolStarted {
            id: "call_1".to_string(),
            name: "bash".to_string(),
            args: r#"{"command":"echo hello"}"#.to_string(),
        });
        app.on_agent_event(AgentEvent::ToolFinished {
            id: "call_1".to_string(),
            ok: true,
            output: "hello\n\u{1b}[31mnot-red".to_string(),
        });
        app.on_agent_event(AgentEvent::Notice("a notice".to_string()));
        app.input.insert_str("next message");

        let screen = rendered(&mut app, 100, 30);
        assert!(screen.contains("sarvam-code"));
        assert!(screen.contains("bash"));
        assert!(screen.contains("hello"));
        assert!(!screen.contains('\u{1b}'));
    }

    #[test]
    fn renders_permission_modal() {
        let mut app = App::new(&config());
        app.on_agent_event(AgentEvent::PermissionRequest {
            id: 1,
            tool: "bash".to_string(),
            summary: "bash → echo hello".to_string(),
            details: "$ echo hello".to_string(),
        });
        let screen = rendered(&mut app, 100, 30);
        assert!(screen.contains("permission required"));
        assert!(screen.contains("echo hello"));
    }

    #[test]
    fn tiny_terminal_does_not_panic() {
        let mut app = App::new(&config());
        let screen = rendered(&mut app, 16, 6);
        assert!(screen.contains("terminal"));
    }
}
