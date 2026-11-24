// SPDX-License-Identifier: MPL-2.0

//! The conversation's layout: its messages as rows of styled text at a
//! width, with what a click on each does and where pictures go.
//! [`Layout::of`] is all of it; drawing only paints what it returns, and
//! what can be expanded is asked of it ([`can_expand`]), not remembered
//! from the last frame.

use std::collections::{HashMap, HashSet};

use chrono::{Local, TimeZone};

use super::*;
use crate::{app::Pictures, model::Reaction};

/// Messages from the same sender within this window share one header.
const GROUP_WINDOW_MS: u64 = 5 * 60 * 1000;

/// One rendered line of the conversation, plus what clicking it does.
#[derive(Default)]
pub(crate) struct Row {
    pub line: Line<'static>,
    /// Event id of the message this line belongs to, when it can be selected.
    pub item: Option<String>,
    /// Action for a click anywhere on the line (wins over selecting).
    pub action: Option<HitAction>,
    /// Smaller targets inside the line: (first cell, width, action).
    pub spots: Vec<(usize, usize, HitAction)>,
    /// A row kept free for a Sixel picture: (event id, this row, rows and
    /// columns of the picture).
    pub picture: Option<(String, usize, usize, usize)>,
}

impl Row {
    fn plain(line: Line<'static>) -> Self {
        Self {
            line,
            ..Self::default()
        }
    }

    fn of(item: &Item, line: Line<'static>) -> Self {
        let selectable = !item.event_id.is_empty();
        Self {
            line,
            item: selectable.then(|| item.event_id.clone()),
            ..Self::default()
        }
    }
}

fn clock(ts_ms: u64) -> String {
    Local
        .timestamp_millis_opt(ts_ms as i64)
        .single()
        .map(|t| t.format("%H:%M").to_string())
        .unwrap_or_default()
}

fn long_date(ts_ms: u64) -> String {
    Local
        .timestamp_millis_opt(ts_ms as i64)
        .single()
        .map(|t| t.format("%A, %-d %B %Y").to_string())
        .unwrap_or_default()
}

/// Cut `text` into pieces of at most `width` cells, without regard for
/// words (for code, where spacing matters).
fn hard_wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(4);
    let mut out = vec![String::new()];
    let mut used = 0;
    for c in text.chars() {
        let w = crate::inline::cells(c);
        if used + w > width {
            out.push(String::new());
            used = 0;
        }
        out.last_mut().expect("never empty").push(c);
        used += w;
    }
    out
}

/// Word-wrap `text` to `width` cells, keeping its own line breaks.
pub(super) fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(4);
    let options = textwrap::Options::new(width).break_words(true);
    let mut out = Vec::new();
    for paragraph in text.split('\n') {
        if paragraph.trim().is_empty() {
            out.push(String::new());
        } else {
            out.extend(
                textwrap::wrap(paragraph, &options)
                    .into_iter()
                    .map(|line| line.into_owned()),
            );
        }
    }
    out
}

/// What the layout of a conversation depends on, besides its messages.
pub(crate) struct Options<'a> {
    pub pictures: &'a Pictures,
    pub show_room_events: bool,
    pub show_receipts: bool,
    pub link_previews: &'a HashMap<String, (String, String)>,
    /// Messages of more lines than this are cut short; 0: none is.
    pub long_limit: usize,
    /// The messages shown in full.
    pub expanded: &'a HashSet<String>,
    pub selected: Option<&'a str>,
    pub width: usize,
    pub theme: &'a Theme,
}

impl<'a> Options<'a> {
    /// As `app` shows its conversation, `width` cells wide.
    pub(crate) fn of(app: &'a App, width: usize) -> Self {
        Self {
            pictures: &app.pictures,
            show_room_events: app.state.prefs.room_events,
            show_receipts: app.state.prefs.read_receipts,
            link_previews: &app.state.link_previews,
            long_limit: app.state.prefs.long_messages,
            expanded: &app.expanded,
            selected: app.selected.as_deref(),
            width,
            theme: &app.theme,
        }
    }
}

/// A conversation laid out: its rows from the oldest message down, and
/// what of it does not show in full.
pub(crate) struct Layout {
    pub rows: Vec<Row>,
    /// The messages with more to show than fits at this width (a long
    /// text, names cut short): they can be expanded and folded again.
    pub collapsible: HashSet<String>,
}

