use pulldown_cmark::{
    Alignment, BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd,
};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const TEXT: Color = Color::White;
const DIM: Color = Color::DarkGray;
const LINK: Color = Color::Cyan;
const CODE: Color = Color::LightGreen;
const INLINE_CODE: Color = Color::Yellow;

/// Render CommonMark/GFM into styled terminal lines that fit `width` cells.
///
/// The caller is responsible for any outer indentation; this renderer emits
/// list markers, quote bars, table borders and code gutters relative to
/// column zero.
pub fn render(text: &str, width: usize) -> Vec<Line<'static>> {
    let mut renderer = Renderer::new(width);
    let mut options = Options::empty();
    options.insert(
        Options::ENABLE_TABLES
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_GFM,
    );
    for event in Parser::new_ext(&sanitize(text), options) {
        renderer.event(event);
    }
    renderer.finish()
}

/// Remove terminal control sequences from model/tool output.
pub fn sanitize(text: &str) -> String {
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

#[derive(Debug)]
enum Piece {
    Text(String, Style),
    /// A hard line break (`\` at the end of a line, i.e. `<br>`).
    Break,
}

struct ListState {
    next: u64,
    ordered: bool,
}

struct ItemState {
    marker: String,
    marker_width: usize,
    indent: usize,
    /// The marker has not been emitted yet.
    first: bool,
}

struct CodeState {
    label: String,
    text: String,
}

struct TableState {
    aligns: Vec<Alignment>,
    rows: Vec<Vec<Vec<Span<'static>>>>,
    header_rows: usize,
    in_head: bool,
    row: Vec<Vec<Span<'static>>>,
}

struct Renderer {
    width: usize,
    out: Vec<Line<'static>>,
    inline: Vec<Piece>,
    style: Style,
    styles: Vec<Style>,
    quote_colors: Vec<Color>,
    lists: Vec<ListState>,
    items: Vec<ItemState>,
    code: Option<CodeState>,
    table: Option<TableState>,
}

impl Renderer {
    fn new(width: usize) -> Self {
        Self {
            width: width.max(12),
            out: Vec::new(),
            inline: Vec::new(),
            style: Style::default().fg(TEXT),
            styles: Vec::new(),
            quote_colors: Vec::new(),
            lists: Vec::new(),
            items: Vec::new(),
            code: None,
            table: None,
        }
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        self.flush_inline();
        while self.out.last().is_some_and(|line| line.spans.is_empty()) {
            self.out.pop();
        }
        self.out
    }

    fn event(&mut self, event: Event<'_>) {
        if self.code.is_some() {
            match event {
                Event::Text(text) => {
                    if let Some(code) = &mut self.code {
                        code.text.push_str(&text);
                    }
                    return;
                }
                Event::End(TagEnd::CodeBlock) => {
                    self.finish_code();
                    return;
                }
                _ => {}
            }
        }

        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => {
                let text = text.replace('\n', " ");
                self.push_text(&text, self.style);
            }
            Event::Code(code) => {
                self.push_text(&code, Style::default().fg(INLINE_CODE));
            }
            Event::InlineMath(math) | Event::DisplayMath(math) => {
                self.push_text(&format!("${math}$"), Style::default().fg(INLINE_CODE));
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                if is_break_tag(&html) {
                    self.inline.push(Piece::Break);
                }
            }
            Event::FootnoteReference(label) => {
                self.push_text(&format!("[^{label}]"), Style::default().fg(DIM));
            }
            Event::SoftBreak => self.push_text(" ", self.style),
            Event::HardBreak => self.inline.push(Piece::Break),
            Event::Rule => {
                self.flush_inline();
                self.block_break();
                self.out.push(Line::from(Span::styled(
                    "─".repeat(self.width),
                    Style::default().fg(DIM),
                )));
                self.block_break();
            }
            Event::TaskListMarker(checked) => {
                self.push_text(
                    if checked { "☑ " } else { "☐ " },
                    Style::default().fg(Color::LightBlue),
                );
            }
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {}
            Tag::Heading { level, .. } => {
                self.block_break();
                self.styles.push(self.style);
                self.style = heading_style(level);
            }
            Tag::BlockQuote(kind) => {
                if self.quote_colors.is_empty() {
                    self.block_break();
                }
                self.quote_colors.push(quote_color(kind));
            }
            Tag::CodeBlock(kind) => {
                self.block_break();
                let label = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some(CodeState {
                    label,
                    text: String::new(),
                });
            }
            Tag::List(start) => {
                if self.lists.is_empty() {
                    self.block_break();
                } else {
                    // End the parent item's text before its nested list.
                    self.flush_inline();
                }
                self.lists.push(ListState {
                    next: start.unwrap_or(1),
                    ordered: start.is_some(),
                });
            }
            Tag::Item => {
                let indent = self.lists.len().saturating_sub(1) * 2;
                let marker = match self.lists.last_mut() {
                    Some(list) if list.ordered => {
                        let number = list.next;
                        list.next += 1;
                        format!("{number}. ")
                    }
                    _ => "• ".to_string(),
                };
                let marker_width = UnicodeWidthStr::width(marker.as_str());
                self.items.push(ItemState {
                    marker,
                    marker_width,
                    indent,
                    first: true,
                });
            }
            Tag::Emphasis => {
                self.styles.push(self.style);
                self.style = self.style.add_modifier(Modifier::ITALIC);
            }
            Tag::Strong => {
                self.styles.push(self.style);
                self.style = self.style.add_modifier(Modifier::BOLD);
            }
            Tag::Strikethrough => {
                self.styles.push(self.style);
                self.style = self.style.add_modifier(Modifier::CROSSED_OUT);
            }
            Tag::Link { .. } => {
                self.styles.push(self.style);
                self.style = self.style.fg(LINK).add_modifier(Modifier::UNDERLINED);
            }
            Tag::Image { .. } => {
                self.styles.push(self.style);
                self.style = Style::default().fg(Color::Gray);
                self.push_text("[image: ", Style::default().fg(DIM));
            }
            Tag::Table(aligns) => {
                self.block_break();
                self.table = Some(TableState {
                    aligns,
                    rows: Vec::new(),
                    header_rows: 0,
                    in_head: false,
                    row: Vec::new(),
                });
            }
            Tag::TableHead => {
                if let Some(table) = &mut self.table {
                    table.in_head = true;
                    table.row.clear();
                }
            }
            Tag::TableRow => {
                if let Some(table) = &mut self.table {
                    table.row.clear();
                }
            }
            Tag::TableCell => {}
            Tag::FootnoteDefinition(label) => {
                self.push_text(&format!("[^{label}] "), Style::default().fg(DIM));
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush_inline();
                if self.items.is_empty() {
                    self.block_break();
                }
            }
            TagEnd::Heading(_) => {
                self.flush_inline();
                self.style = self.styles.pop().unwrap_or_else(base_style);
                self.block_break();
            }
            TagEnd::BlockQuote(_) => {
                self.quote_colors.pop();
                if self.quote_colors.is_empty() {
                    self.block_break();
                }
            }
            TagEnd::CodeBlock => {}
            TagEnd::List(_) => {
                self.lists.pop();
                if self.lists.is_empty() {
                    self.block_break();
                }
            }
            TagEnd::Item => {
                self.flush_inline();
                self.items.pop();
            }
            TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Link
            | TagEnd::Image => {
                self.style = self.styles.pop().unwrap_or_else(base_style);
                if tag == TagEnd::Image {
                    self.push_text("]", Style::default().fg(DIM));
                }
            }
            TagEnd::TableHead => {
                if let Some(table) = &mut self.table {
                    table.in_head = false;
                    let row = std::mem::take(&mut table.row);
                    if !row.is_empty() {
                        table.rows.push(row);
                    }
                    table.header_rows = table.rows.len();
                }
            }
            TagEnd::TableRow => {
                if let Some(table) = &mut self.table {
                    let row = std::mem::take(&mut table.row);
                    table.rows.push(row);
                }
            }
            TagEnd::TableCell => {
                let spans = pieces_to_spans(std::mem::take(&mut self.inline));
                if let Some(table) = &mut self.table {
                    let spans = if table.in_head {
                        spans
                            .into_iter()
                            .map(|span| {
                                span.patch_style(Style::default().add_modifier(Modifier::BOLD))
                            })
                            .collect()
                    } else {
                        spans
                    };
                    table.row.push(spans);
                }
            }
            TagEnd::Table => self.finish_table(),
            _ => {}
        }
    }

    fn finish_code(&mut self) {
        let Some(code) = self.code.take() else {
            return;
        };
        let gutter = Span::styled("│ ", Style::default().fg(DIM));
        let available = self.width.saturating_sub(2).max(4);

        let label = code.label.trim();
        if !label.is_empty() {
            self.out.push(Line::from(vec![
                gutter.clone(),
                Span::styled(
                    label.to_string(),
                    Style::default().fg(DIM).add_modifier(Modifier::ITALIC),
                ),
            ]));
        }

        let text = code.text.trim_end_matches('\n');
        if text.is_empty() {
            self.out.push(Line::from(vec![
                gutter,
                Span::styled("(empty)", Style::default().fg(DIM)),
            ]));
        } else {
            for raw in text.split('\n') {
                let expanded = raw.replace('\t', "    ");
                let clipped = clip_cells(&expanded, available);
                self.out.push(Line::from(vec![
                    gutter.clone(),
                    Span::styled(clipped, Style::default().fg(CODE)),
                ]));
            }
        }
        self.block_break();
    }

    fn finish_table(&mut self) {
        let Some(table) = self.table.take() else {
            return;
        };
        let columns = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        if columns == 0 {
            return;
        }

        let mut widths = vec![1usize; columns];
        for row in &table.rows {
            for (index, cell) in row.iter().enumerate() {
                widths[index] = widths[index].max(span_width(cell));
            }
        }
        while widths.iter().sum::<usize>() + 2 * columns.saturating_sub(1) > self.width {
            let Some((index, _)) = widths.iter().enumerate().max_by_key(|(_, width)| **width)
            else {
                break;
            };
            if widths[index] <= 3 {
                break;
            }
            widths[index] -= 1;
        }

        for (row_index, row) in table.rows.iter().enumerate() {
            let mut spans = Vec::new();
            for (column, width) in widths.iter().enumerate() {
                if column > 0 {
                    spans.push(Span::raw("  "));
                }
                let empty = Vec::new();
                let cell = row.get(column).unwrap_or(&empty);
                let align = table.aligns.get(column).copied().unwrap_or(Alignment::None);
                spans.extend(fit_spans(cell, *width, align));
            }
            self.out.push(Line::from(spans));

            if row_index + 1 == table.header_rows {
                let mut rule = Vec::new();
                for (column, width) in widths.iter().enumerate() {
                    if column > 0 {
                        rule.push(Span::raw("  "));
                    }
                    rule.push(Span::styled("─".repeat(*width), Style::default().fg(DIM)));
                }
                self.out.push(Line::from(rule));
            }
        }
        self.block_break();
    }

    fn flush_inline(&mut self) {
        if self.inline.is_empty() {
            return;
        }
        let pieces = std::mem::take(&mut self.inline);
        let (first_prefix, next_prefix) = self.line_prefixes();
        let prefix_width = span_width(&first_prefix);
        let available = self.width.saturating_sub(prefix_width).max(8);

        for (index, spans) in wrap_pieces(pieces, available).into_iter().enumerate() {
            let mut line = if index == 0 {
                first_prefix.clone()
            } else {
                next_prefix.clone()
            };
            line.extend(spans);
            self.out.push(Line::from(line));
        }

        if let Some(item) = self.items.last_mut() {
            item.first = false;
        }
    }

    fn line_prefixes(&self) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
        let mut quote = Vec::new();
        for color in &self.quote_colors {
            quote.push(Span::styled("│ ", Style::default().fg(*color)));
        }

        match self.items.last() {
            Some(item) if item.first => {
                let mut first = quote.clone();
                first.push(Span::raw(" ".repeat(item.indent)));
                first.push(Span::styled(
                    item.marker.clone(),
                    Style::default()
                        .fg(Color::LightBlue)
                        .add_modifier(Modifier::BOLD),
                ));
                let mut next = quote;
                next.push(Span::raw(" ".repeat(item.indent + item.marker_width)));
                (first, next)
            }
            _ => (quote.clone(), quote),
        }
    }

    fn push_text(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        self.inline.push(Piece::Text(text.to_string(), style));
    }

    /// Separate block-level elements with one blank line.
    fn block_break(&mut self) {
        if self.out.is_empty() {
            return;
        }
        if self.out.last().is_some_and(|line| line.spans.is_empty()) {
            return;
        }
        self.out.push(Line::default());
    }
}

