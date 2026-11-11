//! GitHub-flavored Markdown, with the HTML GitHub allows in it, rendered as
//! native elements. Remote HTML is read for its structure and never executed.

use std::ops::Range;

use gpui::{
    AnyElement, App, ElementId, Font, FontStyle, FontWeight, Hsla, InteractiveText, IntoElement,
    RenderOnce, SharedString, StrikethroughStyle, StyledText, TextAlign, TextRun, Window, div,
    font, prelude::*, px, svg,
};
use pulldown_cmark::{
    Alignment, BlockQuoteKind, CodeBlockKind, Event, Options, Parser, Tag, TagEnd,
};

use crate::ui::syntax::{Language, highlight_code, language_from_fence};
use crate::ui::theme::{MONO_FONT, colors, content_on, mix, surface_tint};

/// The family the window's root sets. Text that mixes in monospaced runs
/// restates it for the rest of the line.
const UI_FONT: &str = ".SystemUIFont";
/// Past this a table column wraps its text instead of growing.
const MAX_COLUMN_WIDTH: f32 = 420.0;

pub fn safe_link(url: &str) -> bool {
    (url.starts_with("https://") || url.starts_with("http://"))
        && !url.chars().any(char::is_control)
}

/// A comment or reply.
pub fn markdown(id: &str, text: &str) -> Markdown {
    Markdown::new(id, text, false)
}

/// More spacious typography for the Inbox's primary description.
pub fn markdown_prose(id: &str, text: &str) -> Markdown {
    Markdown::new(id, text, true)
}

/// Markdown laid out when it is drawn, which is when tables can measure
/// their columns and folded sections remember whether they are open.
#[derive(IntoElement)]
pub struct Markdown {
    id: SharedString,
    source: String,
    prose: bool,
    repository: Option<String>,
}

impl Markdown {
    fn new(id: &str, text: &str, prose: bool) -> Self {
        Self {
            id: SharedString::from(id.to_owned()),
            source: text.to_owned(),
            prose,
            repository: None,
        }
    }

    /// Links `#123` and `@user` as GitHub does, in the repository `url` (an
    /// issue's or pull request's) belongs to.
    pub fn references_from(mut self, url: &str) -> Self {
        self.repository = repository_url(url);
        self
    }
}

impl RenderOnce for Markdown {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let metrics = Metrics::of(self.prose);
        let nodes = parse(&self.source, self.repository.as_deref());
        let children = Renderer {
            id: self.id,
            metrics,
            next: 0,
            window,
            cx,
        }
        .blocks(nodes, Scope::default());
        div()
            .min_w(px(0.0))
            .flex()
            .flex_col()
            .gap(px(metrics.gap))
            .text_size(px(metrics.text))
            .line_height(px(metrics.line))
            .text_color(colors().foreground)
            .children(children)
    }
}

/// `scheme://host/owner/repo` of a GitHub URL inside that repository.
fn repository_url(url: &str) -> Option<String> {
    let parts: Vec<&str> = url.splitn(6, '/').collect();
    match parts.as_slice() {
        [scheme, "", host, owner, repo, ..]
            if !host.is_empty() && !owner.is_empty() && !repo.is_empty() =>
        {
            let base = format!("{scheme}//{host}/{owner}/{repo}");
            safe_link(&base).then_some(base)
        }
        _ => None,
    }
}

// Document --------------------------------------------------------------------

/// How a stretch of inline text is set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Style {
    strong: bool,
    italic: bool,
    strike: bool,
    code: bool,
    link: bool,
}

/// A line of text and its styling: spans follow each other in order without
/// overlapping, and links mark the ranges a click opens.
#[derive(Clone, Debug, Default, PartialEq)]
struct Inline {
    text: String,
    spans: Vec<(Range<usize>, Style)>,
    links: Vec<(Range<usize>, String)>,
}

impl Inline {
    fn plain(text: &str) -> Self {
        let mut inline = Self::default();
        inline.push(text, Style::default(), None);
        inline
    }

    fn push(&mut self, text: &str, style: Style, link: Option<String>) {
        let start = self.text.len();
        self.text.push_str(text);
        let end = self.text.len();
        if start == end {
            return;
        }
        match self.spans.last_mut() {
            Some((range, last)) if *last == style && range.end == start => range.end = end,
            _ => self.spans.push((start..end, style)),
        }
        if let Some(url) = link {
            match self.links.last_mut() {
                Some((range, last)) if *last == url && range.end == start => range.end = end,
                _ => self.links.push((start..end, url)),
            }
        }
    }

    fn ends_open(&self) -> bool {
        self.text.is_empty() || self.text.ends_with(char::is_whitespace)
    }

    fn trim_end(&mut self) {
        let len = self.text.trim_end().len();
        self.text.truncate(len);
        self.spans.retain_mut(|(range, _)| {
            range.end = range.end.min(len);
            range.start < range.end
        });
        self.links.retain_mut(|(range, _)| {
            range.end = range.end.min(len);
            range.start < range.end
        });
    }

    /// `other` after a space, as a block's text joins a line.
    fn append(&mut self, other: Inline) {
        if !self.ends_open() {
            self.push(" ", Style::default(), None);
        }
        let offset = self.text.len();
        self.text.push_str(&other.text);
        let shift = |range: Range<usize>| range.start + offset..range.end + offset;
        self.spans.extend(
            other
                .spans
                .into_iter()
                .map(|(range, style)| (shift(range), style)),
        );
        self.links.extend(
            other
                .links
                .into_iter()
                .map(|(range, url)| (shift(range), url)),
        );
    }
}

/// What leads a list item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Marker {
    Bullet,
    Number(u64),
    Task(bool),
}

#[derive(Debug, PartialEq)]
enum Node {
    Paragraph(Inline),
    Heading(u8, Inline),
    Code {
        text: String,
        language: Language,
    },
    Quote(Vec<Node>),
    Alert(BlockQuoteKind, Vec<Node>),
    /// Numbered from `start` when ordered.
    List {
        start: Option<u64>,
        items: Vec<Item>,
    },
    Table(Table),
    /// `index` counts folded sections in the order they are written, so each
    /// keeps its open state however the ones before it are folded.
    Details {
        index: usize,
        open: bool,
        summary: Inline,
        body: Vec<Node>,
    },
    Rule,
}

#[derive(Debug, Default, PartialEq)]
struct Item {
    task: Option<bool>,
    body: Vec<Node>,
}

#[derive(Debug, Default, PartialEq)]
struct Table {
    /// A Markdown table's alignment, by column.
    aligns: Vec<Option<TextAlign>>,
    head: Vec<Vec<Cell>>,
    rows: Vec<Vec<Cell>>,
}

#[derive(Debug, Default, PartialEq)]
struct Cell {
    header: bool,
    /// An HTML cell's own alignment.
    align: Option<TextAlign>,
    content: Inline,
}

// Parsing ---------------------------------------------------------------------

fn parse(source: &str, repository: Option<&str>) -> Vec<Node> {
    let mut builder = Builder::new(repository);
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_GFM;
    for event in Parser::new_ext(source, options) {
        builder.event(event);
    }
    builder.finish()
}

/// The containers a closing tag can end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Quote,
    List,
    Item,
    Details,
    Summary,
    Table,
    Row,
    Cell,
}

enum Frame {
    Root(Vec<Node>),
    Quote(Option<BlockQuoteKind>, Vec<Node>),
    List(Option<u64>, Vec<Item>),
    Item(Item),
    Details {
        index: usize,
        open: bool,
        summary: Option<Inline>,
        body: Vec<Node>,
    },
    Summary(Inline),
    Table(Table),
    Row {
        head: bool,
        cells: Vec<Cell>,
    },
    Cell(Cell),
}

impl Frame {
    fn kind(&self) -> Option<Kind> {
        Some(match self {
            Self::Root(_) => return None,
            Self::Quote(..) => Kind::Quote,
            Self::List(..) => Kind::List,
            Self::Item(_) => Kind::Item,
            Self::Details { .. } => Kind::Details,
            Self::Summary(_) => Kind::Summary,
            Self::Table(_) => Kind::Table,
            Self::Row { .. } => Kind::Row,
            Self::Cell(_) => Kind::Cell,
        })
    }
}