impl Layout {
    pub(crate) fn of(items: &[Item], options: &Options) -> Self {
        let mut collapsible = HashSet::new();
        let rows = rows(items, options, &mut collapsible);
        Self { rows, collapsible }
    }

    /// The first and last row of a message.
    pub(crate) fn rows_of(&self, event_id: &str) -> Option<(usize, usize)> {
        let its = |row: &Row| row.item.as_deref() == Some(event_id);
        Some((
            self.rows.iter().position(its)?,
            self.rows.iter().rposition(its)?,
        ))
    }
}

/// Whether `item` has more to show than `app`'s conversation shows of it
/// at its width: it can be expanded (and, once it is, folded again).
pub(crate) fn can_expand(app: &App, item: &Item) -> bool {
    // The panel's border on both sides; before the first frame, a guess.
    let width = match app.timeline_area.width {
        0 => 80,
        width => width.saturating_sub(2) as usize,
    };
    !Layout::of(std::slice::from_ref(item), &Options::of(app, width))
        .collapsible
        .is_empty()
}

fn rows(items: &[Item], options: &Options, collapsible: &mut HashSet<String>) -> Vec<Row> {
    let Options {
        pictures,
        show_room_events,
        show_receipts,
        link_previews,
        long_limit,
        expanded,
        selected,
        width,
        theme,
    } = *options;
    let dim = Style::new().fg(theme.dim);
    let mut lines: Vec<Row> = Vec::new();
    // (sender, timestamp) of the previous chat message, for grouping.
    let mut previous: Option<(&str, u64)> = None;

    for item in items {
        match &item.kind {
            ItemKind::DateDivider => {
                previous = None;
                if !lines.is_empty() {
                    lines.push(Row::plain(Line::default()));
                }
                let label = format!("── {} ──", long_date(item.ts_ms));
                lines.push(Row::plain(
                    Line::from(Span::styled(label, dim)).alignment(Alignment::Center),
                ));
                continue;
            }
            ItemKind::ReadMarker => {
                previous = None;
                lines.push(Row::plain(
                    Line::from(Span::styled("── new ──", Style::new().fg(theme.unread)))
                        .alignment(Alignment::Center),
                ));
                continue;
            }
            ItemKind::State if !show_room_events => continue,
            ItemKind::State => {
                previous = None;
                for text in wrap(&item.body, width.saturating_sub(4)) {
                    lines.push(Row::plain(Line::from(Span::styled(
                        format!("  · {text}"),
                        dim.add_modifier(Modifier::ITALIC),
                    ))));
                }
                continue;
            }
            _ => {}
        }

        let grouped = previous.is_some_and(|(sender, ts)| {
            sender == item.sender_id && item.ts_ms.saturating_sub(ts) < GROUP_WINDOW_MS
        });
        previous = Some((&item.sender_id, item.ts_ms));

        if !grouped {
            if !lines.is_empty() {
                lines.push(Row::plain(Line::default()));
            }
            lines.push(Row::plain(Line::from(vec![
                Span::styled(
                    item.sender_name.clone(),
                    Style::new()
                        .fg(theme.sender_color(&item.sender_id))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("  {}", clock(item.ts_ms)), dim),
            ])));
        }

        if let Some((name, snippet)) = &item.reply {
            // A quote without text (a picture, or IServ's "null"): the name.
            let quote = if snippet.is_empty() {
                format!("↩ {name}")
            } else {
                format!("{name}: {snippet}")
            };
            let quote = fit(&quote, width.saturating_sub(4));
            lines.push(Row::of(
                item,
                Line::from(vec![
                    Span::styled("  ┃ ", Style::new().fg(theme.border)),
                    Span::styled(quote, dim),
                ]),
            ));
        }

        let body_width = width.saturating_sub(2);
        match &item.kind {
            ItemKind::Poll(poll) => push_poll(&mut lines, item, poll, width, theme),
            ItemKind::Redacted => lines.push(Row::plain(Line::from(Span::styled(
                "  message deleted",
                dim.add_modifier(Modifier::ITALIC),
            )))),
            ItemKind::Undecryptable => lines.push(Row::of(
                item,
                Line::from(Span::styled(
                    "  🔒 cannot decrypt this message yet (/recover <key> restores old messages)",
                    dim,
                )),
            )),
            kind => {
                let (text, mut style) = match kind {
                    ItemKind::Emote => (
                        format!("* {} {}", item.sender_name, item.body),
                        Style::new().fg(theme.fg).add_modifier(Modifier::ITALIC),
                    ),
                    ItemKind::Notice => (item.body.clone(), dim),
                    ItemKind::Media => (item.body.clone(), Style::new().fg(theme.accent)),
                    _ => (item.body.clone(), Style::new().fg(theme.fg)),
                };
                if item.pending {
                    style = dim;
                }
                let segments = if matches!(kind, ItemKind::Text | ItemKind::Notice) {
                    codeblock::segments(&text)
                } else {
                    vec![Segment::Text(text)]
                };
                let mut body: Vec<Row> = Vec::new();
                let mut code_index = 0;
                for segment in segments {
                    match segment {
                        Segment::Text(text) => {
                            let (parsed, links) = inline::parse_links(&text);
                            // Which link the text is in, also across a
                            // line break.
                            let mut next_link = 0usize;
                            let mut in_link: Option<usize> = None;
                            for styled in inline::wrap(&parsed, body_width) {
                                let mut link_spots: Vec<(usize, usize, usize)> = Vec::new();
                                let mut column = 2usize;
                                for (c, flags) in &styled {
                                    let cells = crate::inline::cells(*c);
                                    if flags & inline::LINK == 0 {
                                        in_link = None;
                                    } else {
                                        let link = *in_link.get_or_insert_with(|| {
                                            next_link += 1;
                                            next_link - 1
                                        });
                                        match link_spots.last_mut() {
                                            Some((last, start, width))
                                                if *last == link && *start + *width == column =>
                                            {
                                                *width += cells;
                                            }
                                            _ => link_spots.push((link, column, cells)),
                                        }
                                    }
                                    column += cells;
                                }
                                let mut spans = vec![Span::raw("  ")];
                                let mut run = String::new();
                                let mut run_flags = 0u8;
                                let flush = |run: &mut String, flags: u8, spans: &mut Vec<Span>| {
                                    if run.is_empty() {
                                        return;
                                    }
                                    let mut look = style;
                                    if flags & inline::CODE != 0 {
                                        look = look.fg(theme.success);
                                    }
                                    if flags & inline::BOLD != 0 {
                                        look = look.add_modifier(Modifier::BOLD);
                                    }
                                    if flags & inline::ITALIC != 0 {
                                        look = look.add_modifier(Modifier::ITALIC);
                                    }
                                    if flags & inline::STRIKE != 0 {
                                        look = look.add_modifier(Modifier::CROSSED_OUT);
                                    }
                                    if flags & inline::HEADING != 0 {
                                        look = look.fg(theme.accent).add_modifier(Modifier::BOLD);
                                    }
                                    if flags & inline::QUOTE != 0 {
                                        look = look.fg(theme.dim);
                                    }
                                    if flags & inline::LINK != 0 {
                                        look = look
                                            .fg(theme.accent)
                                            .add_modifier(Modifier::UNDERLINED);
                                    }
                                    spans.push(Span::styled(std::mem::take(run), look));
                                };
                                for (c, flags) in styled {
                                    if flags != run_flags {
                                        flush(&mut run, run_flags, &mut spans);
                                        run_flags = flags;
                                    }
                                    run.push(c);
                                }
                                flush(&mut run, run_flags, &mut spans);
                                let mut entry = Row::of(item, Line::from(spans));
                                for (link, start, width) in link_spots {
                                    if let Some(address) = links.get(link) {
                                        entry.spots.push((
                                            start,
                                            width,
                                            HitAction::OpenLink(address.clone()),
                                        ));
                                    }
                                }
                                body.push(entry);
                            }
                        }
                        // A code block: framed, kept as typed, with its own
                        // copy button.
                        Segment::Code { language, content } => {
                            let label = if language.is_empty() {
                                "code".to_owned()
                            } else {
                                format!("code · {language}")
                            };
                            let head = format!("  ┌ {label}  ");
                            let button = "[copy]";
                            let mut entry = Row::of(
                                item,
                                Line::from(vec![
                                    Span::styled(head.clone(), dim),
                                    Span::styled(button, Style::new().fg(theme.accent)),
                                ]),
                            );
                            if !item.event_id.is_empty() {
                                entry.spots.push((
                                    head.width(),
                                    button.width(),
                                    HitAction::CopyCode {
                                        event_id: item.event_id.clone(),
                                        index: code_index,
                                    },
                                ));
                            }
                            body.push(entry);
                            let code_style = Style::new().fg(theme.success);
                            for line in content.split('\n') {
                                let line = line.replace('\t', "    ");
                                for piece in hard_wrap(&line, body_width.saturating_sub(2)) {
                                    body.push(Row::of(
                                        item,
                                        Line::from(vec![
                                            Span::styled("  │ ", dim),
                                            Span::styled(piece, code_style),
                                        ]),
                                    ));
                                }
                            }
                            body.push(Row::of(item, Line::from(Span::styled("  └", dim))));
                            code_index += 1;
                        }
                    }
                }

                // A wall of text is cut short until asked for.
                let total = body.len();
                if long_limit > 0 && total > long_limit + 2 && !item.event_id.is_empty() {
                    collapsible.insert(item.event_id.clone());
                    let open = expanded.contains(&item.event_id);
                    let label = if open {
                        "  ▴ show less".to_owned()
                    } else {
                        body.truncate(long_limit);
                        format!("  ▾ show {} more lines", total - long_limit)
                    };
                    let mut toggle = Row::of(
                        item,
                        Line::from(Span::styled(label, Style::new().fg(theme.accent))),
                    );
                    toggle.action = Some(HitAction::ToggleExpand(item.event_id.clone()));
                    body.push(toggle);
                }

                if let Some(last) = body.last_mut() {
                    if item.edited {
                        last.line.spans.push(Span::styled(" (edited)", dim));
                    }
                    if item.failed {
                        last.line
                            .spans
                            .push(Span::styled("  ✗ not sent", Style::new().fg(theme.error)));
                    } else if item.pending {
                        last.line.spans.push(Span::styled("  ⋯", dim));
                    }
                }
                lines.extend(body);
            }
        }

        if item.image {
            match pictures.drawn(&item.event_id, width.saturating_sub(2)) {
                // A Sixel picture is painted over rows kept free for it,
                // once all of them are in view.
                Some(Drawn::Painted { rows, columns }) => {
                    for row in 0..rows {
                        let mut entry = Row::of(item, Line::default());
                        entry.picture = Some((item.event_id.clone(), row, rows, columns));
                        lines.push(entry);
                    }
                }
                Some(Drawn::Lines(drawn)) => {
                    lines.extend(drawn.into_iter().map(|line| Row::of(item, line)));
                }
                None => {}
            }
        }

        // What is behind the first link, when the homeserver told us.
        if matches!(item.kind, ItemKind::Text | ItemKind::Notice)
            && !link_previews.is_empty()
            && let Some(url) = inline::parse_links(&item.body).1.into_iter().next()
            && let Some((title, description)) = link_previews.get(&url)
            && !title.is_empty()
        {
            let room = width.saturating_sub(5);
            let mut entry = Row::of(
                item,
                Line::from(vec![
                    Span::styled("  ▎ ", Style::new().fg(theme.border)),
                    Span::styled(
                        fit(title, room),
                        Style::new().fg(theme.fg).add_modifier(Modifier::BOLD),
                    ),
                ]),
            );
            entry.action = Some(HitAction::OpenLink(url.clone()));
            lines.push(entry);
            if !description.is_empty() {
                let mut entry = Row::of(
                    item,
                    Line::from(vec![
                        Span::styled("  ▎ ", Style::new().fg(theme.border)),
                        Span::styled(fit(description, room), dim),
                    ]),
                );
                entry.action = Some(HitAction::OpenLink(url));
                lines.push(entry);
            }
        }

        // Names cut short (reactions, "read by") show in full while the
        // message is expanded; one "▴ less" under it folds them again,
        // unless a long text has its own.
        let open = expanded.contains(&item.event_id);
        let text_toggle = collapsible.contains(&item.event_id);
        let mut names_cut = false;

        if !item.reactions.is_empty() {
            let named = open || selected == Some(item.event_id.as_str());
            let long = item.reactions.iter().any(|r| r.count > NAMES_SHOWN);
            if long && !item.event_id.is_empty() {
                collapsible.insert(item.event_id.clone());
                names_cut = true;
            }
            if open && long {
                lines.extend(reaction_lists(item, width, theme));
            } else {
                lines.extend(reaction_row(item, named, width, theme));
            }
        }

        // A thread started here: how much is in it; a click opens it.
        if let Some((replies, latest)) = &item.thread
            && !item.event_id.is_empty()
        {
            let count = match replies {
                1 => "1 reply".to_owned(),
                count => format!("{count} replies"),
            };
            let head = format!("  ↳ {count}");
            let newest = match latest {
                Some((name, text)) if !text.is_empty() => format!(" · {name}: {text}"),
                Some((name, _)) => format!(" · {name}"),
                None => String::new(),
            };
            let newest = fit(&newest, width.saturating_sub(head.width() + 1));
            let mut entry = Row::of(
                item,
                Line::from(vec![
                    Span::styled(
                        head,
                        Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(newest, dim),
                ]),
            );
            entry.action = Some(HitAction::OpenThread(item.event_id.clone()));
            lines.push(entry);
        }

        if item.pinned {
            lines.push(Row::of(
                item,
                Line::from(Span::styled("  ⚑ pinned", Style::new().fg(theme.mention))),
            ));
        }

        if show_receipts && !item.read_by.is_empty() {
            let long = item.read_by.len() > NAMES_SHOWN && !item.event_id.is_empty();
            if long {
                collapsible.insert(item.event_id.clone());
                names_cut = true;
            }
            lines.extend(read_by_lines(item, long && open, width, theme));
        }

        if open && names_cut && !text_toggle {
            lines.push(less_line(item, theme));
        }
    }
    lines
}

/// How many names a reaction or "read by" shows before `+N`; the rest
/// show when the message is expanded (`x`, or a click on `+N`).
const NAMES_SHOWN: usize = 3;

/// Who reacted so: the user ("you") first, then the others in order.
fn reaction_names(reaction: &Reaction) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    if reaction.mine {
        names.push("you".to_owned());
    }
    names.extend(reaction.senders.iter().cloned());
    names
}