fn base_style() -> Style {
    Style::default().fg(TEXT)
}

fn heading_style(level: HeadingLevel) -> Style {
    let (color, modifier) = match level {
        HeadingLevel::H1 => (Color::Cyan, Modifier::BOLD | Modifier::UNDERLINED),
        HeadingLevel::H2 => (Color::Cyan, Modifier::BOLD),
        HeadingLevel::H3 => (Color::LightBlue, Modifier::BOLD),
        HeadingLevel::H4 => (Color::LightMagenta, Modifier::BOLD),
        HeadingLevel::H5 => (Color::Magenta, Modifier::BOLD),
        HeadingLevel::H6 => (DIM, Modifier::BOLD),
    };
    Style::default().fg(color).add_modifier(modifier)
}

fn quote_color(kind: Option<BlockQuoteKind>) -> Color {
    match kind {
        Some(BlockQuoteKind::Note) => Color::Blue,
        Some(BlockQuoteKind::Tip) => Color::Green,
        Some(BlockQuoteKind::Important) => Color::Magenta,
        Some(BlockQuoteKind::Warning) => Color::Yellow,
        Some(BlockQuoteKind::Caution) => Color::Red,
        None => DIM,
    }
}

fn is_break_tag(html: &str) -> bool {
    let tag = html.trim().trim_start_matches('<').trim_end_matches('>');
    tag.eq_ignore_ascii_case("br") || tag.eq_ignore_ascii_case("br/")
}