/// The paragraph or heading being written into the block at the top.
struct Open {
    inline: Inline,
    heading: Option<u8>,
}

/// Builds the document from Markdown events and the HTML inside them, which
/// open and close the same containers, so a `<details>` can hold Markdown.
struct Builder {
    stack: Vec<Frame>,
    open: Option<Open>,
    strong: usize,
    italic: usize,
    strike: usize,
    code: usize,
    /// The links around the text, innermost last; an unsafe one is `None`.
    links: Vec<Option<String>>,
    /// Whether each image being written has shown any alt text yet.
    images: Vec<bool>,
    details: usize,
    /// Containers opened inside a summary or cell, which only hold a line,
    /// with the depth they were opened at so their closings are skipped too.
    ignored: Vec<(Kind, usize)>,
    /// Inside an HTML `<thead>`.
    head: bool,
    code_block: Option<(String, Language)>,
    pre: Option<(String, Language)>,
    html_block: Option<String>,
    /// An element whose content is never shown, such as `<script>`.
    skip: Option<String>,
    /// Markdown text held until the next event, so a URL split across
    /// events still links whole.
    pending: String,
    repository: Option<String>,
}

impl Builder {
    fn new(repository: Option<&str>) -> Self {
        Self {
            stack: vec![Frame::Root(Vec::new())],
            open: None,
            strong: 0,
            italic: 0,
            strike: 0,
            code: 0,
            links: Vec::new(),
            images: Vec::new(),
            details: 0,
            ignored: Vec::new(),
            head: false,
            code_block: None,
            pre: None,
            html_block: None,
            skip: None,
            pending: String::new(),
            repository: repository.map(str::to_owned),
        }
    }

    fn event(&mut self, event: Event<'_>) {
        if let Event::Text(text) = &event {
            match &mut self.code_block {
                Some((code, _)) => code.push_str(text),
                None => self.pending.push_str(text),
            }
            return;
        }
        self.flush_text();
        match event {
            Event::Start(Tag::Paragraph) => self.paragraph(None),
            Event::End(TagEnd::Paragraph) => self.end_paragraph(),
            Event::Start(Tag::Heading { level, .. }) => self.paragraph(Some(level as u8)),
            Event::End(TagEnd::Heading(_)) => self.end_paragraph(),
            Event::Start(Tag::BlockQuote(kind)) => self.open_frame(Frame::Quote(kind, Vec::new())),
            Event::End(TagEnd::BlockQuote(_)) => self.close(Kind::Quote),
            Event::Start(Tag::CodeBlock(kind)) => {
                self.flush();
                let language = match kind {
                    CodeBlockKind::Fenced(info) => language_from_fence(
                        info.split(|c: char| c.is_whitespace() || c == ',' || c == '{')
                            .next()
                            .unwrap_or(""),
                    ),
                    CodeBlockKind::Indented => Language::Plain,
                };
                self.code_block = Some((String::new(), language));
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((text, language)) = self.code_block.take() {
                    self.code(text, language);
                }
            }
            Event::Start(Tag::List(start)) => self.open_frame(Frame::List(start, Vec::new())),
            Event::End(TagEnd::List(_)) => self.close(Kind::List),
            Event::Start(Tag::Item) => self.open_frame(Frame::Item(Item::default())),
            Event::End(TagEnd::Item) => self.close(Kind::Item),
            Event::TaskListMarker(checked) => self.task(checked),
            Event::Start(Tag::Table(aligns)) => self.open_frame(Frame::Table(Table {
                aligns: aligns.into_iter().map(alignment).collect(),
                ..Table::default()
            })),
            Event::End(TagEnd::Table) => self.close(Kind::Table),
            Event::Start(Tag::TableHead) => self.open_frame(Frame::Row {
                head: true,
                cells: Vec::new(),
            }),
            Event::Start(Tag::TableRow) => self.open_frame(Frame::Row {
                head: false,
                cells: Vec::new(),
            }),
            Event::End(TagEnd::TableHead | TagEnd::TableRow) => self.close(Kind::Row),
            Event::Start(Tag::TableCell) => {
                let header = matches!(self.stack.last(), Some(Frame::Row { head: true, .. }));
                self.open_frame(Frame::Cell(Cell {
                    header,
                    ..Cell::default()
                }));
            }
            Event::End(TagEnd::TableCell) => self.close(Kind::Cell),
            Event::Start(Tag::Strong) => self.strong += 1,
            Event::End(TagEnd::Strong) => self.strong = self.strong.saturating_sub(1),
            Event::Start(Tag::Emphasis) => self.italic += 1,
            Event::End(TagEnd::Emphasis) => self.italic = self.italic.saturating_sub(1),
            Event::Start(Tag::Strikethrough) => self.strike += 1,
            Event::End(TagEnd::Strikethrough) => self.strike = self.strike.saturating_sub(1),
            Event::Start(Tag::Link { dest_url, .. }) => {
                self.links
                    .push(safe_link(&dest_url).then(|| dest_url.into_string()));
            }
            Event::End(TagEnd::Link) => {
                self.links.pop();
            }
            Event::Start(Tag::Image { dest_url, .. }) => self.start_image(&dest_url),
            Event::End(TagEnd::Image) => self.end_image(),
            Event::Code(text) => {
                self.code += 1;
                self.push(&text);
                self.code -= 1;
            }
            Event::SoftBreak => self.push(" "),
            Event::HardBreak => self.line_break(),
            Event::Rule => {
                self.flush();
                self.attach(Node::Rule);
            }
            Event::Start(Tag::HtmlBlock) => {
                self.flush();
                self.html_block = Some(String::new());
            }
            Event::Html(html) => match &mut self.html_block {
                Some(block) => block.push_str(&html),
                None => self.html(&html),
            },
            Event::End(TagEnd::HtmlBlock) => {
                if let Some(block) = self.html_block.take() {
                    self.html(&block);
                }
            }
            Event::InlineHtml(html) => self.html(&html),
            _ => {}
        }
    }

    fn finish(mut self) -> Vec<Node> {
        self.flush_text();
        if let Some((text, language)) = self.code_block.take().or(self.pre.take()) {
            self.code(text, language);
        }
        self.flush();
        while self.stack.len() > 1 {
            self.pop();
        }
        match self.stack.pop() {
            Some(Frame::Root(body)) => body,
            _ => Vec::new(),
        }
    }

    fn style(&self) -> Style {
        Style {
            strong: self.strong > 0,
            italic: self.italic > 0,
            strike: self.strike > 0,
            code: self.code > 0,
            link: self.links.last().is_some_and(Option::is_some),
        }
    }

    /// Whether inline content lands in a summary or cell rather than a block.
    fn in_line(&self) -> bool {
        matches!(self.stack.last(), Some(Frame::Summary(_) | Frame::Cell(_)))
    }

    /// Where inline content goes: the summary or cell being written, else
    /// the paragraph open in the block at the top, opened if need be.
    fn target(&mut self) -> Option<&mut Inline> {
        match self.stack.last() {
            Some(Frame::Table(_) | Frame::Row { .. }) => return None,
            Some(Frame::List(..)) => self.stack.push(Frame::Item(Item::default())),
            _ => {}
        }
        match self.stack.last_mut() {
            Some(
                Frame::Summary(inline)
                | Frame::Cell(Cell {
                    content: inline, ..
                }),
            ) => Some(inline),
            _ => Some(
                &mut self
                    .open
                    .get_or_insert_with(|| Open {
                        inline: Inline::default(),
                        heading: None,
                    })
                    .inline,
            ),
        }
    }

    fn writing(&self) -> bool {
        match self.stack.last() {
            Some(
                Frame::Summary(inline)
                | Frame::Cell(Cell {
                    content: inline, ..
                }),
            ) => !inline.text.is_empty(),
            _ => self
                .open
                .as_ref()
                .is_some_and(|open| !open.inline.text.is_empty()),
        }
    }

    /// Adds text in the current style. Whitespace between blocks is
    /// dropped, and so is leading space on a line, which HTML collapses.
    fn push(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if text.trim().is_empty() && !self.writing() {
            return;
        }
        let style = self.style();
        let link = self.links.last().cloned().flatten();
        let Some(inline) = self.target() else {
            return;
        };
        let text = if !style.code && inline.ends_open() {
            text.trim_start()
        } else {
            text
        };
        if text.is_empty() {
            return;
        }
        inline.push(text, style, link);
        if let Some(shown) = self.images.last_mut() {
            *shown = true;
        }
    }