fn reaction_style(reaction: &Reaction, theme: &Theme) -> Style {
    if reaction.mine {
        Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.dim)
    }
}

/// The reactions side by side, onto the next line when one does not fit:
/// with counts (`👍 3`), or `named` with the first names (`👍 you, Amy,
/// Ben +4`). A click on a reaction adds the user's or takes it back, one
/// on `+4` shows all names.
fn reaction_row(item: &Item, named: bool, width: usize, theme: &Theme) -> Vec<Row> {
    let mut lines = vec![Row::of(item, Line::from(Span::raw("  ")))];
    let mut column = 2;
    for reaction in &item.reactions {
        let style = reaction_style(reaction, theme);
        let mut parts: Vec<(String, Style, Option<HitAction>)> = Vec::new();
        let react = (!item.event_id.is_empty()).then(|| HitAction::React {
            event_id: item.event_id.clone(),
            key: reaction.key.clone(),
        });
        let names = reaction_names(reaction);
        if named && !names.is_empty() {
            let shown = &names[..names.len().min(NAMES_SHOWN)];
            parts.push((
                format!("{} {}", reaction.key, shown.join(", ")),
                style,
                react,
            ));
            let more = reaction.count.saturating_sub(shown.len());
            if more > 0 {
                parts.push((
                    format!(" +{more}"),
                    Style::new().fg(theme.accent),
                    (!item.event_id.is_empty())
                        .then(|| HitAction::ToggleExpand(item.event_id.clone())),
                ));
            }
        } else {
            parts.push((format!("{} {}", reaction.key, reaction.count), style, react));
        }
        let chip_width: usize = parts.iter().map(|(text, ..)| text.width()).sum();
        if column > 2 && column + chip_width > width {
            lines.push(Row::of(item, Line::from(Span::raw("  "))));
            column = 2;
        }
        let entry = lines.last_mut().expect("a line");
        for (text, style, action) in parts {
            let cells = text.width();
            if let Some(action) = action {
                entry.spots.push((column, cells, action));
            }
            entry.line.spans.push(Span::styled(text, style));
            column += cells;
        }
        entry.line.spans.push(Span::raw("  "));
        column += 2;
    }
    lines
}