fn span_width(spans: &[Span<'static>]) -> usize {
    spans
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum()
}

/// Wrap styled pieces into lines of at most `width` cells.
fn wrap_pieces(pieces: Vec<Piece>, width: usize) -> Vec<Vec<Span<'static>>> {
    let width = width.max(4);
    let mut lines: Vec<Vec<(char, Style)>> = Vec::new();
    let mut line: Vec<(char, Style)> = Vec::new();
    let mut line_width = 0;
    let mut word: Vec<(char, Style)> = Vec::new();
    let mut word_width = 0;

    for piece in pieces {
        match piece {
            Piece::Break => {
                flush_word(
                    &mut word,
                    &mut word_width,
                    &mut line,
                    &mut line_width,
                    &mut lines,
                    width,
                );
                lines.push(std::mem::take(&mut line));
                line_width = 0;
            }
            Piece::Text(text, style) => {
                for character in text.chars() {
                    if character.is_whitespace() {
                        flush_word(
                            &mut word,
                            &mut word_width,
                            &mut line,
                            &mut line_width,
                            &mut lines,
                            width,
                        );
                    } else {
                        word_width += UnicodeWidthChar::width(character).unwrap_or(0);
                        word.push((character, style));
                    }
                }
            }
        }
    }

    flush_word(
        &mut word,
        &mut word_width,
        &mut line,
        &mut line_width,
        &mut lines,
        width,
    );
    if !line.is_empty() {
        lines.push(line);
    }
    lines.into_iter().map(spans_from_chars).collect()
}

