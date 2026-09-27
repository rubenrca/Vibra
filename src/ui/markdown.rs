//! CommonMark rendered as native text. Remote HTML is never executed.

use crate::ui::theme::{MONO_FONT, colors, surface_tint};
use gpui::{
    AnyElement, FontStyle, FontWeight, HighlightStyle, InteractiveText, SharedString, StyledText,
    div, prelude::*, px,
};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use std::ops::Range;

#[derive(Default)]
struct Block {
    text: String,
    runs: Vec<(Range<usize>, HighlightStyle)>,
    links: Vec<(Range<usize>, String)>,
    heading: u8,
    code: bool,
    quote: bool,
    rule: bool,
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
    let mut quote = false;
    let flush = |output: &mut Vec<Block>, block: &mut Block| {
        if !block.text.is_empty() || block.rule {
            output.push(std::mem::take(block));
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
            }
            Event::Start(Tag::CodeBlock(_)) => {
                flush(&mut output, &mut block);
                block.code = true;
            }
            Event::Start(Tag::Paragraph) => {
                if !item_prefix {
                    flush(&mut output, &mut block);
                }
                block.quote = quote;
            }
            Event::Start(Tag::BlockQuote(_)) => {
                quote = true;
                block.quote = true;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                flush(&mut output, &mut block);
                quote = false;
            }
            Event::Start(Tag::List(start)) => lists.push(start),
            Event::End(TagEnd::List(_)) => {
                lists.pop();
            }
            Event::Start(Tag::Item) => {
                flush(&mut output, &mut block);
                item_prefix = true;
                block
                    .text
                    .push_str(&"  ".repeat(lists.len().saturating_sub(1)));
                if let Some(Some(number)) = lists.last_mut() {
                    block.text.push_str(&format!("{number}. "));
                    *number += 1;
                } else {
                    block.text.push_str("• ");
                }
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
                let start = block.text.len();
                block.text.push_str(&text);
                let range = start..block.text.len();
                if !range.is_empty() {
                    block.runs.push((
                        range.clone(),
                        HighlightStyle {
                            background_color: Some(
                                surface_tint(colors().elevated, colors().background).into(),
                            ),
                            color: Some(colors().accent.into()),
                            ..Default::default()
                        },
                    ));
                    if let Some(url) = link.as_ref().filter(|url| safe_link(url)) {
                        block.links.push((range, url.clone()));
                    }
                }
            }
            Event::Text(text) => {
                item_prefix = false;
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
            Event::TaskListMarker(checked) => {
                block.text.push_str(if checked { "☑ " } else { "☐ " })
            }
            Event::Rule => {
                flush(&mut output, &mut block);
                block.rule = true;
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
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::CodeBlock
                | TagEnd::Item
                | TagEnd::TableRow,
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
    div()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .text_size(px(13.0))
        .line_height(px(21.0))
        .text_color(colors().foreground)
        .children(blocks(text).into_iter().enumerate().map(|(index, block)| {
            if block.rule {
                return div()
                    .h(px(1.0))
                    .bg(colors().border_subtle)
                    .into_any_element();
            }
            let urls: Vec<_> = block.links.iter().map(|(_, url)| url.clone()).collect();
            let ranges = block.links.into_iter().map(|(range, _)| range).collect();
            let text = InteractiveText::new(
                SharedString::from(format!("{id}-{index}")),
                StyledText::new(block.text).with_highlights(block.runs),
            )
            .on_click(ranges, move |index, _, cx| {
                if let Some(url) = urls.get(index) {
                    cx.open_url(url);
                }
            });
            div()
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
                    row.p_3()
                        .font_family(MONO_FONT)
                        .text_size(px(12.0))
                        .rounded(px(6.0))
                        .bg(surface_tint(colors().elevated, colors().background))
                })
                .when(block.quote, |row| {
                    row.pl_3()
                        .border_l_2()
                        .border_color(colors().border_subtle)
                        .text_color(colors().muted)
                })
                .child(text)
                .into_any_element()
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn markdown_keeps_loose_list_prefixes_and_inline_code() {
        let parsed = blocks("- uno\n\n- **dos** `code`\n");
        assert_eq!(parsed[0].text, "• uno");
        assert_eq!(parsed[1].text, "• dos code");
        assert!(
            parsed[1]
                .runs
                .iter()
                .any(|(_, style)| style.background_color.is_some())
        );
    }
    #[test]
    fn markdown_retains_headings_code_and_safe_links_without_executing_html() {
        let parsed = blocks(
            "# Título\n\nTexto **fuerte** [web](https://example.com).\n\n```rs\nlet x = 1;\n```\n<script>bad()</script>",
        );
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