/// Every reaction on its own lines, with all names.
fn reaction_lists(item: &Item, width: usize, theme: &Theme) -> Vec<Row> {
    let mut lines = Vec::new();
    for reaction in &item.reactions {
        let style = reaction_style(reaction, theme);
        let head = format!("  {} ", reaction.key);
        let indent = head.width();
        let names = reaction_names(reaction).join(", ");
        for (index, piece) in wrap(&names, width.saturating_sub(indent))
            .into_iter()
            .enumerate()
        {
            let lead = if index == 0 {
                head.clone()
            } else {
                " ".repeat(indent)
            };
            let mut entry = Row::of(
                item,
                Line::from(vec![Span::styled(lead, style), Span::styled(piece, style)]),
            );
            if index == 0 && !item.event_id.is_empty() {
                entry.spots.push((
                    2,
                    reaction.key.width(),
                    HitAction::React {
                        event_id: item.event_id.clone(),
                        key: reaction.key.clone(),
                    },
                ));
            }
            lines.push(entry);
        }
    }
    lines
}

/// `▴ less`: a click folds the message again.
fn less_line(item: &Item, theme: &Theme) -> Row {
    let mut entry = Row::of(
        item,
        Line::from(Span::styled("  ▴ less", Style::new().fg(theme.accent))),
    );
    entry.action = Some(HitAction::ToggleExpand(item.event_id.clone()));
    entry
}