fn flush_word(
    word: &mut Vec<(char, Style)>,
    word_width: &mut usize,
    line: &mut Vec<(char, Style)>,
    line_width: &mut usize,
    lines: &mut Vec<Vec<(char, Style)>>,
    width: usize,
) {
    if word.is_empty() {
        return;
    }

    if !line.is_empty() && *line_width + 1 + *word_width > width {
        lines.push(std::mem::take(line));
        *line_width = 0;
    }
    if !line.is_empty() {
        line.push((' ', Style::default()));
        *line_width += 1;
    }

    if *line_width + *word_width <= width {
        line.append(word);
        *line_width += *word_width;
    } else {
        for (character, style) in word.drain(..) {
            let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
            if !line.is_empty() && *line_width + character_width > width {
                lines.push(std::mem::take(line));
                *line_width = 0;
            }
            line.push((character, style));
            *line_width += character_width;
        }
    }
    *word_width = 0;

    if *line_width >= width {
        lines.push(std::mem::take(line));
        *line_width = 0;
    }
}

fn spans_from_chars(chars: Vec<(char, Style)>) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (character, style) in chars {
        if let Some(last) = spans.last_mut()
            && last.style == style
        {
            last.content.to_mut().push(character);
            continue;
        }
        spans.push(Span::styled(character.to_string(), style));
    }
    spans
}

