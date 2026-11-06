//! CommonMark rendered as native text. Remote HTML is never executed.

use crate::ui::syntax::{Language, highlight_code, language_from_fence};
use crate::ui::theme::{MONO_FONT, colors, content_on, mix, surface_tint};
use gpui::{
    AnyElement, Font, FontStyle, FontWeight, HighlightStyle, Hsla, InteractiveText, SharedString,
    StyledText, TextRun, div, font, prelude::*, px, svg,
};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use std::ops::Range;

/// The family the window's root sets. Text that mixes in monospaced runs
/// restates it for the rest of the line.
const UI_FONT: &str = ".SystemUIFont";

/// What leads a list item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Marker {
    Bullet,
    Number(u64),
    Task(bool),
}

#[derive(Default)]
struct Block {
    text: String,
    runs: Vec<(Range<usize>, HighlightStyle)>,
    /// Inline code, set in the monospaced font.
    code_spans: Vec<Range<usize>>,
    links: Vec<(Range<usize>, String)>,
    heading: u8,
    code: bool,
    quote: bool,
    rule: bool,
    /// How many lists hold the block; 0 outside any.
    depth: usize,
    /// The marker of the item this block opens. A later paragraph or code
    /// block of the same item has none and lines up with the item's text.
    marker: Option<Marker>,
}

/// A block that is not an item's first line takes the depth of the lists
/// around it, so it stays indented under its item.
fn begin(block: &mut Block, depth: usize) {
    if block.text.is_empty() && block.marker.is_none() {
        block.depth = depth;
    }
}