    /// The Markdown text held so far, with bare URLs, `#123`, and `@user`
    /// linked as GitHub links them.
    fn flush_text(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.pending);
        if self.code > 0 || !self.links.is_empty() {
            self.push(&text);
            return;
        }
        let mut at = 0;
        for found in references(&text, self.repository.as_deref()) {
            self.push(&text[at..found.range.start]);
            self.links.push(Some(found.url));
            self.strong += usize::from(found.mention);
            self.push(&text[found.range.clone()]);
            self.strong -= usize::from(found.mention);
            self.links.pop();
            at = found.range.end;
        }
        self.push(&text[at..]);
    }

    fn line_break(&mut self) {
        if !self.writing() {
            return;
        }
        if let Some(inline) = self.target() {
            inline.push("\n", Style::default(), None);
        }
    }

    fn start_image(&mut self, source: &str) {
        let target = self
            .links
            .last()
            .cloned()
            .flatten()
            .or_else(|| safe_link(source).then(|| source.to_owned()));
        self.links.push(target);
        self.images.push(false);
    }

    /// Images are not fetched: their alt text stands in, linked to them.
    fn end_image(&mut self) {
        if self.images.last() == Some(&false) {
            self.push("Image");
        }
        self.images.pop();
        self.links.pop();
    }

    fn paragraph(&mut self, heading: Option<u8>) {
        self.flush();
        if self.in_line() {
            self.separate();
        } else {
            self.open = Some(Open {
                inline: Inline::default(),
                heading,
            });
        }
    }

    fn end_paragraph(&mut self) {
        if self.in_line() {
            self.separate();
        } else {
            self.flush();
        }
    }

    /// A block's end inside a summary or cell leaves a space.
    fn separate(&mut self) {
        if let Some(
            Frame::Summary(inline)
            | Frame::Cell(Cell {
                content: inline, ..
            }),
        ) = self.stack.last_mut()
            && !inline.ends_open()
        {
            inline.push(" ", Style::default(), None);
        }
    }

    fn task(&mut self, checked: bool) {
        let opens_item = matches!(
            self.stack.last(),
            Some(Frame::Item(item)) if item.body.is_empty() && item.task.is_none()
        ) && !self.writing();
        match self.stack.last_mut() {
            Some(Frame::Item(item)) if opens_item => item.task = Some(checked),
            _ => self.push(if checked { "☑ " } else { "☐ " }),
        }
    }

    fn code(&mut self, text: String, language: Language) {
        let text = text.strip_prefix('\n').unwrap_or(&text);
        let text = text.trim_end_matches('\n').to_owned();
        self.flush();
        self.attach(Node::Code { text, language });
    }

    /// Ends the paragraph being written, adding it to its block.
    fn flush(&mut self) {
        let Some(Open {
            mut inline,
            heading,
        }) = self.open.take()
        else {
            return;
        };
        inline.trim_end();
        if inline.text.is_empty() {
            return;
        }
        self.attach(match heading {
            Some(level) => Node::Heading(level, inline),
            None => Node::Paragraph(inline),
        });
    }

    /// Adds a finished block to the container at the top.
    fn attach(&mut self, node: Node) {
        if matches!(self.stack.last(), Some(Frame::List(..))) {
            self.stack.push(Frame::Item(Item::default()));
        }
        match self.stack.last_mut() {
            Some(
                Frame::Root(body)
                | Frame::Quote(_, body)
                | Frame::Details { body, .. }
                | Frame::Item(Item { body, .. }),
            ) => body.push(node),
            // A summary or cell holds a line, so a block keeps only its text.
            Some(
                Frame::Summary(inline)
                | Frame::Cell(Cell {
                    content: inline, ..
                }),
            ) => {
                if let Node::Paragraph(text) | Node::Heading(_, text) = node {
                    inline.append(text);
                }
            }
            _ => {}
        }
    }

    fn open_frame(&mut self, frame: Frame) {
        self.flush();
        let Some(kind) = frame.kind() else {
            return;
        };
        let depth = self.stack.len();
        let fits = match (self.stack.last(), &frame) {
            (Some(Frame::Summary(_) | Frame::Cell(_)), _) => false,
            (_, Frame::Cell(_)) => {
                if matches!(self.stack.last(), Some(Frame::Table(_))) {
                    self.stack.push(Frame::Row {
                        head: self.head,
                        cells: Vec::new(),
                    });
                }
                matches!(self.stack.last(), Some(Frame::Row { .. }))
            }
            (_, Frame::Row { .. }) => matches!(self.stack.last(), Some(Frame::Table(_))),
            (Some(Frame::Table(_) | Frame::Row { .. }), _) => false,
            (Some(Frame::List(..)), Frame::Item(_)) => true,
            (Some(Frame::List(..)), _) => {
                self.stack.push(Frame::Item(Item::default()));
                true
            }
            _ => true,
        };
        if fits {
            self.stack.push(frame);
        } else {
            self.ignored.push((kind, depth));
        }
    }

    fn close(&mut self, kind: Kind) {
        let depth = self.stack.len();
        if let Some(index) = self
            .ignored
            .iter()
            .rposition(|ignored| *ignored == (kind, depth))
        {
            self.ignored.remove(index);
            return;
        }
        let Some(index) = self
            .stack
            .iter()
            .rposition(|frame| frame.kind() == Some(kind))
        else {
            return;
        };
        self.flush();
        while self.stack.len() > index {
            self.pop();
        }
        let depth = self.stack.len();
        self.ignored.retain(|(_, at)| *at <= depth);
    }

    /// Ends the container at the top, adding it to the one below.
    fn pop(&mut self) {
        self.flush();
        let Some(frame) = self.stack.pop() else {
            return;
        };
        match frame {
            Frame::Root(body) => self.stack.push(Frame::Root(body)),
            Frame::Quote(kind, body) => self.attach(match kind {
                Some(kind) => Node::Alert(kind, body),
                None => Node::Quote(body),
            }),
            Frame::List(start, items) => self.attach(Node::List { start, items }),
            Frame::Item(item) => match self.stack.last_mut() {
                Some(Frame::List(_, items)) => items.push(item),
                _ => self.attach(Node::List {
                    start: None,
                    items: vec![item],
                }),
            },
            Frame::Details {
                index,
                open,
                summary,
                body,
            } => self.attach(Node::Details {
                index,
                open,
                summary: summary.unwrap_or_else(|| Inline::plain("Details")),
                body,
            }),
            Frame::Summary(mut inline) => {
                inline.trim_end();
                match self.stack.last_mut() {
                    Some(Frame::Details { summary, .. }) if summary.is_none() => {
                        *summary = Some(inline);
                    }
                    _ => self.attach(Node::Paragraph(inline)),
                }
            }
            Frame::Table(table) => {
                if !table.head.is_empty() || !table.rows.is_empty() {
                    self.attach(Node::Table(table));
                }
            }
            Frame::Row { head, cells } => {
                let head = head || (!cells.is_empty() && cells.iter().all(|cell| cell.header));
                if let Some(Frame::Table(table)) = self.stack.last_mut()
                    && !cells.is_empty()
                {
                    if head && table.rows.is_empty() {
                        table.head.push(cells);
                    } else {
                        table.rows.push(cells);
                    }
                }
            }
            Frame::Cell(mut cell) => {
                cell.content.trim_end();
                if let Some(Frame::Row { cells, .. }) = self.stack.last_mut() {
                    cells.push(cell);
                }
            }
        }
    }

    fn html(&mut self, source: &str) {
        for token in tokens(source) {
            if let Some(skipped) = &self.skip {
                if matches!(&token, Token::Close(name) if name == skipped) {
                    self.skip = None;
                }
                continue;
            }
            // Preformatted text keeps its spacing, and tags inside it only
            // style it.
            if let Some((code, _)) = &mut self.pre {
                match token {
                    Token::Text(text) => code.push_str(&decode_entities(text)),
                    Token::Open { name, .. } if name == "br" => code.push('\n'),
                    Token::Close(name) if name == "pre" => {
                        if let Some((text, language)) = self.pre.take() {
                            self.code(text, language);
                        }
                    }
                    _ => {}
                }
                continue;
            }
            match token {
                Token::Text(text) => self.push(&collapse_whitespace(&decode_entities(text))),
                Token::Open {
                    name,
                    attributes,
                    closed,
                } => self.open_tag(&name, &attributes, closed),
                Token::Close(name) => self.close_tag(&name),
            }
        }
    }

    fn open_tag(&mut self, name: &str, attributes: &[(String, String)], closed: bool) {
        let attribute = |key: &str| {
            attributes
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.as_str())
        };
        if let Some(level) = heading_level(name) {
            self.paragraph(Some(level));
            return;
        }
        match name {
            "p" => self.paragraph(None),
            "blockquote" => self.open_frame(Frame::Quote(None, Vec::new())),
            "ul" => self.open_frame(Frame::List(None, Vec::new())),
            "ol" => {
                let start = attribute("start")
                    .and_then(|start| start.trim().parse().ok())
                    .unwrap_or(1);
                self.open_frame(Frame::List(Some(start), Vec::new()));
            }
            "li" => {
                // An item ends where the next one starts.
                if matches!(self.stack.last(), Some(Frame::Item(_))) {
                    self.close(Kind::Item);
                }
                self.open_frame(Frame::Item(Item::default()));
            }
            "details" => {
                self.details += 1;
                self.open_frame(Frame::Details {
                    index: self.details,
                    open: attribute("open").is_some(),
                    summary: None,
                    body: Vec::new(),
                });
            }
            "summary" => self.open_frame(Frame::Summary(Inline::default())),
            "table" => self.open_frame(Frame::Table(Table::default())),
            "thead" => self.head = true,
            "tbody" | "tfoot" => self.head = false,
            "tr" => self.open_frame(Frame::Row {
                head: self.head,
                cells: Vec::new(),
            }),
            "th" | "td" => self.open_frame(Frame::Cell(Cell {
                header: name == "th",
                align: attribute("align").and_then(html_alignment),
                content: Inline::default(),
            })),
            "pre" => {
                self.flush();
                let language = attribute("lang").map_or(Language::Plain, language_from_fence);
                self.pre = Some((String::new(), language));
            }
            "br" => self.line_break(),
            "hr" => {
                self.flush();
                self.attach(Node::Rule);
            }
            "img" => {
                self.start_image(attribute("src").unwrap_or(""));
                self.push(&collapse_whitespace(attribute("alt").unwrap_or("")));
                self.end_image();
            }
            "input"
                if attribute("type").is_some_and(|kind| kind.eq_ignore_ascii_case("checkbox")) =>
            {
                self.task(attribute("checked").is_some());
            }
            _ if closed => {}
            "code" | "kbd" | "samp" | "tt" => self.code += 1,
            "strong" | "b" => self.strong += 1,
            "em" | "i" => self.italic += 1,
            "del" | "s" | "strike" => self.strike += 1,
            "a" => {
                let href = attribute("href").filter(|href| safe_link(href));
                self.links.push(href.map(str::to_owned));
            }
            "script" | "style" | "template" | "textarea" | "noscript" => {
                self.skip = Some(name.to_owned());
            }
            _ if breaks_line(name) => self.end_paragraph(),
            _ => {}
        }
    }

    fn close_tag(&mut self, name: &str) {
        if heading_level(name).is_some() || name == "p" || breaks_line(name) {
            self.end_paragraph();
            return;
        }
        match name {
            "blockquote" => self.close(Kind::Quote),
            "ul" | "ol" => self.close(Kind::List),
            "li" => self.close(Kind::Item),
            "details" => self.close(Kind::Details),
            "summary" => self.close(Kind::Summary),
            "table" => self.close(Kind::Table),
            "thead" => self.head = false,
            "tr" => self.close(Kind::Row),
            "th" | "td" => self.close(Kind::Cell),
            "code" | "kbd" | "samp" | "tt" => self.code = self.code.saturating_sub(1),
            "strong" | "b" => self.strong = self.strong.saturating_sub(1),
            "em" | "i" => self.italic = self.italic.saturating_sub(1),
            "del" | "s" | "strike" => self.strike = self.strike.saturating_sub(1),
            "a" => {
                self.links.pop();
            }
            _ => {}
        }
    }
}