fn pieces_to_spans(pieces: Vec<Piece>) -> Vec<Span<'static>> {
    let mut chars = Vec::new();
    for piece in pieces {
        match piece {
            Piece::Text(text, style) => {
                for character in text.chars() {
                    chars.push((if character == '\n' { ' ' } else { character }, style));
                }
            }
            Piece::Break => chars.push((' ', Style::default())),
        }
    }
    spans_from_chars(chars)
}

fn fit_spans(spans: &[Span<'static>], width: usize, align: Alignment) -> Vec<Span<'static>> {
    let mut chars: Vec<(char, Style)> = Vec::new();
    for span in spans {
        for character in span.content.chars() {
            chars.push((character, span.style));
        }
    }

    let mut used = chars
        .iter()
        .map(|(character, _)| UnicodeWidthChar::width(*character).unwrap_or(0))
        .sum::<usize>();

    if used > width {
        let limit = width.saturating_sub(1);
        let mut kept = Vec::new();
        let mut current = 0;
        for (character, style) in chars {
            let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
            if current + character_width > limit {
                break;
            }
            current += character_width;
            kept.push((character, style));
        }
        kept.push(('…', Style::default().fg(DIM)));
        used = current + 1;
        chars = kept;
    }

    let padding = width.saturating_sub(used);
    let (left, right) = match align {
        Alignment::Right => (padding, 0),
        Alignment::Center => (padding / 2, padding - padding / 2),
        _ => (0, padding),
    };

    let mut out = Vec::new();
    if left > 0 {
        out.push(Span::raw(" ".repeat(left)));
    }
    out.extend(spans_from_chars(chars));
    if right > 0 {
        out.push(Span::raw(" ".repeat(right)));
    }
    out
}

/// Clip a plain string to at most `max` terminal cells.
fn clip_cells(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for character in text.chars() {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn find_style(lines: &[Line<'static>], text: &str) -> Style {
        lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .find(|span| span.content == text)
            .map(|span| span.style)
            .unwrap_or_else(|| panic!("missing span {text:?} in {:?}", plain(lines)))
    }

    #[test]
    fn renders_inline_emphasis_strikethrough_and_code() {
        let lines = render("**bold** *italic* ~~gone~~ `code`", 60);
        assert_eq!(plain(&lines), "bold italic gone code");

        assert!(
            find_style(&lines, "bold")
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(
            find_style(&lines, "italic")
                .add_modifier
                .contains(Modifier::ITALIC)
        );
        assert!(
            find_style(&lines, "gone")
                .add_modifier
                .contains(Modifier::CROSSED_OUT)
        );
        assert_eq!(find_style(&lines, "code").fg, Some(INLINE_CODE));
    }

    #[test]
    fn renders_headings_with_distinct_styles() {
        let lines = render("# Title\n\n## Section", 40);
        assert_eq!(plain(&lines), "Title\n\nSection");
        let h1 = find_style(&lines, "Title");
        assert_eq!(h1.fg, Some(Color::Cyan));
        assert!(h1.add_modifier.contains(Modifier::BOLD));
        let h2 = find_style(&lines, "Section");
        assert_eq!(h2.fg, Some(Color::Cyan));
        assert!(!h2.add_modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn renders_fenced_code_without_reflowing() {
        let lines = render(
            "```rust\nfn main() {\n    println!(\"hello world\");\n}\n```",
            60,
        );
        let text = plain(&lines);
        assert!(text.contains("rust"));
        assert!(text.contains("│ fn main() {"));
        assert!(text.contains("│     println!(\"hello world\");"));
        assert!(text.contains("│ }"));
    }

    #[test]
    fn clips_code_instead_of_wrapping_it() {
        let lines = render("```\naaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n```", 20);
        assert_eq!(lines.len(), 1);
        let text = plain(&lines);
        assert!(text.ends_with('…'));
        assert!(!text.contains('\n'));
    }

    #[test]
    fn renders_nested_and_ordered_lists() {
        let text = plain(&render("- one\n  - nested\n- two", 40));
        assert!(text.contains("• one"));
        assert!(text.contains("  • nested"));
        assert!(text.contains("• two"));

        let text = plain(&render("1. first\n2. second", 40));
        assert!(text.contains("1. first"));
        assert!(text.contains("2. second"));
    }

    #[test]
    fn renders_task_lists_and_quotes() {
        let text = plain(&render("- [x] done\n- [ ] todo", 40));
        assert!(text.contains("☑ done"));
        assert!(text.contains("☐ todo"));

        let lines = render("> quoted", 40);
        assert!(plain(&lines).contains("│ quoted"));
        assert_eq!(find_style(&lines, "quoted").fg, Some(TEXT));
    }

    #[test]
    fn renders_tables_with_aligned_columns() {
        let lines = render("| Name | Type |\n|------|-----:|\n| foo  | bar  |", 60);
        let text = plain(&lines);
        assert!(text.contains("Name"));
        assert!(text.contains("──"));
        assert!(text.contains("foo"));
        assert!(text.contains("bar"));
        assert!(
            find_style(&lines, "Name")
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }

    #[test]
    fn renders_links_and_rules() {
        let lines = render("[docs](https://example.com)", 60);
        assert_eq!(plain(&lines), "docs");
        let style = find_style(&lines, "docs");
        assert_eq!(style.fg, Some(LINK));
        assert!(style.add_modifier.contains(Modifier::UNDERLINED));

        let text = plain(&render("a\n\n---\n\nb", 20));
        assert!(text.contains('─'));
    }

    #[test]
    fn wraps_paragraphs_to_width() {
        let lines = render(
            "word word word word word word word word word word word word",
            20,
        );
        assert!(lines.len() > 1);
        assert!(
            lines.iter().all(|line| span_width(&line.spans) <= 20),
            "lines overflow: {:?}",
            plain(&lines)
        );
    }

    #[test]
    fn strips_html_and_control_characters() {
        let lines = render("hello <b>world</b>\n\n\u{1b}[31mred", 40);
        let text = plain(&lines);
        assert!(text.contains("hello world"));
        assert!(!text.contains('\u{1b}'));
    }

    #[test]
    fn empty_input_renders_nothing() {
        assert!(render("", 40).is_empty());
        assert!(render("\n\n\n", 40).is_empty());
    }
}