/// `✓ read by Amy, Ben, Cleo +2` on the right, `open` with all names
/// (over several lines when needed).
fn read_by_lines(item: &Item, open: bool, width: usize, theme: &Theme) -> Vec<Row> {
    let dim = Style::new().fg(theme.dim);
    let room = width.saturating_sub(2);
    let right = |text: String, more: Option<String>| {
        let more_width = more.as_ref().map_or(0, |more| more.width());
        let text = fit(&text, room.saturating_sub(more_width));
        let gap = width.saturating_sub(text.width() + more_width);
        let mut entry = Row::of(
            item,
            Line::from(vec![
                Span::raw(" ".repeat(gap)),
                Span::styled(text.clone(), dim),
            ]),
        );
        if let Some(more) = more {
            if !item.event_id.is_empty() {
                entry.spots.push((
                    gap + text.width(),
                    more.width(),
                    HitAction::ToggleExpand(item.event_id.clone()),
                ));
            }
            entry
                .line
                .spans
                .push(Span::styled(more, Style::new().fg(theme.accent)));
        }
        entry
    };
    let names = &item.read_by;
    if open {
        return wrap(&format!("✓ read by {}", names.join(", ")), room)
            .into_iter()
            .map(|piece| right(piece, None))
            .collect();
    }
    if names.len() > NAMES_SHOWN && !item.event_id.is_empty() {
        let more = format!(" +{}", names.len() - NAMES_SHOWN);
        vec![right(
            format!("✓ read by {}", names[..NAMES_SHOWN].join(", ")),
            Some(more),
        )]
    } else {
        vec![right(format!("✓ read by {}", names.join(", ")), None)]
    }
}