fn alignment(alignment: Alignment) -> Option<TextAlign> {
    match alignment {
        Alignment::None => None,
        Alignment::Left => Some(TextAlign::Left),
        Alignment::Center => Some(TextAlign::Center),
        Alignment::Right => Some(TextAlign::Right),
    }
}

fn html_alignment(value: &str) -> Option<TextAlign> {
    match value.trim().to_ascii_lowercase().as_str() {
        "left" => Some(TextAlign::Left),
        "center" => Some(TextAlign::Center),
        "right" => Some(TextAlign::Right),
        _ => None,
    }
}

fn heading_level(name: &str) -> Option<u8> {
    match name.as_bytes() {
        [b'h', level @ b'1'..=b'6'] => Some(level - b'0'),
        _ => None,
    }
}

/// Block elements that only separate what is around them.
fn breaks_line(name: &str) -> bool {
    matches!(
        name,
        "div"
            | "section"
            | "article"
            | "header"
            | "footer"
            | "main"
            | "nav"
            | "aside"
            | "center"
            | "dl"
            | "dt"
            | "dd"
            | "figure"
            | "figcaption"
    )
}

// HTML ------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
enum Token<'a> {
    Text(&'a str),
    Open {
        name: String,
        attributes: Vec<(String, String)>,
        /// Written `<br />`, so nothing follows inside it.
        closed: bool,
    },
    Close(String),
}

/// Splits HTML into text and tags. Comments and declarations are dropped,
/// and a `<` that starts no tag stays text.
fn tokens(source: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let mut text_start = 0;
    let mut at = 0;
    while let Some(offset) = source[at..].find('<') {
        let start = at + offset;
        match tag(source, start) {
            Some((token, end)) => {
                if text_start < start {
                    tokens.push(Token::Text(&source[text_start..start]));
                }
                tokens.extend(token);
                at = end;
                text_start = end;
            }
            None => at = start + 1,
        }
    }
    if text_start < source.len() {
        tokens.push(Token::Text(&source[text_start..]));
    }
    tokens
}

/// The tag starting at `start`, which holds a `<`, and where it ends.
fn tag(source: &str, start: usize) -> Option<(Option<Token<'static>>, usize)> {
    let rest = &source[start..];
    if let Some(comment) = rest.strip_prefix("<!--") {
        let end = comment
            .find("-->")
            .map_or(source.len(), |end| start + 4 + end + 3);
        return Some((None, end));
    }
    if rest.starts_with("<!") || rest.starts_with("<?") {
        let end = rest.find('>').map_or(source.len(), |end| start + end + 1);
        return Some((None, end));
    }
    let closing = rest.starts_with("</");
    let name_start = if closing { 2 } else { 1 };
    let bytes = rest.as_bytes();
    if !bytes.get(name_start)?.is_ascii_alphabetic() {
        return None;
    }
    let mut at = name_start;
    while at < bytes.len() && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'-') {
        at += 1;
    }
    let name = rest[name_start..at].to_ascii_lowercase();
    let mut attributes = Vec::new();
    let mut closed = false;
    loop {
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        match bytes.get(at)? {
            b'>' => {
                at += 1;
                break;
            }
            b'/' => {
                closed = true;
                at += 1;
                continue;
            }
            _ => {}
        }
        let key_start = at;
        while at < bytes.len()
            && !bytes[at].is_ascii_whitespace()
            && !matches!(bytes[at], b'=' | b'>' | b'/')
        {
            at += 1;
        }
        let key = rest[key_start..at].to_ascii_lowercase();
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        let mut value = String::new();
        if bytes.get(at) == Some(&b'=') {
            at += 1;
            while at < bytes.len() && bytes[at].is_ascii_whitespace() {
                at += 1;
            }
            match bytes.get(at)? {
                quote @ (b'"' | b'\'') => {
                    let value_start = at + 1;
                    let length = rest[value_start..].find(*quote as char)?;
                    value = decode_entities(&rest[value_start..value_start + length]);
                    at = value_start + length + 1;
                }
                _ => {
                    let value_start = at;
                    while at < bytes.len() && !bytes[at].is_ascii_whitespace() && bytes[at] != b'>'
                    {
                        at += 1;
                    }
                    value = decode_entities(&rest[value_start..at]);
                }
            }
        }
        if !key.is_empty() {
            attributes.push((key, value));
        }
    }
    let token = if closing {
        Token::Close(name)
    } else {
        Token::Open {
            name,
            attributes,
            closed,
        }
    };
    Some((Some(token), start + at))
}