fn blocks(text: &str) -> Vec<Block> {
    let mut output = Vec::new();
    let mut block = Block::default();
    let mut strong = 0;
    let mut emphasis = 0;
    let mut strike = 0;
    let mut item_prefix = false;
    let mut link: Option<String> = None;
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut quote_depth = 0usize;
    let mut code_language: Option<Language> = None;
    // An empty block is dropped whole, so an empty item's marker never
    // leaks into the block after it.
    let flush = |output: &mut Vec<Block>, block: &mut Block| {
        if !block.text.is_empty() || block.rule {
            output.push(std::mem::take(block));
        } else {
            *block = Block::default();
        }
    };
    for event in Parser::new_ext(
        text,
        Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH,
    ) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                flush(&mut output, &mut block);
                block.heading = level as u8;
                block.quote = quote_depth > 0;
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                flush(&mut output, &mut block);
                block.code = true;
                block.quote = quote_depth > 0;
                code_language = Some(match kind {
                    CodeBlockKind::Fenced(info) => language_from_fence(
                        info.split(|c: char| c.is_whitespace() || c == ',' || c == '{')
                            .next()
                            .unwrap_or(""),
                    ),
                    CodeBlockKind::Indented => Language::Plain,
                });
            }
            Event::End(TagEnd::CodeBlock) => {
                // The closing newline would paint an empty last line.
                let code = block.text.trim_end_matches('\n').len();
                block.text.truncate(code);
                block.runs =
                    highlight_code(&block.text, code_language.take().unwrap_or(Language::Plain));
                block.code_spans.clear();
                block.links.clear();
                flush(&mut output, &mut block);
                item_prefix = false;
            }
            Event::Start(Tag::Paragraph) => {
                if !item_prefix {
                    flush(&mut output, &mut block);
                }
                block.quote = quote_depth > 0;
            }
            Event::Start(Tag::BlockQuote(_)) => {
                quote_depth += 1;
                block.quote = true;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                flush(&mut output, &mut block);
                quote_depth -= 1;
            }
            Event::Start(Tag::List(start)) => lists.push(start),
            Event::End(TagEnd::List(_)) => {
                lists.pop();
            }
            Event::Start(Tag::Item) => {
                flush(&mut output, &mut block);
                item_prefix = true;
                block.quote = quote_depth > 0;
                block.depth = lists.len();
                block.marker = Some(match lists.last_mut() {
                    Some(Some(number)) => {
                        let marker = Marker::Number(*number);
                        *number += 1;
                        marker
                    }
                    _ => Marker::Bullet,
                });
            }
            Event::Start(Tag::Strikethrough) => strike += 1,
            Event::End(TagEnd::Strikethrough) => strike -= 1,
            Event::Start(Tag::Strong) => strong += 1,
            Event::End(TagEnd::Strong) => strong -= 1,
            Event::Start(Tag::Emphasis) => emphasis += 1,
            Event::End(TagEnd::Emphasis) => emphasis -= 1,
            Event::Start(Tag::Link { dest_url, .. })
            | Event::Start(Tag::Image { dest_url, .. }) => {
                link = Some(dest_url.into_string());
            }
            Event::End(TagEnd::Link) | Event::End(TagEnd::Image) => link = None,
            Event::Code(text) => {
                item_prefix = false;
                begin(&mut block, lists.len());
                let start = block.text.len();
                block.text.push_str(&text);
                let range = start..block.text.len();
                if !range.is_empty() {
                    block.runs.push((
                        range.clone(),
                        HighlightStyle {
                            background_color: Some(Hsla::from(colors().foreground).opacity(0.08)),
                            font_weight: (strong > 0).then_some(FontWeight::SEMIBOLD),
                            color: link.as_ref().map(|_| colors().accent.into()),
                            ..Default::default()
                        },
                    ));
                    block.code_spans.push(range.clone());
                    if let Some(url) = link.as_ref().filter(|url| safe_link(url)) {
                        block.links.push((range, url.clone()));
                    }
                }
            }
            Event::Text(text) => {
                item_prefix = false;
                begin(&mut block, lists.len());
                let start = block.text.len();
                block.text.push_str(&text);
                let range = start..block.text.len();
                let style = HighlightStyle {
                    font_weight: (strong > 0).then_some(FontWeight::SEMIBOLD),
                    font_style: (emphasis > 0).then_some(FontStyle::Italic),
                    strikethrough: (strike > 0).then_some(gpui::StrikethroughStyle::default()),
                    color: link.as_ref().map(|_| colors().accent.into()),
                    ..Default::default()
                };
                if !range.is_empty() {
                    block.runs.push((range.clone(), style));
                }
                if let Some(url) = link.as_ref().filter(|url| safe_link(url)) {
                    block.links.push((range, url.clone()));
                }
            }
            Event::SoftBreak => block.text.push(' '),
            Event::HardBreak => block.text.push('\n'),
            // The item's marker becomes its checkbox.
            Event::TaskListMarker(checked) => {
                if block.marker.is_some() && block.text.is_empty() {
                    block.marker = Some(Marker::Task(checked));
                } else {
                    block.text.push_str(if checked { "☑ " } else { "☐ " });
                }
            }
            Event::Rule => {
                flush(&mut output, &mut block);
                block.rule = true;
                block.quote = quote_depth > 0;
                flush(&mut output, &mut block);
            }
            Event::Start(Tag::TableHead) => {
                flush(&mut output, &mut block);
                strong += 1;
            }
            Event::End(TagEnd::TableHead) => {
                strong -= 1;
                flush(&mut output, &mut block);
            }
            Event::End(TagEnd::TableCell) => block.text.push_str("    "),
            Event::End(
                TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item | TagEnd::TableRow,
            ) => {
                flush(&mut output, &mut block);
                item_prefix = false;
            }
            _ => {}
        }
    }
    flush(&mut output, &mut block);
    output
}

pub fn safe_link(url: &str) -> bool {
    (url.starts_with("https://") || url.starts_with("http://"))
        && !url.chars().any(char::is_control)
}

pub fn markdown(id: &str, text: &str) -> AnyElement {
    render_markdown(id, text, false)
}

/// More spacious typography for the Inbox's primary description.
pub fn markdown_prose(id: &str, text: &str) -> AnyElement {
    render_markdown(id, text, true)
}

/// Type sizes and spacing of one of the two styles.
struct Metrics {
    text: f32,
    line: f32,
    /// Between paragraphs, headings, and lists.
    gap: f32,
    /// Between the items of a list.
    item_gap: f32,
    /// The column a marker sits in; nested lists step in by as much.
    marker: f32,
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
            }
        } else {
            Self {
                text: 13.0,
                line: 21.0,
                gap: 12.0,
                item_gap: 4.0,
                marker: 20.0,
            }
        }
    }
}

fn render_markdown(id: &str, text: &str, prose: bool) -> AnyElement {
    let metrics = Metrics::of(prose);
    // A list's items sit closer together than the paragraphs around it.
    let mut children = Vec::new();
    let mut list = Vec::new();
    for (index, block) in blocks(text).into_iter().enumerate() {
        let depth = block.depth;
        if depth == 0 && !list.is_empty() {
            children.push(list_group(std::mem::take(&mut list), &metrics));
        }
        let element = render_block(id, index, block, &metrics);
        if depth > 0 {
            list.push(element);
        } else {
            children.push(element);
        }
    }
    if !list.is_empty() {
        children.push(list_group(list, &metrics));
    }
    div()
        .flex()
        .flex_col()
        .gap(px(metrics.gap))
        .text_size(px(metrics.text))
        .line_height(px(metrics.line))
        .text_color(colors().foreground)
        .children(children)
        .into_any_element()
}

