use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Wrap,
    },
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    app::{
        App, AssistantItem, Completion, Item, NoticeKind, PermissionModal, ToolItem, ToolStatus,
    },
    markdown::sanitize,
};

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

    let matches = app.completion_matches();
    let palette_height = if matches.is_empty() {
        0
    } else {
        matches.len().min(8) as u16 + 2
    };

    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(palette_height),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .split(area);

    draw_header(frame, app, chunks[0]);
    draw_chat(frame, app, chunks[1]);
    if palette_height > 0 {
        draw_command_palette(frame, app, &matches, chunks[2]);
    }
    draw_input(frame, app, chunks[3]);
    draw_footer(frame, app, chunks[4]);

    if let Some(modal) = app.permission.clone() {
        draw_permission(frame, &modal);
    }
}

/// Slash-command and model suggestions shown directly above the input.
fn draw_command_palette(frame: &mut Frame, app: &App, matches: &[Completion], area: Rect) {
    let title = if matches
        .first()
        .is_some_and(|completion| completion.is_model())
    {
        " models "
    } else {
        " commands "
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Line::from(vec![
            Span::styled(
                title,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "{} match{} ",
                    matches.len(),
                    if matches.len() == 1 { "" } else { "es" }
                ),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let rows = inner.height as usize;
    let selected = app.completion_index.min(matches.len().saturating_sub(1));
    // Keep the highlighted row visible when the list is taller than the box.
    let offset = selected.saturating_sub(rows.saturating_sub(1));

    let label_width = matches
        .iter()
        .map(|completion| UnicodeWidthStr::width(completion.label().as_str()))
        .max()
        .unwrap_or(0)
        .min(inner.width as usize / 2);

    let lines: Vec<Line<'static>> = matches
        .iter()
        .enumerate()
        .skip(offset)
        .take(rows)
        .map(|(index, completion)| {
            let active = index == selected;
            let marker = if active { "› " } else { "  " };
            let label = completion.label();
            let padding = label_width.saturating_sub(UnicodeWidthStr::width(label.as_str()));
            let row = format!(
                "{marker}{label}{}{}",
                " ".repeat(padding),
                if inner.width as usize > label_width + marker.len() + 4 {
                    format!("  {}", completion.description())
                } else {
                    String::new()
                }
            );
            let row = slice_from_cells(&row, 0, inner.width as usize);
            let style = if active {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            Line::from(Span::styled(row, style))
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), inner);
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
    if app.busy {
        let chunks = Layout::horizontal([Constraint::Min(0), Constraint::Length(10)]).split(area);
        frame.render_widget(Paragraph::new(line), chunks[0]);
        let status = Line::from(vec![
            Span::styled(
                format!("{} ", spinner_glyph(app.spinner)),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled("working", Style::default().fg(Color::Yellow)),
            Span::raw(" "),
        ]);
        frame.render_widget(
            Paragraph::new(status).alignment(Alignment::Right),
            chunks[1],
        );
    } else {
        frame.render_widget(Paragraph::new(line), area);
    }
}

fn draw_chat(frame: &mut Frame, app: &mut App, area: Rect) {
    let content_area = Rect {
        width: area.width.saturating_sub(1),
        ..area
    };
    let mut lines = if app.items.is_empty() {
        welcome(
            app,
            content_area.width as usize,
            content_area.height as usize,
        )
    } else {
        build_chat(app, content_area.width as usize)
    };

    let height = content_area.height as usize;
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

    frame.render_widget(Paragraph::new(window), content_area);
    if max_scroll > 0 && area.width > content_area.width {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .thumb_symbol("█")
            .track_symbol(Some("│"));
        let mut state = ScrollbarState::new(total)
            .viewport_content_length(height)
            .position(start);
        frame.render_stateful_widget(
            scrollbar,
            Rect {
                x: content_area.right(),
                width: 1,
                ..area
            },
            &mut state,
        );
    }
}

/// Terminal blue, used for the mark and the brand line.
const ACCENT: Color = Color::LightBlue;

/// The Sarvam rosette: eight interlocking loops around a central diamond.
const LOGO_LARGE: [&str; 12] = [
    "         ▗▄▛▀▙▖         ",
    "    ▄▛▀▜█▜▙▄ ▄██▟█▄▄    ",
    "   ▐▘ ▗▟  ▄▛▜▙  ▜▖ ▝▙   ",
    "  ▄█▀▀▜▀▀▀▘  ▝▀▀▀█▀▜█▖  ",
    "▄▛▘▜▖▄█          █▖ ▟▀▙▖",
    "▜▖ ▟█      ▄▄     ▀█▙  █",
    " ▜▟▘▝█     ▀▀    ▗▞▘▝▙▟▘",
    " ▟▀▙▄▛            ▙▗▄█▌ ",
    " █  ▐▛▀▜▖      ▄█▛▜▛ ▐▌ ",
    " ▝▀▙▟▙▄▄█▄▟▀▜▄▟▌  █▄▄▛▘ ",
    "     ▜▌▀ ▀▙▖▄▞▀▀▀█▘     ",
    "      ▝▙▄█▀▀▀▙▄▄▛       ",
];

const LOGO_SMALL: [&str; 10] = [
    "       ▗▄▀▀▚▖       ",
    "   ▟▀▀█▀▀▄▄▞▀█▀▀▙   ",
    "  ▐▙▄▟█▄▟▘▝▚▄▟▙▄▟▌  ",
    "▗▞▜▌▗▟        █ ▝▛▄ ",
    "▜ ▗█▛    ▄▄    ▀▟▌ █",
    "▝▙▛ ▜    ▀▀   ▗▛ ▜▟▘",
    " ▛▝▚▙▄         █▄▀▙ ",
    " ▜▄ ▌ ▜▖▗▄▄ ▟▀▀█ ▄▛ ",
    "   ▀▜▛▀▀▙  █▀▀█▀▀▘  ",
    "     ▀▄▄▞▀▜▄▄▟▘     ",
];

const TOOL_NAMES: [&str; 5] = ["list_dir", "read_file", "edit_file", "write_file", "bash"];

/// Landing screen shown before the first message: the Sarvam mark, a short
/// greeting and the live session configuration. The mark is dropped when the
/// viewport is too small for it to breathe.
fn welcome(app: &App, width: usize, height: usize) -> Vec<Line<'static>> {
    let large = width >= 40 && height >= 22;
    let small = !large && width >= 28 && height >= 19;

    let mut lines = Vec::new();
    if large {
        lines.extend(logo_lines(&LOGO_LARGE, width));
        lines.push(Line::default());
    } else if small {
        lines.extend(logo_lines(&LOGO_SMALL, width));
        lines.push(Line::default());
    }

    if height >= 5 {
        if width >= 26 {
            lines.push(centered(
                vec![
                    Span::styled("Welcome to ", Style::default().fg(Color::Gray)),
                    Span::styled(
                        "sarvam-code",
                        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                    ),
                ],
                width,
            ));
        } else {
            lines.push(centered_text("sarvam-code", ACCENT, width));
        }
        if height >= 12 && width >= 36 {
            lines.push(centered_text(
                "a minimal Sarvam AI coding agent",
                Color::DarkGray,
                width,
            ));
        }
        lines.push(Line::default());
    }

    if height >= 9 {
        lines.extend(session_specs(app, width, height >= 20));
        lines.push(Line::default());
    }

    let tip = if width >= 52 {
        "ask a question, or type / for commands"
    } else if width >= 32 {
        "ask a question, or / for commands"
    } else {
        "/ for commands"
    };
    lines.push(centered_text(tip, Color::DarkGray, width));

    if lines.len() < height {
        let top = ((height - lines.len()) / 2).min(6);
        if top > 0 {
            lines.splice(0..0, std::iter::repeat_n(Line::default(), top));
        }
    }
    lines.truncate(height);
    lines
}

fn logo_lines(art: &'static [&'static str], width: usize) -> Vec<Line<'static>> {
    art.iter()
        .map(|row| centered(vec![Span::styled(*row, Style::default().fg(ACCENT))], width))
        .collect()
}

/// A quiet, borderless list of the live session configuration. The block is
/// centered as a whole so labels and values stay aligned.
fn session_specs(app: &App, width: usize, include_tools: bool) -> Vec<Line<'static>> {
    let value_width = width
        .saturating_sub(13)
        .clamp(11, 56)
        .min((width * 3 / 5).max(18));
    let mode_style = match app.mode {
        crate::config::PermissionMode::Ask => Style::default().fg(ACCENT),
        crate::config::PermissionMode::ReadOnly => Style::default().fg(Color::LightBlue),
        crate::config::PermissionMode::Yolo => Style::default().fg(Color::LightRed),
    };

    let mut rows: Vec<(&str, String, Style)> = vec![
        (
            "model",
            app.model.clone(),
            Style::default().fg(Color::White),
        ),
        ("mode", app.mode.to_string(), mode_style),
        (
            "workspace",
            shorten_path(&app.workspace, value_width),
            Style::default().fg(Color::Gray),
        ),
    ];

    if include_tools {
        let tools = TOOL_NAMES.join(" · ");
        if UnicodeWidthStr::width(tools.as_str()) <= value_width {
            rows.push(("tools", tools, Style::default().fg(Color::DarkGray)));
        }
    }

    for (_, value, _) in &mut rows {
        let clipped = clip_cells(value.as_str(), value_width);
        *value = clipped;
    }

    let block_width = 9
        + 2
        + rows
            .iter()
            .map(|(_, value, _)| UnicodeWidthStr::width(value.as_str()))
            .max()
            .unwrap_or(0);
    let left = width.saturating_sub(block_width) / 2;
    let label_style = Style::default().fg(Color::DarkGray);

    rows.into_iter()
        .map(|(name, value, style)| {
            let mut spans = Vec::new();
            if left > 0 {
                spans.push(Span::raw(" ".repeat(left)));
            }
            spans.push(Span::styled(format!("{name:<9}  "), label_style));
            spans.push(Span::styled(value, style));
            Line::from(spans)
        })
        .collect()
}

fn centered(spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let used: usize = spans
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum();
    let left = width.saturating_sub(used) / 2;
    let mut line = Vec::with_capacity(spans.len() + 1);
    if left > 0 {
        line.push(Span::raw(" ".repeat(left)));
    }
    line.extend(spans);
    Line::from(line)
}

fn centered_text(text: &str, color: Color, width: usize) -> Line<'static> {
    centered(
        vec![Span::styled(text.to_string(), Style::default().fg(color))],
        width,
    )
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
    push_wrapped(
        &mut out,
        text,
        width.saturating_sub(3).max(10),
        "  ",
        Style::default().fg(Color::White),
    );
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

    let output = sanitize(&tool.output);
    let lines: Vec<&str> = output.trim_end_matches('\n').split('\n').collect();
    let limit = if expanded { lines.len() } else { 4 };
    let style = if tool.status == ToolStatus::Error {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::Gray)
    };
    let output_width = width.saturating_sub(6).max(1);
    let options = textwrap::Options::new(output_width).break_words(true);

    for line in lines.iter().take(limit) {
        let line = line.replace('\t', "    ");
        for chunk in textwrap::wrap(&line, options.clone()) {
            out.push(Line::from(vec![
                Span::styled("  │ ", Style::default().fg(Color::DarkGray)),
                Span::styled(chunk.into_owned(), style),
            ]));
        }
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
    for line in crate::markdown::render(text, width) {
        let mut spans = Vec::with_capacity(line.spans.len() + 1);
        spans.push(Span::raw("  "));
        spans.extend(line.spans);
        out.push(Line::from(spans));
    }
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
    let hint = if app.completion_open() {
        "↑↓ select · tab complete · enter run · esc clear"
    } else if app.busy {
        "esc cancel · wheel/pgup scroll · ctrl+o tools · ctrl+c quit"
    } else {
        "enter send · / commands · wheel/pgup scroll · ctrl+o tools · esc clear · ctrl+c quit"
    };
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
        let status_width = (UnicodeWidthStr::width(text.as_str()) + 1).min(area.width as usize);
        let chunks =
            Layout::horizontal([Constraint::Min(0), Constraint::Length(status_width as u16)])
                .split(area);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {hint}"),
                Style::default().fg(Color::DarkGray),
            ))),
            chunks[0],
        );
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("{text} "),
                Style::default().fg(color),
            )))
            .alignment(Alignment::Right),
            chunks[1],
        );
    } else {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {hint}"),
                Style::default().fg(Color::DarkGray),
            ))),
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
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        let character = if character.is_control() {
            ' '
        } else {
            character
        };
        let width = UnicodeWidthChar::width(character).unwrap_or(0);
        // Reserve a cell for the ellipsis when there is more text to come.
        if characters.peek().is_some() && used + width >= max {
            if !out.is_empty() {
                out.push('…');
            }
            return out;
        }
        if used + width > max {
            break;
        }
        out.push(character);
        used += width;
    }
    out
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
    fn shows_a_scrollbar_for_long_conversations() {
        let mut app = long_conversation();
        let rows = screen_rows(&mut app, 60, 20);
        assert!(rows.iter().any(|row| row.contains('█')));
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
    fn wraps_long_tool_output_to_the_chat_width() {
        let mut app = App::new(&config());
        app.on_agent_event(AgentEvent::ToolStarted {
            id: "call_1".to_string(),
            name: "bash".to_string(),
            args: r#"{"command":"printf output"}"#.to_string(),
        });
        app.on_agent_event(AgentEvent::ToolFinished {
            id: "call_1".to_string(),
            ok: true,
            output: "abcdefghijabcdefghijabcdefghij".to_string(),
        });

        let rows = screen_rows(&mut app, 30, 20);
        assert!(rows.iter().any(|row| row.contains("abcdefgh")));
        assert!(rows.iter().filter(|row| row.contains("│")).count() >= 2);
    }

    #[test]
    fn keeps_busy_indicator_and_footer_status_separate() {
        let mut app = App::new(&config());
        app.model = "a-model-name-that-is-too-long-for-the-header".to_string();
        app.workspace = "/a/very/long/workspace/path/that/does/not/fit".to_string();
        app.busy = true;
        app.status = Some("waiting for approval".to_string());

        let rows = screen_rows(&mut app, 70, 20);
        assert!(rows[0].contains("sarvam-code"));
        assert!(rows[0].contains("working"));
        assert!(rows[19].contains("esc cancel"));
        assert!(rows[19].contains("waiting for approval"));
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
    fn renders_assistant_markdown() {
        let mut app = App::new(&config());
        app.on_agent_event(AgentEvent::TurnStarted);
        app.on_agent_event(AgentEvent::TextDelta(
            "## Markdown demo\n\nThis is **bold**, *italic*, `code`, and ~~gone~~.\n\n\
             - item one\n  - nested\n\n> quoted\n\n\
             ```rust\nfn main() {}\n```\n\n\
             | Name | Type |\n|------|------|\n| foo  | bar  |\n\n---\n\nDone."
                .to_string(),
        ));
        app.on_agent_event(AgentEvent::TurnFinished);

        let screen = rendered(&mut app, 100, 40);
        assert!(screen.contains("Markdown demo"));
        assert!(screen.contains("• item one"));
        assert!(screen.contains("nested"));
        assert!(screen.contains("│ quoted"));
        assert!(screen.contains("│ fn main() {}"));
        assert!(screen.contains("Name"));
        assert!(screen.contains("bar"));
        assert!(screen.contains("────"));
        assert!(screen.contains("Done."));
        assert!(!screen.contains("**"));
        assert!(!screen.contains("```"));
    }

    #[test]
    fn shows_slash_command_palette_above_the_input() {
        let mut app = App::new(&config());
        app.input.set("/".to_string());

        let rows = screen_rows(&mut app, 70, 24);
        let palette = rows
            .iter()
            .position(|row| row.contains("commands 8 matches"))
            .expect("palette title is visible");
        let input = rows
            .iter()
            .position(|row| row.contains("message"))
            .expect("input box is visible");
        assert!(palette < input, "palette should sit above the input");
        assert!(
            rows[palette + 1..input]
                .iter()
                .any(|row| row.contains("/help"))
        );
        assert!(
            rows[palette + 1..input]
                .iter()
                .any(|row| row.contains("/tools"))
        );
        assert!(rows[palette + 1..input].iter().any(|row| row.contains('›')));
    }

    #[test]
    fn filters_the_slash_command_palette() {
        let mut app = App::new(&config());
        app.input.set("/mo".to_string());

        let rows = screen_rows(&mut app, 70, 24);
        let start = rows
            .iter()
            .position(|row| row.contains("commands 2 matches"))
            .expect("palette title");
        let end = rows
            .iter()
            .position(|row| row.contains("message"))
            .expect("input box");
        let palette = &rows[start..end];
        assert!(palette[0].contains("2 matches"));
        assert!(palette.iter().any(|row| row.contains("/mode")));
        assert!(palette.iter().any(|row| row.contains("/model")));
        assert!(!palette.iter().any(|row| row.contains("/help")));

        app.completion_move(1);
        let rows = screen_rows(&mut app, 70, 24);
        let model_row = rows
            .iter()
            .find(|row| row.contains("/model"))
            .expect("/model row");
        assert!(model_row.contains('›'));
    }

    #[test]
    fn shows_fuzzy_model_palette_after_model_command() {
        let mut app = App::new(&config());
        app.input.set("/model s105".to_string());

        let rows = screen_rows(&mut app, 70, 24);
        assert!(rows.iter().any(|row| row.contains("models 2 matches")));
        assert!(rows.iter().any(|row| row.contains("sarvam-105b")));
        assert!(
            rows.iter()
                .any(|row| row.contains("sarvam-105b-conversations"))
        );
        assert!(!rows.iter().any(|row| row.contains("gemma4")));
    }

    #[test]
    fn welcome_screen_shows_mark_and_session_specs() {
        let mut app = App::new(&config());
        let screen = rendered(&mut app, 100, 32);
        assert!(screen.contains("▗▄▛▀▙▖"), "the Sarvam rosette is rendered");
        assert!(screen.contains("Welcome to sarvam-code"));
        assert!(screen.contains("a minimal Sarvam AI coding agent"));
        assert!(screen.contains("test-model"));
        assert!(screen.contains("workspace"));
        assert!(screen.contains("list_dir"));
        assert!(screen.contains("ask"));
        assert!(screen.contains("/ for commands"));
        assert!(!screen.contains("sarvam-code v"), "no version panel");
        assert!(!screen.contains("███████╗"), "no block-letter wordmark");
    }

    #[test]
    fn welcome_screen_adapts_to_small_terminals() {
        let mut app = App::new(&config());
        let compact = rendered(&mut app, 44, 24);
        assert!(compact.contains("▗▄▀▀▚▖"), "compact rosette is rendered");
        assert!(!compact.contains("▗▄▛▀▙▖"), "large rosette is not rendered");
        assert!(compact.contains("test-model"));
        assert!(compact.contains("Welcome to sarvam-code"));

        let mut app = App::new(&config());
        let medium = rendered(&mut app, 44, 20);
        assert!(medium.contains("Welcome to sarvam-code"));
        assert!(medium.contains("test-model"));
        assert!(!medium.contains("▗"), "mark is dropped when it cannot fit");

        let mut app = App::new(&config());
        let text_only = rendered(&mut app, 28, 12);
        assert!(text_only.contains("Welcome to sarvam-code"));
        assert!(
            !text_only.contains("▗"),
            "mark is dropped when it cannot fit"
        );
        assert!(text_only.contains("/ for commands"));

        let mut app = App::new(&config());
        let narrow = rendered(&mut app, 24, 16);
        assert!(narrow.contains("sarvam-code"));
        assert!(narrow.contains("test-model"));
    }

    #[test]
    fn tiny_terminal_does_not_panic() {
        let mut app = App::new(&config());
        let screen = rendered(&mut app, 16, 6);
        assert!(screen.contains("terminal"));

        let mut app = App::new(&config());
        app.input.set("/".to_string());
        let _ = rendered(&mut app, 30, 9);
    }
}