/// Runs of whitespace read as one space, as HTML lays out text.
fn collapse_whitespace(text: &str) -> String {
    let mut collapsed = String::with_capacity(text.len());
    let mut space = false;
    for character in text.chars() {
        if character.is_ascii_whitespace() {
            space = true;
        } else {
            if space {
                collapsed.push(' ');
                space = false;
            }
            collapsed.push(character);
        }
    }
    if space {
        collapsed.push(' ');
    }
    collapsed
}

fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    let mut decoded = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        decoded.push_str(&rest[..at]);
        rest = &rest[at..];
        let entity = rest[1..]
            .find(';')
            .filter(|end| *end <= 32)
            .and_then(|end| Some((entity(&rest[1..end + 1])?, end + 2)));
        match entity {
            Some((character, length)) => {
                decoded.push(character);
                rest = &rest[length..];
            }
            None => {
                decoded.push('&');
                rest = &rest[1..];
            }
        }
    }
    decoded.push_str(rest);
    decoded
}

fn entity(name: &str) -> Option<char> {
    if let Some(number) = name.strip_prefix('#') {
        let code = match number.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => number.parse().ok()?,
        };
        return char::from_u32(code).filter(|character| *character != '\0');
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "ensp" => '\u{2002}',
        "emsp" => '\u{2003}',
        "thinsp" => '\u{2009}',
        "ndash" => '–',
        "mdash" => '—',
        "hellip" => '…',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "laquo" => '«',
        "raquo" => '»',
        "bull" => '•',
        "middot" => '·',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "deg" => '°',
        "times" => '×',
        "divide" => '÷',
        "plusmn" => '±',
        "ne" => '≠',
        "le" => '≤',
        "ge" => '≥',
        "larr" => '←',
        "rarr" => '→',
        "uarr" => '↑',
        "darr" => '↓',
        "check" => '✓',
        _ => return None,
    })
}

// References --------------------------------------------------------------------

#[derive(Debug, PartialEq)]
struct Reference {
    range: Range<usize>,
    url: String,
    mention: bool,
}

/// Bare URLs, and with a repository its `#123` issues and `@user` mentions,
/// where GitHub would link them: at the start of a word.
fn references(text: &str, repository: Option<&str>) -> Vec<Reference> {
    let host = repository.and_then(|repository| {
        let parts: Vec<&str> = repository.splitn(4, '/').collect();
        (parts.len() == 4).then(|| format!("{}//{}", parts[0], parts[2]))
    });
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(character) = text[at..].chars().next() {
        let starts_word = text[..at]
            .chars()
            .next_back()
            .is_none_or(|before| before.is_whitespace() || "([{<\"'*_~".contains(before));
        if starts_word {
            let rest = &text[at..];
            let reference = if let Some(length) = url_length(rest) {
                let url = &rest[..length];
                let url = if url.starts_with("www.") {
                    format!("https://{url}")
                } else {
                    url.to_owned()
                };
                safe_link(&url).then_some((length, url, false))
            } else if let (Some(repository), Some(length)) = (repository, issue_length(rest)) {
                Some((
                    length,
                    format!("{repository}/issues/{}", &rest[1..length]),
                    false,
                ))
            } else if let (Some(host), Some(length)) = (&host, mention_length(rest)) {
                Some((length, format!("{host}/{}", &rest[1..length]), true))
            } else {
                None
            };
            if let Some((length, url, mention)) = reference {
                found.push(Reference {
                    range: at..at + length,
                    url,
                    mention,
                });
                at += length;
                continue;
            }
        }
        at += character.len_utf8();
    }
    found
}

fn url_length(rest: &str) -> Option<usize> {
    let scheme = ["https://", "http://", "www."].into_iter().find(|scheme| {
        rest.get(..scheme.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(scheme))
    })?;
    let mut end = rest
        .find(|character: char| character.is_whitespace() || character == '<')
        .unwrap_or(rest.len());
    // Trailing punctuation belongs to the sentence, and so does a closing
    // parenthesis the URL never opened.
    while let Some(last) = rest[..end].chars().next_back() {
        let url = &rest[..end];
        if "?!.,:;*_~'\"".contains(last)
            || (last == ')' && url.matches(')').count() > url.matches('(').count())
        {
            end -= last.len_utf8();
        } else {
            break;
        }
    }
    (end > scheme.len()).then_some(end)
}

fn issue_length(rest: &str) -> Option<usize> {
    let digits = rest
        .strip_prefix('#')?
        .bytes()
        .take_while(u8::is_ascii_digit)
        .count();
    let end = 1 + digits;
    (digits > 0 && !ends_word(&rest[end..])).then_some(end)
}

fn mention_length(rest: &str) -> Option<usize> {
    let name = rest.strip_prefix('@')?;
    let length = name
        .bytes()
        .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
        .count();
    let end = 1 + length;
    let valid = (1..=39).contains(&length)
        && !name.starts_with('-')
        && !name[..length].ends_with('-')
        && !ends_word(&rest[end..])
        && !rest[end..].starts_with('/');
    valid.then_some(end)
}

/// Whether `rest` carries on the word before it.
fn ends_word(rest: &str) -> bool {
    rest.starts_with(|character: char| character.is_alphanumeric() || character == '_')
}

// Rendering ---------------------------------------------------------------------

/// Type sizes and spacing of one of the two styles.
#[derive(Clone, Copy)]
struct Metrics {
    text: f32,
    line: f32,
    /// Between paragraphs, headings, and lists.
    gap: f32,
    /// Between the items of a list.
    item_gap: f32,
    /// The column a marker sits in; nested lists step in by as much.
    marker: f32,
    cell_x: f32,
    cell_y: f32,
}

impl Metrics {
    fn of(prose: bool) -> Self {
        if prose {
            Self {
                text: 15.0,
                line: 25.0,
                gap: 16.0,
                item_gap: 6.0,
                marker: 24.0,
                cell_x: 13.0,
                cell_y: 6.0,
            }
        } else {
            Self {
                text: 13.0,
                line: 21.0,
                gap: 12.0,
                item_gap: 4.0,
                marker: 20.0,
                cell_x: 10.0,
                cell_y: 4.0,
            }
        }
    }
}

/// Where a block sits: how many lists hold it, and whether a quote does.
#[derive(Clone, Copy, Default)]
struct Scope {
    depth: usize,
    quoted: bool,
}

struct Renderer<'a> {
    id: SharedString,
    metrics: Metrics,
    next: usize,
    window: &'a mut Window,
    cx: &'a mut App,
}