fn list_group(items: Vec<AnyElement>, metrics: &Metrics) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(metrics.item_gap))
        .children(items)
        .into_any_element()
}

fn render_block(id: &str, index: usize, block: Block, metrics: &Metrics) -> AnyElement {
    if block.rule {
        return div()
            .h(px(1.0))
            .bg(colors().border_subtle)
            .into_any_element();
    }
    let color: Hsla = if block.quote {
        colors().muted.into()
    } else {
        colors().foreground.into()
    };
    let weight = if block.heading > 0 {
        FontWeight::SEMIBOLD
    } else {
        FontWeight::NORMAL
    };
    let urls: Vec<_> = block.links.iter().map(|(_, url)| url.clone()).collect();
    let ranges = block.links.into_iter().map(|(range, _)| range).collect();
    // Highlights only restyle the inherited font; inline code needs runs of
    // its own to switch to the monospaced one.
    let styled = if block.code_spans.is_empty() {
        StyledText::new(block.text).with_highlights(block.runs)
    } else {
        let runs = text_runs(&block.text, &block.runs, &block.code_spans, weight, color);
        StyledText::new(block.text).with_runs(runs)
    };
    let text = InteractiveText::new(SharedString::from(format!("{id}-{index}")), styled).on_click(
        ranges,
        move |index, _, cx| {
            if let Some(url) = urls.get(index) {
                cx.open_url(url);
            }
        },
    );
    let content = div()
        .min_w(px(0.0))
        .when(block.heading > 0, |row| {
            row.font_weight(FontWeight::SEMIBOLD)
                .text_size(px(match block.heading {
                    1 => 22.0,
                    2 => 18.0,
                    _ => 15.0,
                }))
                .mt_2()
        })
        .when(block.code, |row| {
            row.px_3()
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
        })
        .when(block.quote, |row| {
            row.pl_3()
                .border_l_2()
                .border_color(colors().border_subtle)
                .text_color(colors().muted)
        })
        .child(text);
    if block.depth == 0 {
        return content.into_any_element();
    }
    // Wrapped lines hang under the item's text, not under its marker.
    let indent = (block.depth - 1) as f32 * metrics.marker;
    let row = div().flex().items_start();
    match block.marker {
        Some(marker) => row
            .pl(px(indent))
            .child(marker_slot(marker, block.depth, metrics))
            .child(content.flex_1()),
        None => row.pl(px(indent + metrics.marker)).child(content.flex_1()),
    }
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

/// Runs for text that mixes the interface font with inline code. The
/// highlights come in order and never overlap; the gaps between them take
/// the block's own weight and color.
fn text_runs(
    text: &str,
    highlights: &[(Range<usize>, HighlightStyle)],
    code: &[Range<usize>],
    weight: FontWeight,
    color: Hsla,
) -> Vec<TextRun> {
    let base = Font {
        weight,
        ..font(UI_FONT)
    };
    let plain = |len: usize| TextRun {
        len,
        font: base.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let mut runs = Vec::new();
    let mut at = 0;
    for (range, style) in highlights {
        if range.is_empty() || range.start < at || range.end > text.len() {
            continue;
        }
        if range.start > at {
            runs.push(plain(range.start - at));
        }
        let mut font = if code.contains(range) {
            Font {
                family: MONO_FONT.into(),
                ..base.clone()
            }
        } else {
            base.clone()
        };
        if let Some(weight) = style.font_weight {
            font.weight = weight;
        }
        if let Some(style) = style.font_style {
            font.style = style;
        }
        runs.push(TextRun {
            len: range.len(),
            font,
            color: style.color.unwrap_or(color),
            background_color: style.background_color,
            underline: style.underline,
            strikethrough: style.strikethrough,
        });
        at = range.end;
    }
    if at < text.len() {
        runs.push(plain(text.len() - at));
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_items_carry_their_marker_and_inline_code_its_span() {
        let parsed = blocks("- uno\n\n- **dos** `code`\n");
        assert_eq!(parsed[0].text, "uno");
        assert_eq!(parsed[0].marker, Some(Marker::Bullet));
        assert_eq!(parsed[1].text, "dos code");
        assert_eq!(parsed[1].code_spans, vec![4..8]);
        assert!(
            parsed[1]
                .runs
                .iter()
                .any(|(_, style)| style.background_color.is_some())
        );
    }

    #[test]
    fn task_items_become_checkboxes_without_a_bullet() {
        let parsed = blocks("- [ ] uno\n- [x] dos\n");
        assert_eq!(parsed[0].text, "uno");
        assert_eq!(parsed[0].marker, Some(Marker::Task(false)));
        assert_eq!(parsed[1].text, "dos");
        assert_eq!(parsed[1].marker, Some(Marker::Task(true)));
    }

    #[test]
    fn ordered_and_nested_items_keep_their_numbers_and_depth() {
        let parsed = blocks("3. one\n4. two\n   - nested\n");
        assert_eq!(parsed[0].marker, Some(Marker::Number(3)));
        assert_eq!(parsed[1].marker, Some(Marker::Number(4)));
        assert_eq!((parsed[1].depth, parsed[2].depth), (1, 2));
        assert_eq!(parsed[2].marker, Some(Marker::Bullet));
    }

    #[test]
    fn an_items_later_paragraphs_stay_under_it_and_the_list_ends_cleanly() {
        let parsed = blocks("- first\n\n  second\n\n-\n\nafter\n");
        let shape: Vec<_> = parsed
            .iter()
            .map(|block| (block.text.as_str(), block.depth, block.marker))
            .collect();
        assert_eq!(
            shape,
            [
                ("first", 1, Some(Marker::Bullet)),
                ("second", 1, None),
                ("after", 0, None),
            ]
        );
    }

    #[test]
    fn inline_code_runs_switch_to_the_monospaced_font_and_cover_the_text() {
        let parsed = blocks("use `cargo test` now");
        let block = &parsed[0];
        let runs = text_runs(
            &block.text,
            &block.runs,
            &block.code_spans,
            FontWeight::NORMAL,
            colors().foreground.into(),
        );
        assert_eq!(
            runs.iter().map(|run| run.len).sum::<usize>(),
            block.text.len()
        );
        let families: Vec<_> = runs.iter().map(|run| run.font.family.as_ref()).collect();
        assert_eq!(families, [UI_FONT, MONO_FONT, UI_FONT]);
    }

    #[test]
    fn fenced_code_is_highlighted_in_its_language_without_a_trailing_line() {
        let parsed = blocks("```rust\nlet total = 1;\n```\n");
        assert_eq!(parsed.len(), 1);
        assert!(parsed[0].code);
        assert_eq!(parsed[0].text, "let total = 1;");
        let keyword = crate::ui::syntax::SyntaxKind::Keyword.highlight_style();
        assert!(parsed[0].runs.contains(&(0..3, keyword)));
    }

    #[test]
    fn nested_block_quotes_keep_the_outer_quote_after_the_inner_quote_ends() {
        let parsed = blocks("> outer\n>\n>> inner\n>\n> outer again\n\nplain\n");
        assert_eq!(parsed.len(), 4);
        assert!(parsed[..3].iter().all(|block| block.quote));
        assert!(!parsed[3].quote);
    }

    #[test]
    fn quoted_headings_code_and_lists_keep_the_quote_style() {
        let parsed = blocks("> # Heading\n>\n> ```\n> code\n> ```\n>\n> - item\n");
        assert_eq!(parsed.len(), 3);
        assert!(parsed.iter().all(|block| block.quote));
        assert_eq!(parsed[0].heading, 1);
        assert!(parsed[1].code);
        assert_eq!(parsed[2].text, "item");
        assert_eq!(parsed[2].marker, Some(Marker::Bullet));
    }

    #[test]
    fn markdown_retains_headings_code_and_safe_links_without_executing_html() {
        let parsed = blocks(concat!(
            "# Título\n\nTexto **fuerte** [web](https://example.com).\n\n",
            "```rs\nlet x = 1;\n```\n<script>bad()</script>",
        ));
        assert_eq!(parsed[0].heading, 1);
        assert!(
            parsed
                .iter()
                .any(|block| block.code && block.text.contains("let x"))
        );
        assert!(parsed.iter().any(|block| block.links.len() == 1));
        assert!(!safe_link("javascript:alert(1)"));
        assert!(!safe_link("file:///etc/passwd"));
    }
}