fn push_poll(lines: &mut Vec<Row>, item: &Item, poll: &Poll, width: usize, theme: &Theme) {
    let dim = Style::new().fg(theme.dim);
    let can_vote = !poll.is_ended && !item.event_id.is_empty();

    for text in wrap(&poll.question, width.saturating_sub(4)) {
        lines.push(Row::of(
            item,
            Line::from(vec![
                Span::styled("  ▍ ", Style::new().fg(theme.accent)),
                Span::styled(text, Style::new().fg(theme.fg).add_modifier(Modifier::BOLD)),
            ]),
        ));
    }

    let bar_width = (width / 3).clamp(8, 24);
    for answer in &poll.answers {
        let action = can_vote.then(|| HitAction::Vote {
            poll_event_id: item.event_id.clone(),
            answer_id: answer.id.clone(),
        });
        let votes = answer.voters.len() as u64;
        let fraction = if poll.results_visible && poll.voter_count > 0 {
            votes as f64 / poll.voter_count as f64
        } else {
            0.0
        };
        let highlight = if answer.is_winner {
            theme.success
        } else {
            theme.accent
        };

        // Once a poll has ended there is nothing left to choose, so only
        // our own answers keep a marker.
        let marker = match (answer.self_voted, poll.max_selections > 1) {
            (true, false) => "●",
            (true, true) => "■",
            (false, _) if poll.is_ended => " ",
            (false, false) => "○",
            (false, true) => "□",
        };
        let marker_style = if answer.self_voted {
            Style::new().fg(highlight).add_modifier(Modifier::BOLD)
        } else {
            dim
        };
        let count = if poll.results_visible {
            format!("{votes} · {}%", (fraction * 100.0).round() as u64)
        } else {
            String::new()
        };
        let star = if answer.is_winner { "★ " } else { "" };
        let text_width = width.saturating_sub(6 + star.width() + count.width() + 2);
        let wrapped = wrap(&answer.text, text_width);
        for (index, text) in wrapped.iter().enumerate() {
            let mut text_style = Style::new().fg(theme.fg);
            if answer.is_winner {
                text_style = text_style.fg(theme.success).add_modifier(Modifier::BOLD);
            }
            let mut spans = if index == 0 {
                vec![
                    Span::raw("    "),
                    Span::styled(format!("{marker} "), marker_style),
                    Span::styled(star, Style::new().fg(theme.success)),
                    Span::styled(text.clone(), text_style),
                ]
            } else {
                vec![Span::raw("      "), Span::styled(text.clone(), text_style)]
            };
            if index == 0 && !count.is_empty() {
                let used = 6 + star.width() + text.width();
                let gap = width.saturating_sub(used + count.width() + 1).max(1);
                spans.push(Span::raw(" ".repeat(gap)));
                spans.push(Span::styled(count.clone(), dim));
            }
            let mut entry = Row::of(item, Line::from(spans));
            entry.action = action.clone();
            lines.push(entry);
        }

        // Share bar plus who voted for this answer.
        if poll.results_visible || !answer.voters.is_empty() {
            let filled = ((fraction * bar_width as f64).round() as usize).min(bar_width);
            let names_width = width.saturating_sub(6 + bar_width + 2);
            let names = wrap(&answer.voters.join(", "), names_width);
            for (index, names_line) in names.iter().enumerate() {
                let mut spans = vec![Span::raw("      ")];
                if index == 0 {
                    spans.push(Span::styled("█".repeat(filled), Style::new().fg(highlight)));
                    spans.push(Span::styled(
                        "░".repeat(bar_width - filled),
                        Style::new().fg(theme.border),
                    ));
                } else {
                    spans.push(Span::raw(" ".repeat(bar_width)));
                }
                spans.push(Span::styled(format!("  {names_line}"), dim));
                let mut entry = Row::of(item, Line::from(spans));
                entry.action = action.clone();
                lines.push(entry);
            }
        }
    }

    let status = if poll.is_ended {
        format!("Final results · Votes: {}", poll.voter_count)
    } else if item.event_id.is_empty() {
        "Sending poll…".to_owned()
    } else if !poll.results_visible {
        format!(
            "Votes: {} · Results are shown when the poll ends",
            poll.voter_count
        )
    } else if poll.max_selections > 1 {
        format!(
            "Votes: {} · Choose up to {} · click an answer to vote",
            poll.voter_count, poll.max_selections
        )
    } else {
        format!("Votes: {} · click an answer to vote", poll.voter_count)
    };
    let status = format!("    {status}");
    let mut entry = Row::of(item, Line::default());
    let mut spans = vec![Span::styled(status.clone(), dim)];
    // Only the author can close a poll.
    if item.is_own && can_vote {
        let button = "[ End poll ]";
        entry.spots.push((
            status.width() + 2,
            button.width(),
            HitAction::AskEndPoll(item.event_id.clone()),
        ));
        spans.push(Span::raw("  "));
        spans.push(Span::styled(button, Style::new().fg(theme.accent)));
    }
    entry.line = Line::from(spans);
    lines.push(entry);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::PollAnswer;

    /// The layout of `items` for a fresh `App`, as `change` leaves it.
    fn laid_out(
        items: &[Item],
        width: usize,
        change: impl FnOnce(&mut App),
    ) -> (Vec<String>, Layout) {
        let mut app = App::new(crate::theme::THEMES[0], "@me:x".into());
        change(&mut app);
        let layout = Layout::of(items, &Options::of(&app, width));
        let text = layout.rows.iter().map(|row| row.line.to_string()).collect();
        (text, layout)
    }

    fn message(event_id: &str, body: &str) -> Item {
        Item {
            key: event_id.into(),
            event_id: event_id.into(),
            sender_id: "@amy:x".into(),
            sender_name: "Amy".into(),
            body: body.into(),
            ..Item::default()
        }
    }

    fn reaction(key: &str, names: &[&str]) -> Reaction {
        Reaction {
            key: key.into(),
            count: names.len(),
            mine: false,
            senders: names.iter().map(|name| (*name).to_owned()).collect(),
        }
    }

    #[test]
    fn messages_wrap_at_the_width_and_know_their_rows() {
        let items = [
            message("$1", "one two three four five six"),
            message("$2", "short"),
        ];
        let (text, layout) = laid_out(&items, 16, |_| {});
        // One header for both (same sender, same minute), the body indented.
        assert!(text[0].starts_with("Amy"), "{text:?}");
        assert_eq!(text[1..], ["  one two three", "  four five six", "  short"]);
        assert_eq!(layout.rows_of("$1"), Some((1, 2)));
        assert_eq!(layout.rows_of("$2"), Some((3, 3)));
        assert_eq!(layout.rows_of("$3"), None);
        assert!(layout.collapsible.is_empty());
        // The header belongs to no message: a click there selects nothing.
        assert_eq!(layout.rows[0].item, None);
    }

    #[test]
    fn a_long_message_is_cut_short_until_it_is_expanded() {
        let body = (1..=8)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let items = [message("$long", &body), message("$short", "hi")];
        let (text, layout) = laid_out(&items, 40, |app| app.state.prefs.long_messages = 3);
        assert_eq!(
            text[1..5],
            ["  line 1", "  line 2", "  line 3", "  ▾ show 5 more lines"]
        );
        assert!(layout.collapsible.contains("$long") && !layout.collapsible.contains("$short"));
        assert!(matches!(
            &layout.rows[4].action,
            Some(HitAction::ToggleExpand(event_id)) if event_id == "$long"
        ));

        let (text, layout) = laid_out(&items, 40, |app| {
            app.state.prefs.long_messages = 3;
            app.expanded.insert("$long".into());
        });
        assert_eq!(text[8..10], ["  line 8", "  ▴ show less"]);
        assert!(layout.collapsible.contains("$long"));

        // Whether a message can be expanded depends on the width it has.
        let wide = message("$w", "one two three four five six seven eight nine ten");
        let narrow = |app: &mut App| {
            app.state.prefs.long_messages = 1;
            app.timeline_area = ratatui::layout::Rect::new(0, 0, 14, 10);
        };
        let mut app = App::new(crate::theme::THEMES[0], "@me:x".into());
        app.state.prefs.long_messages = 1;
        app.timeline_area = ratatui::layout::Rect::new(0, 0, 120, 10);
        assert!(!can_expand(&app, &wide));
        narrow(&mut app);
        assert!(can_expand(&app, &wide));
    }

    #[test]
    fn reactions_that_do_not_fit_go_onto_the_next_line() {
        let mut item = message("$r", "hi");
        item.reactions = vec![
            reaction("👍", &["Amy Winter", "Ben Okafor"]),
            reaction("🎉", &["Cleo"]),
        ];
        let items = [item];
        // Names show for the selected message, counts otherwise.
        let (text, _) = laid_out(&items, 30, |app| app.selected = Some("$r".into()));
        assert_eq!(text[2..], ["  👍 Amy Winter, Ben Okafor  ", "  🎉 Cleo  "]);
        let (text, _) = laid_out(&items, 30, |_| {});
        assert_eq!(text[2..], ["  👍 2  🎉 1  "]);
    }

    #[test]
    fn a_poll_s_answers_are_rows_to_vote_with() {
        let mut item = message("$poll", "");
        item.kind = ItemKind::Poll(Poll {
            question: "Lunch?".into(),
            max_selections: 1,
            answers: ["Pizza", "Soup"]
                .iter()
                .map(|text| PollAnswer {
                    id: text.to_lowercase(),
                    text: (*text).to_owned(),
                    ..PollAnswer::default()
                })
                .collect(),
            ..Poll::default()
        });
        let (text, layout) = laid_out(&[item], 40, |_| {});
        assert!(text.iter().any(|line| line.contains("Lunch?")), "{text:?}");
        let votes: Vec<&str> = layout
            .rows
            .iter()
            .filter_map(|row| match &row.action {
                Some(HitAction::Vote { answer_id, .. }) => Some(answer_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(votes, ["pizza", "soup"]);
    }

    #[test]
    fn room_events_show_only_when_wanted() {
        let mut joined = message("$s", "Amy joined");
        joined.kind = ItemKind::State;
        let items = [joined];
        let (text, _) = laid_out(&items, 40, |app| app.state.prefs.room_events = true);
        assert_eq!(text, ["  · Amy joined"]);
        let (text, _) = laid_out(&items, 40, |app| app.state.prefs.room_events = false);
        assert!(text.is_empty());
    }
}