impl Renderer<'_> {
    fn next_id(&mut self) -> SharedString {
        self.next += 1;
        SharedString::from(format!("{}-{}", self.id, self.next))
    }

    fn blocks(&mut self, nodes: Vec<Node>, scope: Scope) -> Vec<AnyElement> {
        nodes
            .into_iter()
            .map(|node| self.block(node, scope))
            .collect()
    }

    fn block(&mut self, node: Node, scope: Scope) -> AnyElement {
        let color: Hsla = if scope.quoted {
            colors().muted.into()
        } else {
            colors().foreground.into()
        };
        match node {
            Node::Paragraph(inline) => div()
                .min_w(px(0.0))
                .child(self.text(inline, FontWeight::NORMAL, color))
                .into_any_element(),
            Node::Heading(level, inline) => self.heading(level, inline, scope),
            Node::Code { text, language } => code_block(text, language),
            Node::Quote(body) => div()
                .min_w(px(0.0))
                .pl(px(12.0))
                .border_l_3()
                .border_color(rule_color())
                .flex()
                .flex_col()
                .gap(px(self.metrics.gap))
                .children(self.blocks(
                    body,
                    Scope {
                        quoted: true,
                        ..scope
                    },
                ))
                .into_any_element(),
            Node::Alert(kind, body) => self.alert(kind, body, scope),
            Node::List { start, items } => self.list(start, items, scope),
            Node::Table(table) => self.table(table, color),
            Node::Details {
                index,
                open,
                summary,
                body,
            } => self.details(index, open, summary, body, scope),
            Node::Rule => div().h(px(2.0)).bg(rule_color()).into_any_element(),
        }
    }

    fn text(&mut self, inline: Inline, weight: FontWeight, color: Hsla) -> AnyElement {
        let id = self.next_id();
        let runs = text_runs(&inline, weight, color);
        let (ranges, urls): (Vec<_>, Vec<_>) = inline.links.into_iter().unzip();
        InteractiveText::new(id, StyledText::new(inline.text).with_runs(runs))
            .on_click(ranges, move |index, _, cx| {
                if let Some(url) = urls.get(index) {
                    cx.open_url(url);
                }
            })
            .into_any_element()
    }

    /// Sized as GitHub sizes them against the text; the top two levels
    /// rule off their section.
    fn heading(&mut self, level: u8, inline: Inline, scope: Scope) -> AnyElement {
        let scale = match level {
            1 => 1.6,
            2 => 1.35,
            3 => 1.15,
            4 => 1.0,
            5 => 0.9,
            _ => 0.85,
        };
        let size = (self.metrics.text * scale).round();
        let color: Hsla = if scope.quoted || level >= 6 {
            colors().muted.into()
        } else {
            colors().foreground.into()
        };
        let text = self.text(inline, FontWeight::SEMIBOLD, color);
        div()
            .min_w(px(0.0))
            .mt(px(self.metrics.gap / 2.0))
            .text_size(px(size))
            .line_height(px((size * 1.3).round()))
            .when(level <= 2, |heading| {
                heading
                    .pb(px(6.0))
                    .border_b_1()
                    .border_color(colors().border_subtle)
            })
            .child(text)
            .into_any_element()
    }

    /// GitHub's callouts: a rule and a title in the alert's color.
    fn alert(&mut self, kind: BlockQuoteKind, body: Vec<Node>, scope: Scope) -> AnyElement {
        let (label, color) = match kind {
            BlockQuoteKind::Note => ("Note", colors().accent),
            BlockQuoteKind::Tip => ("Tip", colors().success),
            BlockQuoteKind::Important => ("Important", mix(colors().accent, colors().danger, 0.5)),
            BlockQuoteKind::Warning => ("Warning", colors().warning),
            BlockQuoteKind::Caution => ("Caution", colors().danger),
        };
        div()
            .min_w(px(0.0))
            .pl(px(12.0))
            .border_l_3()
            .border_color(color)
            .flex()
            .flex_col()
            .gap(px(self.metrics.item_gap))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(color)
                    .child(label),
            )
            .children(self.blocks(body, scope))
            .into_any_element()
    }

    fn list(&mut self, start: Option<u64>, items: Vec<Item>, scope: Scope) -> AnyElement {
        let metrics = self.metrics;
        let depth = scope.depth + 1;
        let mut rows = Vec::new();
        for (index, item) in items.into_iter().enumerate() {
            // An empty item shows nothing, not a stray marker.
            if item.body.is_empty() && item.task.is_none() {
                continue;
            }
            let marker = match (item.task, start) {
                (Some(checked), _) => Marker::Task(checked),
                (None, Some(start)) => Marker::Number(start + index as u64),
                (None, None) => Marker::Bullet,
            };
            let body = self.blocks(item.body, Scope { depth, ..scope });
            rows.push(
                div()
                    .flex()
                    .items_start()
                    .child(marker_slot(marker, depth, &metrics))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .gap(px(metrics.item_gap))
                            .children(body),
                    )
                    .into_any_element(),
            );
        }
        div()
            .min_w(px(0.0))
            .flex()
            .flex_col()
            .gap(px(metrics.item_gap))
            .children(rows)
            .into_any_element()
    }

    /// Columns as wide as their widest cell, up to a limit past which the
    /// text wraps. In a narrower pane every column gives way in proportion.
    fn table(&mut self, table: Table, color: Hsla) -> AnyElement {
        let metrics = self.metrics;
        let Table { aligns, head, rows } = table;
        let columns = head.iter().chain(&rows).map(Vec::len).max().unwrap_or(0);
        let mut widths = vec![0.0_f32; columns];
        for row in head.iter().chain(&rows) {
            for (column, cell) in row.iter().enumerate() {
                let width = self.measure(&cell.content, cell_weight(cell));
                widths[column] = widths[column].max(width);
            }
        }
        // The padding, the rule between columns, and a pixel of slack so
        // shaping never wraps a cell measured to fit.
        for width in &mut widths {
            *width = (width.ceil() + 2.0 * metrics.cell_x + 2.0).min(MAX_COLUMN_WIDTH);
        }
        let total: f32 = widths.iter().sum::<f32>() + 2.0;
        let rule = rule_color();
        let stripe = Hsla::from(colors().foreground).opacity(0.03);
        let head_rows = head.len();
        let mut elements = Vec::new();
        for (index, row) in head.into_iter().chain(rows).enumerate() {
            let striped = index >= head_rows && (index - head_rows) % 2 == 1;
            let mut cells = row.into_iter();
            let mut element = div()
                .flex()
                .when(index > 0, |row| row.border_t_1().border_color(rule))
                .when(striped, |row| row.bg(stripe));
            for (column, width) in widths.iter().enumerate() {
                let cell = cells.next().unwrap_or_default();
                let align = cell
                    .align
                    .or(aligns.get(column).copied().flatten())
                    .unwrap_or(if cell.header {
                        TextAlign::Center
                    } else {
                        TextAlign::Left
                    });
                let weight = cell_weight(&cell);
                let text = self.text(cell.content, weight, color);
                element = element.child(
                    div()
                        .flex_basis(px(*width))
                        .min_w(px(0.0))
                        .px(px(metrics.cell_x))
                        .py(px(metrics.cell_y))
                        .when(column > 0, |cell| cell.border_l_1().border_color(rule))
                        .text_align(align)
                        .child(text),
                );
            }
            elements.push(element);
        }
        div()
            .max_w(px(total))
            .flex()
            .flex_col()
            .border_1()
            .border_color(rule)
            .children(elements)
            .into_any_element()
    }

    /// How wide `inline` is set on one line, its widest when it breaks.
    fn measure(&self, inline: &Inline, weight: FontWeight) -> f32 {
        let size = px(self.metrics.text);
        let text_system = self.cx.text_system().clone();
        let mut widest = 0.0_f32;
        let mut line = 0.0_f32;
        let mut at = 0;
        for run in text_runs(inline, weight, colors().foreground.into()) {
            let font_id = text_system.resolve_font(&run.font);
            for character in inline.text[at..at + run.len].chars() {
                if character == '\n' {
                    widest = widest.max(line);
                    line = 0.0;
                } else if let Ok(advance) = text_system.advance(font_id, size, character) {
                    line += f32::from(advance.width);
                }
            }
            at += run.len;
        }
        widest.max(line)
    }

    /// The summary line, which folds the rest away; closed until clicked,
    /// unless written open.
    fn details(
        &mut self,
        index: usize,
        open: bool,
        summary: Inline,
        body: Vec<Node>,
        scope: Scope,
    ) -> AnyElement {
        let metrics = self.metrics;
        let key = SharedString::from(format!("{}-details-{index}", self.id));
        let state = self
            .window
            .use_keyed_state(ElementId::Name(key.clone()), self.cx, |_, _| open);
        let expanded = *state.read(self.cx);
        let color: Hsla = if scope.quoted {
            colors().muted.into()
        } else {
            colors().foreground.into()
        };
        let summary = self.text(summary, FontWeight::NORMAL, color);
        let body = expanded.then(|| self.blocks(body, scope));
        div()
            .min_w(px(0.0))
            .flex()
            .flex_col()
            .gap(px(metrics.gap * 0.75))
            .child(
                div()
                    .id(key)
                    .flex()
                    .items_start()
                    .gap(px(6.0))
                    .cursor_pointer()
                    .on_click(move |_, _, cx| {
                        state.update(cx, |open, cx| {
                            *open = !*open;
                            cx.notify();
                        });
                    })
                    .child(
                        div()
                            .flex_none()
                            .h(px(metrics.line))
                            .flex()
                            .items_center()
                            .child(
                                svg()
                                    .path(if expanded {
                                        "chrome-icons/chevron-down.svg"
                                    } else {
                                        "chrome-icons/chevron-right.svg"
                                    })
                                    .size(px((metrics.text * 0.8).round()))
                                    .text_color(colors().muted),
                            ),
                    )
                    .child(div().flex_1().min_w(px(0.0)).child(summary)),
            )
            .children(body.into_iter().flatten())
            .into_any_element()
    }
}

fn cell_weight(cell: &Cell) -> FontWeight {
    if cell.header {
        FontWeight::SEMIBOLD
    } else {
        FontWeight::NORMAL
    }
}

/// Quotes, rules, and table borders: firmer than a hairline.
fn rule_color() -> Hsla {
    mix(colors().background, colors().foreground, 0.2).into()
}

fn code_block(text: String, language: Language) -> AnyElement {
    let highlights = highlight_code(&text, language);
    div()
        .min_w(px(0.0))
        .px_3()
        .py(px(10.0))
        .font_family(MONO_FONT)
        .text_size(px(12.0))
        .line_height(px(19.0))
        .rounded(px(8.0))
        .border_1()
        .border_color(colors().border_subtle)
        .bg(surface_tint(
            mix(colors().background, colors().foreground, 0.035),
            colors().background,
        ))
        .child(StyledText::new(text).with_highlights(highlights))
        .into_any_element()
}

/// The marker's column, as tall as the item's first line so the marker
/// centers on it.
fn marker_slot(marker: Marker, depth: usize, metrics: &Metrics) -> AnyElement {
    let slot = div()
        .flex_none()
        .min_w(px(metrics.marker))
        .h(px(metrics.line))
        .flex()
        .items_center();
    let muted = colors().muted;
    match marker {
        // Filled, then hollow, then square, as lists nest.
        Marker::Bullet => {
            let size = (metrics.text * 0.38).round();
            let dot = div().size(px(size)).flex_none();
            let dot = match depth {
                1 => dot.rounded_full().bg(muted),
                2 => dot.rounded_full().border_1().border_color(muted),
                _ => dot.rounded(px(1.0)).bg(muted),
            };
            slot.pl(px(4.0)).child(dot)
        }
        Marker::Number(number) => slot
            .pr(px(6.0))
            .text_color(muted)
            .child(format!("{number}.")),
        Marker::Task(checked) => {
            let size = metrics.text;
            let check = div()
                .size(px(size))
                .flex_none()
                .rounded(px(4.0))
                .flex()
                .items_center()
                .justify_center();
            slot.child(if checked {
                check.bg(colors().accent).child(
                    svg()
                        .path("chrome-icons/check.svg")
                        .size(px(size - 4.0))
                        .text_color(content_on(colors().accent)),
                )
            } else {
                check
                    .border_1()
                    .border_color(Hsla::from(colors().foreground).opacity(0.35))
            })
        }
    }
    .into_any_element()
}

/// The runs that set `inline`: the interface font, or the monospaced one
/// for code, in the block's weight and color unless a span says otherwise.
fn text_runs(inline: &Inline, weight: FontWeight, color: Hsla) -> Vec<TextRun> {
    let run = |len: usize, style: Style| TextRun {
        len,
        font: Font {
            family: if style.code { MONO_FONT } else { UI_FONT }.into(),
            weight: if style.strong {
                FontWeight::SEMIBOLD
            } else {
                weight
            },
            style: if style.italic {
                FontStyle::Italic
            } else {
                FontStyle::Normal
            },
            ..font(UI_FONT)
        },
        color: if style.link {
            colors().accent.into()
        } else {
            color
        },
        background_color: style
            .code
            .then(|| Hsla::from(colors().foreground).opacity(0.08)),
        underline: None,
        strikethrough: style.strike.then_some(StrikethroughStyle {
            thickness: px(1.0),
            color: None,
        }),
    };
    let mut runs = Vec::with_capacity(inline.spans.len() + 1);
    let mut at = 0;
    for (range, style) in &inline.spans {
        if range.start < at || range.end > inline.text.len() {
            continue;
        }
        if range.start > at {
            runs.push(run(range.start - at, Style::default()));
        }
        runs.push(run(range.len(), *style));
        at = range.end;
    }
    if at < inline.text.len() {
        runs.push(run(inline.text.len() - at, Style::default()));
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paragraph(node: &Node) -> &Inline {
        match node {
            Node::Paragraph(inline) | Node::Heading(_, inline) => inline,
            other => panic!("expected text, got {other:?}"),
        }
    }

    fn list(node: &Node) -> (Option<u64>, &[Item]) {
        match node {
            Node::List { start, items } => (*start, items),
            other => panic!("expected a list, got {other:?}"),
        }
    }

    fn table(node: &Node) -> &Table {
        match node {
            Node::Table(table) => table,
            other => panic!("expected a table, got {other:?}"),
        }
    }

    fn cells(row: &[Cell]) -> Vec<&str> {
        row.iter().map(|cell| cell.content.text.as_str()).collect()
    }

    #[test]
    fn list_items_hold_their_text_and_inline_code_its_style() {
        let parsed = parse("- uno\n\n- **dos** `code`\n", None);
        let (start, items) = list(&parsed[0]);
        assert_eq!(start, None);
        assert_eq!(paragraph(&items[0].body[0]).text, "uno");
        let second = paragraph(&items[1].body[0]);
        assert_eq!(second.text, "dos code");
        assert!(
            second
                .spans
                .iter()
                .any(|(range, style)| *range == (4..8) && style.code)
        );
        assert!(second.spans.iter().any(|(_, style)| style.strong));
    }

    #[test]
    fn task_items_become_checkboxes() {
        let parsed = parse("- [ ] uno\n- [x] dos\n", None);
        let (_, items) = list(&parsed[0]);
        assert_eq!(items[0].task, Some(false));
        assert_eq!(paragraph(&items[0].body[0]).text, "uno");
        assert_eq!(items[1].task, Some(true));
    }

    #[test]
    fn ordered_lists_keep_their_start_and_nest() {
        let parsed = parse("3. one\n4. two\n   - nested\n", None);
        let (start, items) = list(&parsed[0]);
        assert_eq!(start, Some(3));
        assert_eq!(items.len(), 2);
        let (nested_start, nested) = list(&items[1].body[1]);
        assert_eq!(nested_start, None);
        assert_eq!(paragraph(&nested[0].body[0]).text, "nested");
    }

    #[test]
    fn an_items_later_paragraphs_stay_under_it_and_the_list_ends_cleanly() {
        let parsed = parse("- first\n\n  second\n\n-\n\nafter\n", None);
        let (_, items) = list(&parsed[0]);
        let texts: Vec<_> = items[0]
            .body
            .iter()
            .map(|node| paragraph(node).text.as_str())
            .collect();
        assert_eq!(texts, ["first", "second"]);
        assert!(items[1].body.is_empty());
        assert_eq!(paragraph(&parsed[1]).text, "after");
    }

    #[test]
    fn inline_code_runs_switch_to_the_monospaced_font_and_cover_the_text() {
        let parsed = parse("use `cargo test` now", None);
        let inline = paragraph(&parsed[0]);
        let runs = text_runs(inline, FontWeight::NORMAL, colors().foreground.into());
        assert_eq!(
            runs.iter().map(|run| run.len).sum::<usize>(),
            inline.text.len()
        );
        let families: Vec<_> = runs.iter().map(|run| run.font.family.as_ref()).collect();
        assert_eq!(families, [UI_FONT, MONO_FONT, UI_FONT]);
    }

    #[test]
    fn fenced_code_keeps_its_language_without_a_trailing_line() {
        let parsed = parse("```rust\nlet total = 1;\n```\n", None);
        assert_eq!(
            parsed,
            [Node::Code {
                text: "let total = 1;".into(),
                language: language_from_fence("rust"),
            }]
        );
    }

    #[test]
    fn quotes_nest_and_hold_headings_code_and_lists() {
        let parsed = parse(
            "> outer\n>\n>> inner\n>\n> # Heading\n> - item\n\nplain\n",
            None,
        );
        let Node::Quote(body) = &parsed[0] else {
            panic!("expected a quote");
        };
        assert_eq!(paragraph(&body[0]).text, "outer");
        assert!(matches!(&body[1], Node::Quote(inner) if paragraph(&inner[0]).text == "inner"));
        assert!(matches!(&body[2], Node::Heading(1, _)));
        assert!(matches!(&body[3], Node::List { .. }));
        assert_eq!(paragraph(&parsed[1]).text, "plain");
    }

    #[test]
    fn alerts_keep_their_kind() {
        let parsed = parse("> [!WARNING]\n> Breaks the build.\n", None);
        assert!(matches!(
            &parsed[0],
            Node::Alert(BlockQuoteKind::Warning, body) if paragraph(&body[0]).text == "Breaks the build."
        ));
    }

    #[test]
    fn markdown_tables_keep_their_cells_alignment_and_links() {
        let parsed = parse(
            "| Package | From | To |\n| :--- | ---: | --- |\n| [base64](https://github.com/marshallpierce/rust-base64) | `0.22.1` | `0.23.1` |\n",
            None,
        );
        let table = table(&parsed[0]);
        assert_eq!(
            table.aligns,
            [Some(TextAlign::Left), Some(TextAlign::Right), None]
        );
        assert_eq!(cells(&table.head[0]), ["Package", "From", "To"]);
        assert!(table.head[0].iter().all(|cell| cell.header));
        let row = &table.rows[0];
        assert_eq!(cells(row), ["base64", "0.22.1", "0.23.1"]);
        assert_eq!(
            row[0].content.links,
            [(
                0..6,
                "https://github.com/marshallpierce/rust-base64".to_owned()
            )]
        );
        assert!(row[1].content.spans[0].1.code);
    }

    #[test]
    fn dependabot_html_folds_into_details_with_quotes_lists_and_code() {
        let parsed = parse(
            concat!(
                "Updates `base64` from 0.22.1 to 0.23.1\n",
                "<details>\n",
                "<summary>Changelog</summary>\n",
                "<p><em>Sourced from <a href=\"https://github.com/x/y/blob/master/NOTES.md\">base64's changelog</a>.</em></p>\n",
                "<blockquote>\n",
                "<h1>0.23.1</h1>\n",
                "<ul>\n",
                "<li>Make the tests build &amp; pass</li>\n",
                "<li>Add <code>simd-unsafe</code>\nfeature</li>\n",
                "</ul>\n",
                "</blockquote>\n",
                "</details>\n",
                "<br />\n",
            ),
            None,
        );
        assert_eq!(
            paragraph(&parsed[0]).text,
            "Updates base64 from 0.22.1 to 0.23.1"
        );
        let Node::Details {
            index,
            open,
            summary,
            body,
        } = &parsed[1]
        else {
            panic!("expected details, got {:?}", parsed[1]);
        };
        assert_eq!((*index, *open), (1, false));
        assert_eq!(summary.text, "Changelog");
        let sourced = paragraph(&body[0]);
        assert_eq!(sourced.text, "Sourced from base64's changelog.");
        assert!(sourced.spans.iter().all(|(_, style)| style.italic));
        assert_eq!(sourced.links[0].0, 13..31);
        let Node::Quote(quote) = &body[1] else {
            panic!("expected a quote, got {:?}", body[1]);
        };
        assert!(matches!(&quote[0], Node::Heading(1, title) if title.text == "0.23.1"));
        let (_, items) = list(&quote[1]);
        assert_eq!(
            paragraph(&items[0].body[0]).text,
            "Make the tests build & pass"
        );
        let second = paragraph(&items[1].body[0]);
        assert_eq!(second.text, "Add simd-unsafe feature");
        assert!(
            second
                .spans
                .iter()
                .any(|(range, style)| *range == (4..15) && style.code)
        );
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn markdown_inside_details_and_open_details_are_kept() {
        let parsed = parse(
            "<details open>\n<summary><b>Commands</b></summary>\n\n- one\n- two\n\n</details>\n",
            None,
        );
        let Node::Details {
            open,
            summary,
            body,
            ..
        } = &parsed[0]
        else {
            panic!("expected details");
        };
        assert!(*open);
        assert_eq!(summary.text, "Commands");
        assert!(summary.spans.iter().all(|(_, style)| style.strong));
        assert_eq!(list(&body[0]).1.len(), 2);
    }

    #[test]
    fn html_tables_find_their_header_row() {
        let parsed = parse(
            "<table>\n<tr><th>Name</th><th align=\"right\">Size</th></tr>\n<tr><td>a</td><td>1 KB</td></tr>\n</table>\n",
            None,
        );
        let table = table(&parsed[0]);
        assert_eq!(cells(&table.head[0]), ["Name", "Size"]);
        assert_eq!(table.head[0][1].align, Some(TextAlign::Right));
        assert_eq!(cells(&table.rows[0]), ["a", "1 KB"]);
    }

    #[test]
    fn html_pre_keeps_its_lines_and_scripts_never_show() {
        let parsed = parse(
            "<pre lang=\"rust\">fn main() {\n    run();\n}</pre>\n\n<script>alert(1)</script>\n\nafter\n",
            None,
        );
        assert_eq!(
            parsed[0],
            Node::Code {
                text: "fn main() {\n    run();\n}".into(),
                language: language_from_fence("rust"),
            }
        );
        assert_eq!(parsed.len(), 2);
        assert_eq!(paragraph(&parsed[1]).text, "after");
    }

    #[test]
    fn bare_urls_issues_and_mentions_link_like_github() {
        let parsed = parse(
            "Fixes #12, see https://example.com/a_b_c). Thanks @octo-cat! Not mail@example.com or C#1.",
            Some("https://github.com/owner/repo"),
        );
        let inline = paragraph(&parsed[0]);
        let links: Vec<_> = inline
            .links
            .iter()
            .map(|(range, url)| (&inline.text[range.clone()], url.as_str()))
            .collect();
        assert_eq!(
            links,
            [
                ("#12", "https://github.com/owner/repo/issues/12"),
                ("https://example.com/a_b_c", "https://example.com/a_b_c"),
                ("@octo-cat", "https://github.com/octo-cat"),
            ]
        );
    }

    #[test]
    fn links_are_only_followed_when_safe() {
        let parsed = parse(
            "[web](https://example.com) [bad](javascript:alert(1))",
            None,
        );
        let inline = paragraph(&parsed[0]);
        assert_eq!(inline.text, "web bad");
        assert_eq!(inline.links, [(0..3, "https://example.com".to_owned())]);
        assert!(!safe_link("file:///etc/passwd"));
    }

    #[test]
    fn images_show_their_alt_text_linked() {
        let parsed = parse(
            "[![Compatibility](https://img.example/badge.svg)](https://docs.example/score) ![](https://img.example/x.png)",
            None,
        );
        let inline = paragraph(&parsed[0]);
        assert_eq!(inline.text, "Compatibility Image");
        assert_eq!(
            inline.links,
            [
                (0..13, "https://docs.example/score".to_owned()),
                (14..19, "https://img.example/x.png".to_owned()),
            ]
        );
    }

    #[test]
    fn entities_decode_and_stray_angle_brackets_stay_text() {
        assert_eq!(
            decode_entities("a &amp; b &#x2192; &copy; &bogus;"),
            "a & b → © &bogus;"
        );
        assert_eq!(
            tokens("1 < 2 <b>x</b>"),
            [
                Token::Text("1 < 2 "),
                Token::Open {
                    name: "b".into(),
                    attributes: Vec::new(),
                    closed: false,
                },
                Token::Text("x"),
                Token::Close("b".into()),
            ]
        );
    }

    #[test]
    fn references_need_a_repository_and_a_word_start() {
        assert!(references("#12 @octo", None).is_empty());
        assert_eq!(
            repository_url("https://github.com/owner/repo/pull/47").as_deref(),
            Some("https://github.com/owner/repo")
        );
        assert_eq!(repository_url("https://github.com/owner"), None);
    }
}
