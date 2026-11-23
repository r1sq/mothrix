// SPDX-License-Identifier: MPL-2.0

//! Drawing. Every draw also records the clickable regions in [`App::hits`],
//! so mouse handling always matches what is on screen.

use chrono::{Local, TimeZone};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Paragraph},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    app::{
        AccountsDialog, App, Badge, Bar, BarAction, CELL_WIDTH, Compose, EmojiPicker, Focus,
        Forward, GridRow, Hit, HitAction, Overlay, PickFor, RecoveryKey, ResetPrompt, RoomInfo,
        RoomMenu, Search, SectionEdit, SectionManager, SectionPicker, Setting, SettingsDialog,
        SidebarRow,
    },
    codeblock::{self, Segment},
    inline,
    input::Input,
    matrix::SyncStatus,
    model::{Item, ItemKind, Poll, Preview, Reaction, VerifyState},
    theme::Theme,
    verify::VerifyInput,
};

/// Messages from the same sender within this window share one header.
const GROUP_WINDOW_MS: u64 = 5 * 60 * 1000;
const MAX_COMPOSER_ROWS: usize = 5;
const JUMP_LABEL: &str = " ↓ jump to newest ";
const POLL_BUTTON: &str = " + poll ";
const FILE_BUTTON: &str = " + file ";
const EMOJI_BUTTON: &str = " + emoji ";

pub fn draw(frame: &mut Frame, app: &mut App) {
    app.hits.clear();
    app.sixel_placements.clear();
    app.ensure_logo();
    let area = frame.area();
    let [main, status] = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(area);

    let sidebar_width = match area.width {
        w if w >= 90 => 32,
        w if w >= 60 => 24,
        _ => 0,
    };
    let [sidebar, right] =
        Layout::horizontal([Constraint::Length(sidebar_width), Constraint::Min(20)]).areas(main);

    let composer_rows = composer_layout(
        app.composer.text(),
        app.composer.cursor(),
        right.width.saturating_sub(4),
    )
    .0
    .len();
    let composer_height = composer_rows.clamp(1, MAX_COMPOSER_ROWS) as u16 + 2;
    let [timeline, composer] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(composer_height)]).areas(right);

    if sidebar_width > 0 {
        draw_sidebar(frame, app, sidebar);
    } else {
        app.sidebar_area = Rect::default();
    }
    draw_timeline(frame, app, timeline);
    draw_composer(frame, app, composer);
    draw_mentions(frame, app, composer);
    draw_status(frame, app, status);
    draw_overlays(frame, app, area);
}

/// The dialogs and menus over the main screen, the one on top last. Each
/// covers all that is under it: a click outside goes to it (and closes it,
/// where it closes that way), never to what is underneath.
fn draw_overlays(frame: &mut Frame, app: &mut App, area: Rect) {
    let overlays: Vec<Overlay> = app.overlays.iter().cloned().collect();
    for overlay in overlays {
        app.hits.push(Hit {
            rect: area,
            action: if overlay.closes_on_outside_click() {
                HitAction::CloseDialogs
            } else {
                HitAction::Blocked
            },
        });
        match overlay {
            Overlay::Settings(settings) => draw_settings(frame, app, area, &settings),
            Overlay::Accounts(state) => draw_accounts(frame, app, area, state),
            Overlay::RoomInfo(info) => draw_room_info(frame, app, area, info),
            Overlay::Forward(forward) => draw_forward(frame, app, area, forward),
            Overlay::RecoveryKey(RecoveryKey(key)) => draw_recovery(frame, app, area, &key),
            Overlay::ResetPrompt(ResetPrompt(input)) => draw_reset(frame, app, area, &input),
            Overlay::RoomMenu(menu) => draw_room_menu(frame, app, area, menu),
            Overlay::SectionManager(manager) => draw_section_manager(frame, app, area, manager),
            Overlay::SectionPicker(picker) => draw_sections(frame, app, area, picker),
            Overlay::Search(search) => draw_search(frame, app, area, search),
            Overlay::Verify(state) => draw_verify(frame, app, area, state),
            Overlay::EmojiPicker(picker) => draw_emoji_picker(frame, app, area, picker),
            Overlay::Help(help) => draw_help(frame, app, area, help),
        }
    }
}

fn panel<'a>(theme: &Theme, focused: bool) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(if focused { theme.accent } else { theme.border }))
}

/// Cut `text` to at most `width` terminal cells, marking the cut with `…`.
fn fit(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

// ---------------------------------------------------------------------------
// Sidebar
// ---------------------------------------------------------------------------

fn draw_sidebar(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = app.theme;
    let focused = app.focus == Focus::Sidebar;
    let block = panel(&theme, focused).title(Line::from(Span::styled(
        " mothrix ",
        Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
    )));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.sidebar_area = area;
    if inner.width < 6 || inner.height < 2 {
        return;
    }

    // Bottom line: the sort order (click to change) and key hints.
    let footer = Rect {
        y: inner.bottom() - 1,
        height: 1,
        ..inner
    };
    let sort_label = format!(" sort: {} ", app.state.prefs.sort.label());
    // Next to it: only unread chats, or all of them.
    let unread_label = if app.state.unread_only {
        " ● unread "
    } else {
        " ○ unread "
    };
    let unread_style = if app.state.unread_only {
        Style::new().fg(theme.unread).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.dim)
    };
    let gap = (inner.width as usize).saturating_sub(sort_label.width() + unread_label.width());
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(sort_label.clone(), Style::new().fg(theme.accent)),
            Span::raw(" ".repeat(gap)),
            Span::styled(unread_label, unread_style),
        ])),
        footer,
    );
    let unread_width = (unread_label.width() as u16).min(footer.width);
    app.hits.push(Hit {
        rect: Rect {
            x: footer.right() - unread_width,
            width: unread_width,
            ..footer
        },
        action: HitAction::ToggleUnread,
    });
    app.hits.push(Hit {
        rect: Rect {
            width: (sort_label.width() as u16).min(footer.width),
            ..footer
        },
        action: HitAction::NextSort,
    });

    let mut list = Rect {
        height: inner.height - 1,
        ..inner
    };
    // With several accounts: a row of tabs, "all" and one per account.
    if app.state.accounts.len() > 1 && list.height > 2 {
        let tabs = account_tabs(app, list.width as usize);
        let mut spans = Vec::new();
        let mut x = list.x;
        for (label, filter, unread) in tabs {
            let chosen = app.state.account_filter == filter;
            let mut style = Style::new().fg(if chosen { theme.accent } else { theme.dim });
            if chosen {
                style = style.add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
            } else if unread {
                style = style.fg(theme.unread).add_modifier(Modifier::BOLD);
            }
            let width = label.width() as u16 + 2;
            app.hits.push(Hit {
                rect: Rect {
                    x,
                    y: list.y,
                    width: width.min(list.right().saturating_sub(x)),
                    height: 1,
                },
                action: HitAction::AccountTab(filter),
            });
            x += width;
            spans.push(Span::raw(" "));
            spans.push(Span::styled(label, style));
            spans.push(Span::raw(" "));
        }
        frame.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect { height: 1, ..list },
        );
        list = Rect {
            y: list.y + 1,
            height: list.height - 1,
            ..list
        };
    }
    if app.filtering || !app.filter.is_empty() {
        let (query, column) = app.filter.view((list.width as usize).saturating_sub(3));
        let line = Line::from(vec![
            Span::styled(" / ", Style::new().fg(theme.accent)),
            Span::styled(query, Style::new().fg(theme.fg)),
        ]);
        frame.render_widget(Paragraph::new(line), Rect { height: 1, ..list });
        if app.filtering && focused {
            frame.set_cursor_position((list.x + 3 + column, list.y));
        }
        list = Rect {
            y: list.y + 1,
            height: list.height.saturating_sub(1),
            ..list
        };
    }
    if list.height == 0 {
        return;
    }

    let lines = app.sidebar_lines();
    if lines.is_empty() {
        let text = if !app.filter.is_empty() {
            "No match"
        } else if app.state.unread_only {
            "Nothing unread"
        } else if app.account().sync == SyncStatus::Connecting {
            "Loading rooms…"
        } else {
            "No rooms yet"
        };
        frame.render_widget(
            Paragraph::new(Span::styled(format!(" {text}"), Style::new().fg(theme.dim))),
            list,
        );
        return;
    }

    let height = list.height as usize;
    app.follow_sidebar_cursor(&lines, height);
    let width = list.width as usize;
    let bold = Modifier::BOLD;
    let badge_style = |badge: Badge| {
        Style::new()
            .fg(if badge.is_mention() {
                theme.mention
            } else {
                theme.unread
            })
            .add_modifier(bold)
    };
    for (offset, line) in lines
        .into_iter()
        .skip(app.sidebar_scroll)
        .take(height)
        .enumerate()
    {
        let rect = Rect {
            y: list.y + offset as u16,
            height: 1,
            ..list
        };
        let indent = if line.indent { "  " } else { "" };
        let arrow = if line.folded { "▸" } else { "▾" };
        let badge = line.badge.text();
        let drawn = match &line.row {
            SidebarRow::Header { title: _ } => Line::from(Span::styled(
                format!(" {indent}{arrow} {}", line.label),
                Style::new().fg(theme.dim).add_modifier(bold),
            )),
            SidebarRow::Folded { rooms, .. } => {
                let head = format!(" {indent}{arrow} {} ", line.label);
                let count = rooms.to_string();
                let gap = width.saturating_sub(head.width() + count.width() + badge.width());
                Line::from(vec![
                    Span::styled(head, Style::new().fg(theme.dim).add_modifier(bold)),
                    Span::styled(count, Style::new().fg(theme.dim)),
                    Span::raw(" ".repeat(gap)),
                    Span::styled(badge, badge_style(line.badge)),
                ])
            }
            SidebarRow::Account { index: account } => {
                let state = match &app.state.accounts[*account].sync {
                    SyncStatus::Online => Span::raw(""),
                    SyncStatus::Connecting => Span::styled("○ ", Style::new().fg(theme.dim)),
                    SyncStatus::Offline => Span::styled("● ", Style::new().fg(theme.unread)),
                    SyncStatus::Error(_) => Span::styled("✗ ", Style::new().fg(theme.error)),
                };
                let room_for_name =
                    width.saturating_sub(3 + state.content.width() + badge.width() + 2);
                let head = format!("{arrow} {} ", fit(&line.label, room_for_name));
                let rule =
                    width.saturating_sub(1 + head.width() + state.content.width() + badge.width());
                Line::from(vec![
                    Span::raw(" "),
                    Span::styled(head, Style::new().fg(theme.accent).add_modifier(bold)),
                    state,
                    Span::styled("─".repeat(rule), Style::new().fg(theme.border)),
                    Span::styled(badge, badge_style(line.badge)),
                ])
            }
            SidebarRow::Room { index } => {
                let room = &app.state.rooms[*index];
                let is_open = app.state.open_room.as_deref() == Some(room.id.as_str());
                let is_cursor = focused && app.cursor_room.as_deref() == Some(room.id.as_str());
                let marker = if is_open {
                    "▌"
                } else if is_cursor {
                    "›"
                } else {
                    " "
                };
                let icon = if room.is_dm { "@ " } else { "# " };
                let name_width =
                    width.saturating_sub(1 + indent.len() + icon.width() + badge.width());
                let name = fit(&line.label, name_width);
                let padding = " ".repeat(name_width.saturating_sub(name.width()));

                let muted = room.notify == crate::model::Notify::Mute;
                let mut base = Style::new().fg(if muted { theme.dim } else { theme.fg });
                if is_open {
                    base = base.bg(theme.selection_bg).fg(theme.selection_fg);
                }
                let mut name_style = base;
                if !line.badge.is_empty() {
                    name_style = name_style.add_modifier(bold);
                }
                if is_cursor {
                    name_style = name_style.add_modifier(Modifier::UNDERLINED);
                }
                Line::from(vec![
                    Span::styled(marker, base.fg(theme.accent)),
                    Span::styled(indent, base),
                    Span::styled(icon, base.fg(theme.dim)),
                    Span::styled(name, name_style),
                    Span::styled(padding, base),
                    Span::styled(badge, base.patch(badge_style(line.badge))),
                ])
            }
        };
        frame.render_widget(Paragraph::new(drawn), rect);
        app.hits.push(Hit {
            rect,
            action: line.action,
        });
    }
}

/// The account tabs that fit into `width` cells: (label, what it shows,
/// whether something unread waits there). Names are cut to share the room.
fn account_tabs(app: &App, width: usize) -> Vec<(String, Option<usize>, bool)> {
    let count = app.state.accounts.len();
    // "all" and two cells of padding per tab.
    let for_names = width.saturating_sub(5 + 2 * count);
    let longest = (for_names / count.max(1)).max(1);
    let mut tabs = vec![("all".to_owned(), None, false)];
    for (index, account) in app.state.accounts.iter().enumerate() {
        tabs.push((
            fit(&account.label, longest),
            Some(index),
            !app.account_unread(index).is_empty(),
        ));
    }
    tabs
}

// ---------------------------------------------------------------------------
// Timeline
// ---------------------------------------------------------------------------

/// One rendered line of the conversation, plus what clicking it does.
#[derive(Default)]
struct TimelineLine {
    line: Line<'static>,
    /// Event id of the message this line belongs to, when it can be selected.
    item: Option<String>,
    /// Action for a click anywhere on the line (wins over selecting).
    action: Option<HitAction>,
    /// Smaller targets inside the line: (first cell, width, action).
    spots: Vec<(usize, usize, HitAction)>,
    /// A row kept free for a Sixel picture: (event id, this row, rows and
    /// columns of the picture).
    picture: Option<(String, usize, usize, usize)>,
}

impl TimelineLine {
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
        let w = c.width().unwrap_or(0);
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
fn wrap(text: &str, width: usize) -> Vec<String> {
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

#[allow(clippy::too_many_arguments)]
fn timeline_lines(
    items: &[Item],
    previews: &std::collections::HashMap<String, Preview>,
    terminal_images: &std::collections::HashMap<String, u32>,
    sixel: &std::collections::HashMap<String, String>,
    show_room_events: bool,
    show_receipts: bool,
    link_previews: &std::collections::HashMap<String, (String, String)>,
    long_limit: usize,
    expanded: &std::collections::HashSet<String>,
    collapsible: &mut std::collections::HashSet<String>,
    selected: Option<&str>,
    width: usize,
    theme: &Theme,
) -> Vec<TimelineLine> {
    let dim = Style::new().fg(theme.dim);
    let mut lines: Vec<TimelineLine> = Vec::new();
    // (sender, timestamp) of the previous chat message, for grouping.
    let mut previous: Option<(&str, u64)> = None;

    for item in items {
        match &item.kind {
            ItemKind::DateDivider => {
                previous = None;
                if !lines.is_empty() {
                    lines.push(TimelineLine::plain(Line::default()));
                }
                let label = format!("── {} ──", long_date(item.ts_ms));
                lines.push(TimelineLine::plain(
                    Line::from(Span::styled(label, dim)).alignment(Alignment::Center),
                ));
                continue;
            }
            ItemKind::ReadMarker => {
                previous = None;
                lines.push(TimelineLine::plain(
                    Line::from(Span::styled("── new ──", Style::new().fg(theme.unread)))
                        .alignment(Alignment::Center),
                ));
                continue;
            }
            ItemKind::State if !show_room_events => continue,
            ItemKind::State => {
                previous = None;
                for text in wrap(&item.body, width.saturating_sub(4)) {
                    lines.push(TimelineLine::plain(Line::from(Span::styled(
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
                lines.push(TimelineLine::plain(Line::default()));
            }
            lines.push(TimelineLine::plain(Line::from(vec![
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
            lines.push(TimelineLine::of(
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
            ItemKind::Redacted => lines.push(TimelineLine::plain(Line::from(Span::styled(
                "  message deleted",
                dim.add_modifier(Modifier::ITALIC),
            )))),
            ItemKind::Undecryptable => lines.push(TimelineLine::of(
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
                let mut body: Vec<TimelineLine> = Vec::new();
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
                                    let cells = c.width().unwrap_or(0);
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
                                let mut entry = TimelineLine::of(item, Line::from(spans));
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
                            let mut entry = TimelineLine::of(
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
                                    body.push(TimelineLine::of(
                                        item,
                                        Line::from(vec![
                                            Span::styled("  │ ", dim),
                                            Span::styled(piece, code_style),
                                        ]),
                                    ));
                                }
                            }
                            body.push(TimelineLine::of(item, Line::from(Span::styled("  └", dim))));
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
                    let mut toggle = TimelineLine::of(
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

        if item.image
            && let Some(preview) = previews.get(&item.event_id)
        {
            // A Sixel picture is painted over rows kept free for it, once
            // all of them are in view.
            let as_sixel = sixel.contains_key(&item.event_id) && preview.png_columns + 2 <= width;
            if as_sixel {
                for row in 0..preview.png_rows {
                    let mut entry = TimelineLine::of(item, Line::default());
                    entry.picture = Some((
                        item.event_id.clone(),
                        row,
                        preview.png_rows,
                        preview.png_columns,
                    ));
                    lines.push(entry);
                }
            } else {
                let drawn = match terminal_images.get(&item.event_id) {
                    Some(id) => terminal_image_lines(preview, *id, width.saturating_sub(2)),
                    None => preview_lines(preview, width.saturating_sub(2)),
                };
                for line in drawn {
                    lines.push(TimelineLine::of(item, line));
                }
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
            let mut entry = TimelineLine::of(
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
                let mut entry = TimelineLine::of(
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
            let mut entry = TimelineLine::of(
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
            lines.push(TimelineLine::of(
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
fn reaction_row(item: &Item, named: bool, width: usize, theme: &Theme) -> Vec<TimelineLine> {
    let mut lines = vec![TimelineLine::of(item, Line::from(Span::raw("  ")))];
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
            lines.push(TimelineLine::of(item, Line::from(Span::raw("  "))));
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
fn reaction_lists(item: &Item, width: usize, theme: &Theme) -> Vec<TimelineLine> {
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
            let mut entry = TimelineLine::of(
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
fn less_line(item: &Item, theme: &Theme) -> TimelineLine {
    let mut entry = TimelineLine::of(
        item,
        Line::from(Span::styled("  ▴ less", Style::new().fg(theme.accent))),
    );
    entry.action = Some(HitAction::ToggleExpand(item.event_id.clone()));
    entry
}

/// `✓ read by Amy, Ben, Cleo +2` on the right, `open` with all names
/// (over several lines when needed).
fn read_by_lines(item: &Item, open: bool, width: usize, theme: &Theme) -> Vec<TimelineLine> {
    let dim = Style::new().fg(theme.dim);
    let room = width.saturating_sub(2);
    let right = |text: String, more: Option<String>| {
        let more_width = more.as_ref().map_or(0, |more| more.width());
        let text = fit(&text, room.saturating_sub(more_width));
        let gap = width.saturating_sub(text.width() + more_width);
        let mut entry = TimelineLine::of(
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

/// Reserve the cells for a picture the terminal draws itself (kitty
/// graphics): each cell holds a placeholder that names the picture (by
/// its colour) and which part of it belongs there.
fn terminal_image_lines(preview: &Preview, id: u32, max_width: usize) -> Vec<Line<'static>> {
    let columns = preview
        .png_columns
        .min(max_width)
        .min(crate::kitty::MAX_CELLS);
    let (r, g, b) = crate::kitty::id_color(id);
    let style = Style::new().fg(Color::Rgb(r, g, b));
    (0..preview.png_rows.min(crate::kitty::MAX_CELLS))
        .map(|row| {
            let mut spans = vec![Span::raw("  ")];
            spans.extend(
                (0..columns)
                    .map(|column| Span::styled(crate::kitty::placeholder(row, column), style)),
            );
            Line::from(spans)
        })
        .collect()
}

/// Draw a picture with half-block characters: each cell shows the upper
/// pixel as foreground and the lower one as background of `▀`.
fn preview_lines(preview: &Preview, max_width: usize) -> Vec<Line<'static>> {
    let width = preview.width.min(max_width);
    let visible = |pixel: [u8; 4]| pixel[3] >= 128;
    let color = |pixel: [u8; 4]| Color::Rgb(pixel[0], pixel[1], pixel[2]);
    (0..preview.rows())
        .map(|row| {
            let mut spans = vec![Span::raw("  ")];
            for x in 0..width {
                let top = preview.pixel(x, row * 2);
                let bottom = preview.pixel(x, row * 2 + 1);
                spans.push(match (visible(top), visible(bottom)) {
                    (true, true) => {
                        Span::styled("▀", Style::new().fg(color(top)).bg(color(bottom)))
                    }
                    (true, false) => Span::styled("▀", Style::new().fg(color(top))),
                    (false, true) => Span::styled("▄", Style::new().fg(color(bottom))),
                    (false, false) => Span::raw(" "),
                });
            }
            Line::from(spans)
        })
        .collect()
}

fn push_poll(lines: &mut Vec<TimelineLine>, item: &Item, poll: &Poll, width: usize, theme: &Theme) {
    let dim = Style::new().fg(theme.dim);
    let can_vote = !poll.is_ended && !item.event_id.is_empty();

    for text in wrap(&poll.question, width.saturating_sub(4)) {
        lines.push(TimelineLine::of(
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
            let mut entry = TimelineLine::of(item, Line::from(spans));
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
                let mut entry = TimelineLine::of(item, Line::from(spans));
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
    let mut entry = TimelineLine::of(item, Line::default());
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

/// The strip of buttons for the selected message: (label, action) pairs.
fn bar_buttons(app: &App) -> (Option<&'static str>, Vec<(String, BarAction)>) {
    let Some(item) = app.selected_item() else {
        return (None, Vec::new());
    };
    match app.bar {
        Bar::Actions => {
            let mut buttons: Vec<(String, BarAction)> = app
                .message_actions(item)
                .into_iter()
                .map(|offer| (format!("{} {}", offer.key, offer.label), offer.action))
                .collect();
            buttons.push(("Esc Close".to_owned(), BarAction::Cancel));
            (None, buttons)
        }
        Bar::Emoji => {
            let mut buttons: Vec<(String, BarAction)> = app
                .quick_reactions()
                .into_iter()
                .enumerate()
                .map(|(index, emoji)| (format!("{} {emoji}", index + 1), BarAction::Emoji(emoji)))
                .collect();
            buttons.push(("0 More…".to_owned(), BarAction::MoreEmoji));
            buttons.push(("Esc Back".to_owned(), BarAction::Cancel));
            (None, buttons)
        }
        Bar::ConfirmDelete => (
            Some("Delete this message?"),
            vec![
                ("Enter Delete".to_owned(), BarAction::ConfirmDelete),
                ("Esc Cancel".to_owned(), BarAction::Cancel),
            ],
        ),
        Bar::ConfirmEndPoll => (
            Some("End this poll for everyone?"),
            vec![
                ("Enter End poll".to_owned(), BarAction::ConfirmEndPoll),
                ("Esc Cancel".to_owned(), BarAction::Cancel),
            ],
        ),
    }
}

fn draw_bar(frame: &mut Frame, app: &mut App, rect: Rect) {
    let theme = app.theme;
    let (question, buttons) = bar_buttons(app);
    let mut spans = vec![Span::raw(" ")];
    let mut column = 1usize;
    if let Some(question) = question {
        spans.push(Span::styled(question, Style::new().fg(theme.unread)));
        spans.push(Span::raw("  "));
        column += question.width() + 2;
    }
    let mut hits = Vec::new();
    for (label, action) in buttons {
        let text = format!(" {label} ");
        let width = text.width();
        if column + width > rect.width as usize {
            break;
        }
        hits.push(Hit {
            rect: Rect {
                x: rect.x + column as u16,
                width: width as u16,
                ..rect
            },
            action: HitAction::Bar(action),
        });
        spans.push(Span::styled(
            text,
            Style::new().fg(theme.selection_fg).bg(theme.selection_bg),
        ));
        spans.push(Span::raw(" "));
        column += width + 1;
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), rect);
    app.hits.extend(hits);
}

fn draw_timeline(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = app.theme;
    let mut block = panel(&theme, app.focus == Focus::Timeline);
    let mut back: Option<u16> = None;
    if let Some(room) = app.open_room_entry() {
        let mut title = Vec::new();
        if app.state.open_thread.is_some() {
            // The way back to the room, also for the mouse.
            let label = if app.state.open_thread.as_deref() == Some(crate::matrix::PINNED_VIEW) {
                " ← Pinned in"
            } else {
                " ← Thread in"
            };
            back = Some(label.width() as u16);
            title.push(Span::styled(
                label,
                Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
            ));
        }
        title.push(Span::styled(
            format!(
                " {} ",
                fit(&room.name, area.width.saturating_sub(6) as usize / 2)
            ),
            Style::new().fg(theme.fg).add_modifier(Modifier::BOLD),
        ));
        let topic = room.topic.lines().next().unwrap_or_default().trim();
        if !topic.is_empty() && back.is_none() {
            let room_width = title[0].content.width();
            let space = (area.width as usize).saturating_sub(room_width + 6);
            title.push(Span::styled(
                format!("{} ", fit(topic, space)),
                Style::new().fg(theme.dim),
            ));
        }
        block = block.title(Line::from(title));
    }
    if !app.viewport.at_bottom() {
        block = block.title_bottom(
            Line::from(Span::styled(JUMP_LABEL, Style::new().fg(theme.unread)))
                .alignment(Alignment::Right),
        );
    }
    if !app.state.typing.is_empty() {
        let who = match &app.state.typing[..] {
            [one] => format!("{one} is writing…"),
            [one, two] => format!("{one} and {two} are writing…"),
            [one, rest @ ..] => format!("{one} and {} others are writing…", rest.len()),
            [] => String::new(),
        };
        let room = (area.width as usize).saturating_sub(JUMP_LABEL.width() + 6);
        block = block.title_bottom(Line::from(Span::styled(
            format!(" {} ", fit(&who, room)),
            Style::new().fg(theme.dim).add_modifier(Modifier::ITALIC),
        )));
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.timeline_area = area;
    app.hits.push(Hit {
        rect: area,
        action: HitAction::FocusTimeline,
    });
    if let Some(width) = back {
        app.hits.push(Hit {
            rect: Rect {
                x: area.x + 1,
                y: area.y,
                width: width.min(area.width.saturating_sub(2)),
                height: 1,
            },
            action: HitAction::CloseThread,
        });
    }
    if inner.width < 10 || inner.height == 0 {
        return;
    }

    if app.state.open_room.is_none() {
        // The start screen: the logo (or the text moth), the name, and how
        // to begin.
        let mut hint: Vec<Line> = Vec::new();
        let fits = |rows: usize, columns: usize| {
            rows + 7 <= inner.height as usize && columns + 2 <= inner.width as usize
        };
        let logo = app.previews.get(crate::logo::KEY).cloned();
        let logo = logo.and_then(|logo| {
            let (rows, columns) = match app.graphics {
                crate::app::Graphics::Kitty | crate::app::Graphics::Sixel => {
                    (logo.png_rows, logo.png_columns)
                }
                crate::app::Graphics::Blocks => (logo.rows(), logo.width),
                crate::app::Graphics::Off => return None,
            };
            fits(rows, columns).then_some((logo, rows, columns))
        });
        // Leave the moth out on small screens rather than squeeze the text.
        if let Some((logo, rows, columns)) = logo {
            hint.push(Line::default());
            let width = inner.width as usize;
            let mut lines = match app.graphics {
                crate::app::Graphics::Kitty => match app.terminal_images.get(crate::logo::KEY) {
                    Some(id) => terminal_image_lines(&logo, *id, width),
                    None => vec![Line::default(); rows],
                },
                crate::app::Graphics::Sixel => {
                    if !app.dialog_open() {
                        app.sixel_placements.push(crate::app::SixelPlacement {
                            x: inner.x + (inner.width - columns as u16) / 2,
                            y: inner.y + 1,
                            columns: columns as u16,
                            rows: rows as u16,
                            event_id: crate::logo::KEY.to_owned(),
                            selected: false,
                        });
                    }
                    vec![Line::default(); rows]
                }
                _ => preview_lines(&logo, width),
            };
            for line in &mut lines {
                // Without the indent pictures in messages have.
                if line.spans.first().is_some_and(|span| span.content == "  ") {
                    line.spans.remove(0);
                }
            }
            hint.extend(
                lines
                    .into_iter()
                    .map(|line| line.alignment(Alignment::Center)),
            );
        } else if fits(MOTH.len(), MOTH_WIDTH) {
            hint.push(Line::default());
            hint.extend(
                moth_lines(&theme)
                    .into_iter()
                    .map(|line| line.alignment(Alignment::Center)),
            );
        }
        hint.extend([
            Line::default(),
            Line::from(Span::styled(
                "m o t h r i x",
                Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
            ))
            .alignment(Alignment::Center),
            Line::default(),
            Line::from(Span::styled(
                "Pick a room on the left",
                Style::new().fg(theme.fg),
            ))
            .alignment(Alignment::Center),
            Line::from(Span::styled(
                "click it, or use ↑ ↓ and Enter · Ctrl+K to search",
                Style::new().fg(theme.dim),
            ))
            .alignment(Alignment::Center),
        ]);
        frame.render_widget(Paragraph::new(hint), inner);
        app.viewport.empty(inner.height as usize);
        return;
    }

    // A selected message gets a strip of buttons on the panel's last line.
    let has_bar = app.selected_item().is_some() && inner.height > 3;
    let view = if has_bar {
        Rect {
            height: inner.height - 1,
            ..inner
        }
    } else {
        inner
    };

    let mut collapsible = std::mem::take(&mut app.collapsible);
    collapsible.clear();
    let lines = timeline_lines(
        &app.state.items,
        &app.previews,
        &app.terminal_images,
        &app.sixel,
        app.state.prefs.room_events,
        app.state.prefs.read_receipts,
        &app.state.link_previews,
        app.state.prefs.long_messages,
        &app.expanded,
        &mut collapsible,
        app.selected.as_deref(),
        view.width as usize,
        &theme,
    );
    app.collapsible = collapsible;
    let total = lines.len();
    let height = view.height as usize;
    let newest = app
        .state
        .items
        .last()
        .map(|item| item.key.clone())
        .unwrap_or_default();
    let selected = app.selected.as_deref().and_then(|selected| {
        let is_selected = |entry: &TimelineLine| entry.item.as_deref() == Some(selected);
        Some((
            lines.iter().position(is_selected)?,
            lines.iter().rposition(is_selected)?,
        ))
    });
    let shown = app.viewport.laid_out(total, height, &newest, selected);

    if total == 0 {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "  Loading messages…",
                Style::new().fg(theme.dim),
            )),
            view,
        );
    } else {
        let (start, end) = (shown.start, shown.end);
        // Short timelines sit at the bottom of the panel, next to the composer.
        let top_padding = height - (end - start);
        let mut text = vec![Line::default(); top_padding];
        for (offset, mut entry) in lines.into_iter().skip(start).take(end - start).enumerate() {
            let rect = Rect {
                x: view.x,
                y: view.y + (top_padding + offset) as u16,
                width: view.width,
                height: 1,
            };
            let selected = entry.item.is_some() && entry.item == app.selected;
            // The last row of a picture that is in view from its first:
            // it can be painted.
            if let Some((event_id, row, rows, columns)) = entry.picture.take()
                && row + 1 == rows
                && offset + 1 >= rows
                && !app.dialog_open()
            {
                app.sixel_placements.push(crate::app::SixelPlacement {
                    x: view.x + 2,
                    y: rect.y + 1 - rows as u16,
                    columns: columns as u16,
                    rows: rows as u16,
                    event_id,
                    selected,
                });
            }
            if let Some(action) = entry.action {
                app.hits.push(Hit { rect, action });
            } else if let Some(event_id) = entry.item {
                app.hits.push(Hit {
                    rect,
                    action: HitAction::Select(event_id),
                });
            }
            for (column, width, action) in entry.spots {
                if column < view.width as usize {
                    let width = width.min(view.width as usize - column) as u16;
                    app.hits.push(Hit {
                        rect: Rect {
                            x: view.x + column as u16,
                            width,
                            ..rect
                        },
                        action,
                    });
                }
            }
            text.push(if selected {
                entry.line.style(Style::new().bg(theme.selection_bg))
            } else {
                entry.line
            });
        }
        frame.render_widget(Paragraph::new(text), view);
    }

    if has_bar {
        draw_bar(
            frame,
            app,
            Rect {
                y: inner.bottom() - 1,
                height: 1,
                ..inner
            },
        );
    }

    if !app.viewport.at_bottom() {
        let width = JUMP_LABEL.width() as u16;
        let rect = Rect {
            x: area.right().saturating_sub(width + 1),
            y: area.bottom().saturating_sub(1),
            width,
            height: 1,
        };
        app.hits.push(Hit {
            rect,
            action: HitAction::JumpToBottom,
        });
    }
}

// ---------------------------------------------------------------------------
// Composer
// ---------------------------------------------------------------------------

/// Break the composer text into rows of at most `width` cells and locate the
/// cursor: returns (rows, cursor row, cursor column in cells).
fn composer_layout(text: &str, cursor: usize, width: u16) -> (Vec<String>, usize, u16) {
    let width = width.max(1) as usize;
    let mut rows = vec![String::new()];
    let mut used = 0usize;
    let (mut cursor_row, mut cursor_col) = (0usize, 0u16);
    for (index, c) in text.chars().enumerate() {
        // A line break stays at the end of its row (it is not drawn), so
        // that positions in the rows still add up to positions in the text.
        if c == '\n' {
            if index == cursor {
                cursor_row = rows.len() - 1;
                cursor_col = used as u16;
            }
            rows.last_mut().expect("rows is never empty").push(c);
            rows.push(String::new());
            used = 0;
            continue;
        }
        let w = c.width().unwrap_or(0);
        if used + w > width {
            rows.push(String::new());
            used = 0;
        }
        if index == cursor {
            cursor_row = rows.len() - 1;
            cursor_col = used as u16;
        }
        rows.last_mut().expect("rows is never empty").push(c);
        used += w;
    }
    if cursor >= text.chars().count() {
        if used >= width {
            rows.push(String::new());
            used = 0;
        }
        cursor_row = rows.len() - 1;
        cursor_col = used as u16;
    }
    (rows, cursor_row, cursor_col)
}

fn draw_composer(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = app.theme;
    let focused = app.focus == Focus::Composer;
    let title_color = if focused { theme.accent } else { theme.dim };
    let title_room = (area.width as usize)
        .saturating_sub(POLL_BUTTON.width() + FILE_BUTTON.width() + EMOJI_BUTTON.width() + 10);
    let (title, cancellable) = match &app.compose {
        Compose::Message
            if app.state.open_thread.as_deref() == Some(crate::matrix::PINNED_VIEW) =>
        {
            (" Pinned messages · Esc goes back ".to_owned(), false)
        }
        Compose::Message if app.state.open_thread.is_some() => {
            (" Reply in thread · Esc leaves it ".to_owned(), false)
        }
        Compose::Message => (" Message ".to_owned(), false),
        Compose::Reply { label, .. } => (
            format!(
                " Reply to {} · Esc cancels ",
                fit(label, title_room.saturating_sub(26))
            ),
            true,
        ),
        Compose::Edit { .. } => (" Edit message · Esc cancels ".to_owned(), true),
    };
    let title = fit(&title, title_room.max(9));
    let title_width = title.width() as u16;
    let mut block = panel(&theme, focused).title(Line::from(Span::styled(
        title,
        Style::new().fg(title_color),
    )));
    let buttons_width = EMOJI_BUTTON.width() + FILE_BUTTON.width() + POLL_BUTTON.width() + 2;
    let show_buttons = app.state.open_room.is_some() && area.width as usize > buttons_width + 24;
    if show_buttons {
        block = block.title(
            Line::from(vec![
                Span::styled(EMOJI_BUTTON, Style::new().fg(theme.accent)),
                Span::raw(" "),
                Span::styled(FILE_BUTTON, Style::new().fg(theme.accent)),
                Span::raw(" "),
                Span::styled(POLL_BUTTON, Style::new().fg(theme.accent)),
            ])
            .alignment(Alignment::Right),
        );
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.hits.push(Hit {
        rect: area,
        action: HitAction::FocusComposer,
    });
    if cancellable {
        // Clicking the title backs out of the reply/edit.
        app.hits.push(Hit {
            rect: Rect {
                x: area.x + 1,
                y: area.y,
                width: title_width,
                height: 1,
            },
            action: HitAction::CancelCompose,
        });
    }
    if show_buttons {
        let poll_width = POLL_BUTTON.width() as u16;
        let file_width = FILE_BUTTON.width() as u16;
        let poll_x = area.right() - poll_width - 1;
        app.hits.push(Hit {
            rect: Rect {
                x: poll_x,
                y: area.y,
                width: poll_width,
                height: 1,
            },
            action: HitAction::PollTemplate,
        });
        app.hits.push(Hit {
            rect: Rect {
                x: poll_x - 1 - file_width,
                y: area.y,
                width: file_width,
                height: 1,
            },
            action: HitAction::FileTemplate,
        });
        let emoji_width = EMOJI_BUTTON.width() as u16;
        app.hits.push(Hit {
            rect: Rect {
                x: poll_x - 2 - file_width - emoji_width,
                y: area.y,
                width: emoji_width,
                height: 1,
            },
            action: HitAction::OpenEmojiPicker,
        });
    }
    if inner.width < 4 || inner.height == 0 {
        return;
    }
    let text_area = Rect {
        x: inner.x + 1,
        width: inner.width - 2,
        ..inner
    };

    app.composer_text_area = text_area;
    app.composer_first_row = 0;
    if app.composer.is_empty() && !focused {
        let hint = if app.state.open_room.is_some() {
            "Write a message…"
        } else {
            "Open a room to write"
        };
        frame.render_widget(
            Paragraph::new(Span::styled(hint, Style::new().fg(theme.dim))),
            text_area,
        );
        return;
    }

    let (rows, cursor_row, cursor_col) =
        composer_layout(app.composer.text(), app.composer.cursor(), text_area.width);
    let height = text_area.height as usize;
    // Keep the row with the cursor in view.
    let first = (cursor_row + 1).saturating_sub(height);
    app.composer_text_area = text_area;
    app.composer_first_row = first;

    // Misspelled words get a red underline, grammar findings another colour.
    let normal = Style::new().fg(theme.fg);
    let style_of = |kind: u8| match kind {
        1 => Style::new()
            .fg(theme.error)
            .add_modifier(Modifier::UNDERLINED),
        2 => Style::new()
            .fg(theme.unread)
            .add_modifier(Modifier::UNDERLINED),
        _ => normal,
    };
    let within = |marks: &[(usize, usize)], index: usize| {
        marks
            .iter()
            .any(|(start, end)| (*start..*end).contains(&index))
    };
    let kind_at = |index: usize| {
        if within(&app.misspelled, index) {
            1
        } else if within(&app.grammar_marks, index) {
            2
        } else {
            0
        }
    };
    let mut char_index = 0;
    let mut visible: Vec<Line> = Vec::new();
    for (row_index, row) in rows.iter().enumerate() {
        let mut spans: Vec<Span> = Vec::new();
        let mut run = String::new();
        let mut run_kind = 0u8;
        for c in row.chars() {
            let kind = kind_at(char_index);
            if kind != run_kind && !run.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut run), style_of(run_kind)));
            }
            run_kind = kind;
            if c != '\n' {
                run.push(c);
            }
            char_index += 1;
        }
        if !run.is_empty() {
            spans.push(Span::styled(run, style_of(run_kind)));
        }
        if row_index >= first && visible.len() < height {
            visible.push(Line::from(spans));
        }
    }
    frame.render_widget(Paragraph::new(visible), text_area);
    if focused {
        frame.set_cursor_position((
            text_area.x + cursor_col,
            text_area.y + (cursor_row - first) as u16,
        ));
    }
}

/// The char index in `text` shown at (`row`, `column`) of the message box,
/// for placing the cursor with a click. Clicks past the end of a row land
/// at its end.
pub fn composer_index_at(text: &str, width: u16, row: usize, column: u16) -> usize {
    let (rows, _, _) = composer_layout(text, 0, width);
    let row = row.min(rows.len() - 1);
    let before: usize = rows[..row].iter().map(|line| line.chars().count()).sum();
    let mut used = 0;
    let mut within = 0;
    for c in rows[row].chars() {
        let w = c.width().unwrap_or(0);
        if c == '\n' || used + w > column as usize {
            break;
        }
        used += w;
        within += 1;
    }
    before + within
}

use mothrix_core::art::{MOTH, MOTH_WIDTH, Part};

/// The moth in the theme's colours: wings in the accent colour, the body
/// plain, eyes and diamonds standing out, moon and stars dim.
fn moth_lines(theme: &Theme) -> Vec<Line<'static>> {
    MOTH.iter()
        .enumerate()
        .map(|(row, text)| {
            let spans: Vec<Span> = text
                .chars()
                .enumerate()
                .map(|(column, c)| {
                    let color = match mothrix_core::art::part(row, column, c) {
                        Part::Eye => theme.unread,
                        Part::Gem => theme.success,
                        Part::Sky => theme.dim,
                        Part::Body => theme.fg,
                        Part::Wing => theme.accent,
                    };
                    Span::styled(c.to_string(), Style::new().fg(color))
                })
                .collect();
            Line::from(spans)
        })
        .collect()
}

/// The @-mention suggestions, floating just above the message box.
fn draw_mentions(frame: &mut Frame, app: &mut App, composer: Rect) {
    let crate::app::Popup {
        title,
        rows,
        selected,
    } = app.popup();
    if rows.is_empty() {
        return;
    }
    let theme = app.theme;
    let height = rows.len() as u16 + 2;
    let widest = rows
        .iter()
        .map(|(main, detail)| main.width() + detail.width() + 4)
        .max()
        .unwrap_or(0) as u16;
    let width = (widest + 2)
        .max(title.width() as u16 + 2)
        .clamp(24, composer.width.saturating_sub(4).max(24));
    if composer.y < height || composer.width < width + 2 {
        return;
    }
    let rect = Rect {
        x: composer.x + 2,
        y: composer.y - height,
        width,
        height,
    };
    frame.render_widget(Clear, rect);
    let block = panel(&theme, true).title(Line::from(Span::styled(
        title,
        Style::new().fg(theme.accent),
    )));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    for (index, (main, detail)) in rows.iter().enumerate() {
        let row = Rect {
            y: inner.y + index as u16,
            height: 1,
            ..inner
        };
        let mut style = Style::new().fg(theme.fg);
        if Some(index) == selected {
            style = style.bg(theme.selection_bg).fg(theme.selection_fg);
        }
        let main = fit(main, inner.width as usize - 2);
        let rest = (inner.width as usize).saturating_sub(main.width() + 3);
        let line = Line::from(vec![
            Span::styled(format!(" {main} "), style.add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("{:<rest$}", fit(detail, rest)),
                style.fg(if Some(index) == selected {
                    theme.selection_fg
                } else {
                    theme.dim
                }),
            ),
        ]);
        frame.render_widget(Paragraph::new(line).style(style), row);
        app.hits.push(Hit {
            rect: row,
            action: HitAction::MentionPick(index),
        });
    }
}

/// The device verification dialog, centred over everything else.
fn draw_verify(frame: &mut Frame, app: &mut App, area: Rect, state: VerifyState) {
    let theme = app.theme;
    let text = Style::new().fg(theme.fg);
    let dim = Style::new().fg(theme.dim);

    let mut lines: Vec<Line> = vec![Line::default()];
    let buttons: Vec<(&str, VerifyInput)> = match &state {
        VerifyState::Incoming { device } => {
            lines.push(Line::from(Span::styled(
                format!("{device} asks to verify."),
                text,
            )));
            lines.push(Line::from(Span::styled(
                "Accept to compare emoji on both sides.",
                dim,
            )));
            vec![
                ("Enter Accept", VerifyInput::Accept),
                ("Esc Decline", VerifyInput::Cancel),
            ]
        }
        VerifyState::Waiting => {
            lines.push(Line::from(Span::styled(
                "Waiting for the other side…",
                text,
            )));
            lines.push(Line::from(Span::styled("Accept the request there.", dim)));
            vec![("Esc Cancel", VerifyInput::Cancel)]
        }
        VerifyState::Emojis(emojis) => {
            lines.push(Line::from(Span::styled(
                "Do both devices show these emoji, in this order?",
                text,
            )));
            lines.push(Line::default());
            for (symbol, name) in emojis {
                lines.push(Line::from(vec![
                    Span::raw(format!("   {symbol}  ")),
                    Span::styled(name.clone(), text),
                ]));
            }
            vec![
                ("Enter They match", VerifyInput::Confirm),
                ("Esc They differ", VerifyInput::Cancel),
            ]
        }
        VerifyState::Confirming => {
            lines.push(Line::from(Span::styled(
                "Waiting for the other device to confirm…",
                text,
            )));
            vec![("Esc Cancel", VerifyInput::Cancel)]
        }
        VerifyState::Done => {
            lines.push(Line::from(Span::styled(
                "✓ This device is verified.",
                Style::new().fg(theme.success).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(Span::styled(
                "Encrypted history can now be shared with it.",
                dim,
            )));
            vec![("Enter Close", VerifyInput::Cancel)]
        }
        VerifyState::Failed(reason) => {
            lines.push(Line::from(Span::styled(
                "Verification did not complete.",
                Style::new().fg(theme.error).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(Span::styled(reason.clone(), dim)));
            vec![("Enter Close", VerifyInput::Cancel)]
        }
    };
    lines.push(Line::default());

    let width = 58.min(area.width.saturating_sub(2));
    let height = (lines.len() as u16 + 3).min(area.height.saturating_sub(2));
    if width < 30 || height < 6 {
        return;
    }
    let rect = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, rect);
    let block = panel(&theme, true).title(Line::from(Span::styled(
        " Verify ",
        Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
    )));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let body = Rect {
        x: inner.x + 2,
        width: inner.width.saturating_sub(4),
        ..inner
    };
    frame.render_widget(Paragraph::new(lines), body);

    let button_row = Rect {
        y: inner.bottom() - 1,
        height: 1,
        ..body
    };
    let mut spans = Vec::new();
    let mut column = 0u16;
    for (label, input) in buttons {
        let label = format!(" {label} ");
        let label_width = label.width() as u16;
        app.hits.push(Hit {
            rect: Rect {
                x: button_row.x + column,
                width: label_width,
                ..button_row
            },
            action: HitAction::Verify(input),
        });
        spans.push(Span::styled(
            label,
            Style::new().fg(theme.selection_fg).bg(theme.selection_bg),
        ));
        spans.push(Span::raw("  "));
        column += label_width + 2;
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), button_row);
}

// ---------------------------------------------------------------------------
// Status bar
// ---------------------------------------------------------------------------

fn draw_status(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = app.theme;
    let dim = Style::new().fg(theme.dim);
    let account = app.account();
    let (dot, label) = match &account.sync {
        SyncStatus::Online => (
            Span::styled(" ● ", Style::new().fg(theme.success)),
            "online".to_owned(),
        ),
        SyncStatus::Connecting => (Span::styled(" ○ ", dim), "connecting…".to_owned()),
        SyncStatus::Offline => (
            Span::styled(" ● ", Style::new().fg(theme.unread)),
            "offline".to_owned(),
        ),
        SyncStatus::Error(error) => (
            Span::styled(" ✗ ", Style::new().fg(theme.error)),
            format!("sync error: {error}"),
        ),
    };
    let mut left = vec![
        dot,
        Span::styled(label, dim),
        // The name people see, rather than the (possibly very long) id.
        Span::styled(
            format!(
                "  {}",
                match &account.own_name {
                    Some(name) => fit(name, 30),
                    None => crate::app::short_id(&account.user_id, 30),
                }
            ),
            dim,
        ),
    ];
    if account.verified == Some(false) {
        left.push(Span::styled(
            "  unverified device · /verify",
            Style::new().fg(theme.unread),
        ));
    }
    if let Some(notice) = &app.notice {
        left.push(Span::styled(
            format!("  {notice}"),
            Style::new().fg(theme.unread),
        ));
    }
    let left_width: usize = left.iter().map(|span| span.content.width()).sum();
    frame.render_widget(Paragraph::new(Line::from(left)), area);

    let hints = match app.focus {
        Focus::Sidebar if app.state.accounts.len() > 1 => {
            "↑↓ Enter open · a account · U unread · p pin · s sort · z fold · m mix · "
        }
        Focus::Sidebar => {
            "↑↓ Enter open · U unread · p pin · s sort · z fold · m mix · g section · "
        }
        Focus::Timeline => "↑↓ select · r reply · a react · c copy · e edit · d delete · ",
        Focus::Composer => "Alt+Enter new line · ↑ select message · ^F search · F2 settings · ",
    };
    let theme_label = format!("^T {} ", theme.name);
    let search_label = " search ";
    let settings_label = " settings ";
    let buttons_width =
        (search_label.width() + settings_label.width() + theme_label.width()) as u16;
    let right_width = hints.width() as u16 + buttons_width;
    // Keep at least the buttons when space is tight.
    let (hints, right_width) = if left_width as u16 + right_width + 2 > area.width {
        ("", buttons_width)
    } else {
        (hints, right_width)
    };
    if left_width as u16 + right_width + 2 > area.width {
        return;
    }
    let rect = Rect {
        x: area.right() - right_width,
        width: right_width,
        ..area
    };
    let button = Style::new().fg(theme.accent);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(hints, dim),
            Span::styled(search_label, button),
            Span::styled(settings_label, button),
            Span::styled(theme_label.clone(), button),
        ])),
        rect,
    );
    let mut x = area.right() - buttons_width;
    for (label, action) in [
        (search_label, HitAction::OpenSearch),
        (settings_label, HitAction::OpenSettings),
        (theme_label.as_str(), HitAction::NextTheme),
    ] {
        let width = label.width() as u16;
        app.hits.push(Hit {
            rect: Rect { x, width, ..area },
            action,
        });
        x += width;
    }
}

/// A centred dialog box: clears what is underneath, keeps clicks inside
/// it from reaching anything but its own targets, and returns the area
/// inside the border.
fn dialog(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    title: &str,
    width: u16,
    height: u16,
) -> Option<Rect> {
    let theme = app.theme;
    let width = width.min(area.width.saturating_sub(2));
    let height = height.min(area.height.saturating_sub(2));
    if width < 30 || height < 6 {
        return None;
    }
    let rect = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    app.hits.push(Hit {
        rect,
        action: HitAction::Blocked,
    });
    frame.render_widget(Clear, rect);
    let block = panel(&theme, true).title(Line::from(Span::styled(
        format!(" {title} "),
        Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
    )));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    Some(Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(2),
        ..inner
    })
}

fn draw_settings(frame: &mut Frame, app: &mut App, area: Rect, settings: &SettingsDialog) {
    let theme = app.theme;
    let rows = Setting::ALL.len() as u16;
    let Some(inner) = dialog(frame, app, area, "Settings", 76, rows + 7) else {
        return;
    };
    let dim = Style::new().fg(theme.dim);
    let label_width = 26usize;
    let editing = settings.edit.clone();

    for (index, setting) in Setting::ALL.into_iter().enumerate() {
        let name = setting.label();
        let row = Rect {
            y: inner.y + 1 + index as u16,
            height: 1,
            ..inner
        };
        if row.y >= inner.bottom() {
            break;
        }
        let selected = index == settings.cursor;
        let mut style = Style::new().fg(theme.fg);
        if selected {
            style = style.bg(theme.selection_bg).fg(theme.selection_fg);
        }
        let value_width = (row.width as usize).saturating_sub(label_width + 2);
        let typed = editing.as_ref().filter(|_| index == settings.cursor);
        let value = match typed {
            Some(input) => {
                let (shown, column) = input.view(value_width);
                frame.set_cursor_position((row.x + 1 + label_width as u16 + column, row.y));
                shown
            }
            None => fit(&app.setting_value(setting), value_width),
        };
        let value_style = match typed {
            Some(_) => Style::new().fg(theme.fg),
            None => style.fg(theme.accent).add_modifier(Modifier::BOLD),
        };
        let line = Line::from(vec![
            Span::styled(format!(" {name:<label_width$}"), style),
            Span::styled(format!("{value:<value_width$}"), value_style),
        ]);
        frame.render_widget(Paragraph::new(line).style(style), row);
        app.hits.push(Hit {
            rect: row,
            action: HitAction::Setting(setting),
        });
    }

    let note_text = Setting::at(settings.cursor).note().unwrap_or_default();
    if !note_text.is_empty() && inner.height > rows + 3 {
        let note = Rect {
            y: inner.bottom() - 2,
            height: 1,
            ..inner
        };
        frame.render_widget(
            Paragraph::new(Span::styled(note_text, Style::new().fg(theme.mention))),
            note,
        );
    }
    let help = Rect {
        y: inner.bottom() - 1,
        height: 1,
        ..inner
    };
    let text = if editing.is_some() {
        " type or paste · Enter save · Esc cancel"
    } else {
        " ↑↓ choose · Enter or click to change · Esc close"
    };
    frame.render_widget(Paragraph::new(Span::styled(text, dim)), help);
    // Which mothrix this is, on the free line under the settings.
    let version = Rect {
        y: inner.bottom().saturating_sub(3),
        height: 1,
        ..inner
    };
    if version.y > inner.y + rows {
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!("mothrix {} · {} ", crate::VERSION, crate::repository()),
                dim,
            ))
            .alignment(Alignment::Right),
            version,
        );
    }
}

/// A row of clickable buttons, left to right from `x`.
fn draw_buttons(frame: &mut Frame, app: &mut App, row: Rect, buttons: &[(&str, HitAction)]) {
    let theme = app.theme;
    let mut x = row.x + 1;
    for (label, action) in buttons {
        let text = format!(" {label} ");
        let width = (text.width() as u16).min(row.right().saturating_sub(x));
        if width == 0 {
            break;
        }
        let rect = Rect {
            x,
            y: row.y,
            width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Span::styled(
                text,
                Style::new().fg(theme.selection_fg).bg(theme.selection_bg),
            )),
            rect,
        );
        app.hits.push(Hit {
            rect,
            action: action.clone(),
        });
        x += width + 1;
    }
}

/// The accounts dialog: who is signed in, and the form for one more.
fn draw_accounts(frame: &mut Frame, app: &mut App, area: Rect, state: AccountsDialog) {
    use crate::app::{ADD_FIELDS, AccountsAction, AccountsMode};

    let theme = app.theme;
    let dim = Style::new().fg(theme.dim);
    let line_at = |inner: Rect, offset: u16| Rect {
        y: inner.y + offset,
        height: 1,
        ..inner
    };

    if let AccountsMode::Add(form) = &state.mode {
        let Some(inner) = dialog(frame, app, area, "Add an account", 76, 16) else {
            return;
        };
        let label_width = 11usize;
        for (index, name) in ADD_FIELDS.iter().enumerate() {
            let row = line_at(inner, 1 + index as u16);
            let selected = index == form.field;
            let input = &form.fields[index];
            let value_width = (row.width as usize).saturating_sub(label_width + 2);
            let (shown, column) = if *name == "Password" {
                input.masked().view(value_width)
            } else {
                input.view(value_width)
            };
            if selected && !form.busy {
                frame.set_cursor_position((row.x + 1 + label_width as u16 + column, row.y));
            }
            let label_style = if selected {
                Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)
            } else {
                dim
            };
            let line = Line::from(vec![
                Span::styled(format!(" {name:<label_width$}"), label_style),
                Span::styled(
                    format!("{shown:<value_width$}"),
                    Style::new().fg(theme.fg).bg(theme.selection_bg),
                ),
            ]);
            frame.render_widget(Paragraph::new(line), row);
            app.hits.push(Hit {
                rect: row,
                action: HitAction::Accounts(AccountsAction::Field(index)),
            });
        }
        let hint = match form.field {
            0 => "Like matrix.org, or your full Matrix ID (@you:example.org).",
            1 | 2 => "Empty: sign in through the browser, if the server offers that.",
            _ => "Optional: what the account is called here (school, work …).",
        };
        frame.render_widget(
            Paragraph::new(Span::styled(format!(" {hint}"), dim)),
            line_at(inner, 6),
        );
        // What is going on; an address to open can take several lines.
        let status_style = Style::new().fg(if form.failed {
            theme.error
        } else {
            theme.mention
        });
        let width = (inner.width as usize).saturating_sub(2).max(10);
        for (offset, part) in wrap(&form.status, width).into_iter().take(4).enumerate() {
            frame.render_widget(
                Paragraph::new(Span::styled(format!(" {part}"), status_style)),
                line_at(inner, 8 + offset as u16),
            );
        }
        let help = line_at(inner, inner.height.saturating_sub(1));
        if form.busy {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    " signing in · Esc closes this, the sign-in goes on",
                    dim,
                )),
                help,
            );
        } else {
            draw_buttons(
                frame,
                app,
                help,
                &[
                    (
                        "Enter  sign in",
                        HitAction::Accounts(AccountsAction::Confirm),
                    ),
                    ("Esc  back", HitAction::Accounts(AccountsAction::Back)),
                ],
            );
        }
        return;
    }

    let count = app.state.accounts.len() as u16;
    let Some(inner) = dialog(frame, app, area, "Accounts", 76, count + 8) else {
        return;
    };
    let label_width = 14usize;
    for index in 0..app.state.accounts.len() {
        let row = line_at(inner, 1 + index as u16);
        if row.y + 4 > inner.bottom() {
            break;
        }
        let account = app.state.accounts[index].clone();
        let selected = index == state.cursor;
        let mut style = Style::new().fg(theme.fg);
        if selected {
            style = style.bg(theme.selection_bg).fg(theme.selection_fg);
        }
        let (dot, status, color) = match &account.sync {
            SyncStatus::Online => ("●", "online", theme.success),
            SyncStatus::Connecting => ("○", "connecting", theme.dim),
            SyncStatus::Offline => ("●", "offline", theme.unread),
            SyncStatus::Error(_) => ("✗", "error", theme.error),
        };
        let trust = match account.verified {
            Some(true) => "verified",
            Some(false) => "unverified",
            None => "",
        };
        let right = format!("{dot} {status:<10} {trust:<10}");
        let renaming = match &state.mode {
            AccountsMode::Rename(input) if selected => Some(input),
            _ => None,
        };
        let label = match renaming {
            Some(input) => {
                let (shown, column) = input.view(label_width - 1);
                frame.set_cursor_position((row.x + 1 + column, row.y));
                shown
            }
            None => fit(&account.label, label_width - 1),
        };
        let who_width = (row.width as usize).saturating_sub(1 + label_width + right.width() + 1);
        let who = match &account.own_name {
            Some(name) => format!("{name} · {}", account.user_id),
            None => account.user_id.clone(),
        };
        let who = if who.width() > who_width {
            crate::app::short_id(&who, who_width)
        } else {
            who
        };
        let label_style = match renaming {
            Some(_) => Style::new().fg(theme.fg),
            None => style.fg(theme.accent).add_modifier(Modifier::BOLD),
        };
        let line = Line::from(vec![
            Span::styled(" ", style),
            Span::styled(format!("{label:<label_width$}"), label_style),
            Span::styled(format!("{who:<who_width$} "), style),
            Span::styled(right, style.fg(color)),
        ]);
        frame.render_widget(Paragraph::new(line).style(style), row);
        app.hits.push(Hit {
            rect: row,
            action: HitAction::Accounts(AccountsAction::Select(index)),
        });
    }

    let note = line_at(inner, inner.height.saturating_sub(3));
    let buttons = line_at(inner, inner.height.saturating_sub(1));
    match &state.mode {
        AccountsMode::Remove => {
            let account = &app.state.accounts[state.cursor.min(app.state.accounts.len() - 1)];
            let text = format!(
                " Sign out {}? Its data on this computer is deleted, keys included.",
                account.label
            );
            frame.render_widget(
                Paragraph::new(Span::styled(text, Style::new().fg(theme.unread))),
                note,
            );
            draw_buttons(
                frame,
                app,
                buttons,
                &[
                    ("y  sign out", HitAction::Accounts(AccountsAction::Confirm)),
                    ("n  keep", HitAction::Accounts(AccountsAction::Back)),
                ],
            );
        }
        AccountsMode::Rename(_) => {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    " The name is only for you. Empty: back to the profile's name.",
                    dim,
                )),
                note,
            );
            draw_buttons(
                frame,
                app,
                buttons,
                &[
                    ("Enter  save", HitAction::Accounts(AccountsAction::Confirm)),
                    ("Esc  cancel", HitAction::Accounts(AccountsAction::Back)),
                ],
            );
        }
        _ => {
            if let SyncStatus::Error(error) = &app.state.accounts[state.cursor].sync {
                frame.render_widget(
                    Paragraph::new(Span::styled(
                        fit(&format!(" {error}"), inner.width as usize),
                        Style::new().fg(theme.error),
                    )),
                    note,
                );
            }
            draw_buttons(
                frame,
                app,
                buttons,
                &[
                    ("a  add", HitAction::Accounts(AccountsAction::Add)),
                    ("r  rename", HitAction::Accounts(AccountsAction::Rename)),
                    ("K  up", HitAction::Accounts(AccountsAction::Move(true))),
                    ("J  down", HitAction::Accounts(AccountsAction::Move(false))),
                    ("d  sign out", HitAction::Accounts(AccountsAction::Remove)),
                    ("Esc  close", HitAction::Accounts(AccountsAction::Back)),
                ],
            );
        }
    }
}

/// The new recovery key, and the password prompt of an identity reset.
fn draw_recovery(frame: &mut Frame, app: &mut App, area: Rect, key: &str) {
    let theme = app.theme;
    let dim = Style::new().fg(theme.dim);
    let line_at = |inner: Rect, offset: u16| Rect {
        y: inner.y + offset,
        height: 1,
        ..inner
    };
    let Some(inner) = dialog(frame, app, area, "Your recovery key", 72, 11) else {
        return;
    };
    let lines = [
        (
            " Write this down and keep it somewhere safe. It is shown only now.",
            dim,
        ),
        (
            " With it, a new device can read your older encrypted messages.",
            dim,
        ),
    ];
    for (offset, (text, style)) in lines.into_iter().enumerate() {
        frame.render_widget(
            Paragraph::new(Span::styled(text, style)),
            line_at(inner, offset as u16 + 1),
        );
    }
    frame.render_widget(
        Paragraph::new(Span::styled(
            format!(" {key}"),
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        ))
        .wrap(ratatui::widgets::Wrap { trim: false }),
        Rect {
            y: inner.y + 4,
            height: 2,
            ..inner
        },
    );
    frame.render_widget(
        Paragraph::new(Span::styled(" c copy · Esc close (when you have it)", dim)),
        line_at(inner, inner.height.saturating_sub(1)),
    );
}

/// The password prompt that confirms an identity reset.
fn draw_reset(frame: &mut Frame, app: &mut App, area: Rect, input: &Input) {
    let theme = app.theme;
    let dim = Style::new().fg(theme.dim);
    let line_at = |inner: Rect, offset: u16| Rect {
        y: inner.y + offset,
        height: 1,
        ..inner
    };
    let Some(inner) = dialog(frame, app, area, "Reset your identity", 72, 12) else {
        return;
    };
    let warn = Style::new().fg(theme.unread);
    let lines = [
        (
            " This is the last resort when no other device and no recovery key",
            warn,
        ),
        (
            " is left. Everyone who verified you has to do so again, and encrypted",
            warn,
        ),
        (
            " messages that only the old backup could open stay unreadable.",
            warn,
        ),
        ("", dim),
        (" Your account password confirms it:", dim),
    ];
    for (offset, (text, style)) in lines.into_iter().enumerate() {
        frame.render_widget(
            Paragraph::new(Span::styled(text, style)),
            line_at(inner, offset as u16 + 1),
        );
    }
    let (masked, column) = input
        .masked()
        .view((inner.width as usize).saturating_sub(2));
    frame.render_widget(
        Paragraph::new(Span::styled(
            format!(" {masked}"),
            Style::new().fg(theme.fg).bg(theme.selection_bg),
        )),
        line_at(inner, 7),
    );
    frame.set_cursor_position((inner.x + 1 + column, inner.y + 7));
    frame.render_widget(
        Paragraph::new(Span::styled(" Enter reset · Esc cancel", dim)),
        line_at(inner, inner.height.saturating_sub(1)),
    );
}

/// The help: every key and command, narrowed by what is typed.
fn draw_help(frame: &mut Frame, app: &mut App, area: Rect, help: crate::app::Help) {
    let theme = app.theme;
    let lines = app.help_shown();
    let Some(inner) = dialog(frame, app, area, "Help", 92, area.height.saturating_sub(2)) else {
        return;
    };
    let (query, column) = help.filter.view((inner.width as usize).saturating_sub(3));
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" / ", Style::new().fg(theme.accent)),
            Span::styled(query, Style::new().fg(theme.fg)),
        ])),
        Rect { height: 1, ..inner },
    );
    frame.set_cursor_position((inner.x + 3 + column, inner.y));

    // Between the filter line and the line of keys at the bottom.
    let rows = inner.height.saturating_sub(3) as usize;
    let first = help.scroll.min(lines.len().saturating_sub(rows));
    if let Some(state) = app.overlays.get_mut::<crate::app::Help>() {
        state.scroll = first;
    }
    let keys_width = 24usize;
    let text_width = (inner.width as usize).saturating_sub(keys_width + 3);
    for (offset, line) in lines.iter().skip(first).take(rows).enumerate() {
        let row = Rect {
            y: inner.y + 2 + offset as u16,
            height: 1,
            ..inner
        };
        let line = match line {
            crate::app::HelpLine::Section(title) => Line::from(Span::styled(
                format!(" {title}"),
                Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
            )),
            crate::app::HelpLine::Entry { keys, what } => Line::from(vec![
                Span::styled(
                    format!("   {:<keys_width$}", fit(keys, keys_width)),
                    Style::new().fg(theme.mention),
                ),
                Span::styled(fit(what, text_width), Style::new().fg(theme.fg)),
            ]),
        };
        frame.render_widget(Paragraph::new(line), row);
    }
    let more = if lines.len() > rows {
        format!(
            " · {}-{} of {}",
            first + 1,
            (first + rows).min(lines.len()),
            lines.len()
        )
    } else {
        String::new()
    };
    frame.render_widget(
        Paragraph::new(Span::styled(
            format!(" type to narrow · ↑↓ PgUp PgDn scroll · Esc close{more}"),
            Style::new().fg(theme.dim),
        )),
        Rect {
            y: inner.bottom() - 1,
            height: 1,
            ..inner
        },
    );
}

/// The dialog for choosing the room a message is forwarded to.
fn draw_forward(frame: &mut Frame, app: &mut App, area: Rect, forward: Forward) {
    let theme = app.theme;
    // With several accounts, whose each room is.
    let several = app.state.accounts.len() > 1;
    let targets: Vec<(String, bool, String)> = app
        .forward_targets()
        .iter()
        .map(|room| {
            let name = match app.state.accounts.get(room.account).filter(|_| several) {
                Some(account) => format!("{} · {}", room.name, account.label),
                None => room.name.clone(),
            };
            (name, room.is_dm, room.id.clone())
        })
        .collect();
    let height = (targets.len() as u16 + 6)
        .min(area.height.saturating_sub(2))
        .max(8);
    let Some(inner) = dialog(frame, app, area, "Forward to", 56, height) else {
        return;
    };
    let (query, column) = forward
        .filter
        .view((inner.width as usize).saturating_sub(3));
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" / ", Style::new().fg(theme.accent)),
            Span::styled(query, Style::new().fg(theme.fg)),
        ])),
        Rect { height: 1, ..inner },
    );
    frame.set_cursor_position((inner.x + 3 + column, inner.y));

    let rows = inner.height.saturating_sub(4) as usize;
    let first = forward.cursor.saturating_sub(rows.saturating_sub(1));
    for (offset, (index, (name, is_dm, room_id))) in targets
        .iter()
        .enumerate()
        .skip(first)
        .take(rows)
        .enumerate()
    {
        let row = Rect {
            y: inner.y + 2 + offset as u16,
            height: 1,
            ..inner
        };
        let mut style = Style::new().fg(theme.fg);
        if index == forward.cursor {
            style = style.bg(theme.selection_bg).fg(theme.selection_fg);
        }
        let icon = if *is_dm { "@" } else { "#" };
        let text = format!(
            " {icon} {}",
            fit(name, (row.width as usize).saturating_sub(4))
        );
        frame.render_widget(Paragraph::new(Span::styled(text, style)).style(style), row);
        app.hits.push(Hit {
            rect: row,
            action: HitAction::ForwardTo(room_id.clone()),
        });
    }
    if targets.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                " No room by that name",
                Style::new().fg(theme.dim),
            )),
            Rect {
                y: inner.y + 2,
                height: 1,
                ..inner
            },
        );
    }
    frame.render_widget(
        Paragraph::new(Span::styled(
            " type to narrow · ↑↓ choose · Enter forward · Esc cancel",
            Style::new().fg(theme.dim),
        )),
        Rect {
            y: inner.bottom() - 1,
            height: 1,
            ..inner
        },
    );
}

/// The room info dialog: what the room is and who is in it.
fn draw_room_info(frame: &mut Frame, app: &mut App, area: Rect, info: RoomInfo) {
    use crate::app::{InfoAction, InfoConfirm, InfoInput, InfoMode};

    let theme = app.theme;
    let dim = Style::new().fg(theme.dim);
    let members = info.details.as_ref().map_or(0, |d| d.members.len()) as u16;
    let height = (members + 11).min(area.height.saturating_sub(2)).max(12);
    let Some(inner) = dialog(frame, app, area, "Room", 76, height) else {
        return;
    };
    let line_at = |offset: u16| Rect {
        y: inner.y + offset,
        height: 1,
        ..inner
    };
    let Some(details) = &info.details else {
        frame.render_widget(Paragraph::new(Span::styled(" Loading…", dim)), line_at(1));
        return;
    };
    let width = inner.width as usize;

    let lock = if details.encrypted {
        Span::styled("  encrypted", Style::new().fg(theme.success))
    } else {
        Span::styled("  not encrypted", dim)
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!(" {}", fit(&details.name, width.saturating_sub(18))),
                Style::new().fg(theme.fg).add_modifier(Modifier::BOLD),
            ),
            lock,
        ])),
        line_at(0),
    );
    let address = if details.alias.is_empty() {
        &details.room_id
    } else {
        &details.alias
    };
    frame.render_widget(
        Paragraph::new(Span::styled(
            format!(
                " {}",
                crate::app::short_id(address, width.saturating_sub(2))
            ),
            dim,
        )),
        line_at(1),
    );
    let topic = details.topic.lines().next().unwrap_or_default().trim();
    let topic = if topic.is_empty() { "no topic" } else { topic };
    frame.render_widget(
        Paragraph::new(Span::styled(
            format!(" {}", fit(topic, width.saturating_sub(2))),
            Style::new().fg(theme.fg),
        )),
        line_at(2),
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            format!(" People ({})", details.members.len()),
            dim.add_modifier(Modifier::BOLD),
        )),
        line_at(4),
    );

    // The list scrolls to keep the cursor in view.
    let rows = inner.height.saturating_sub(9) as usize;
    let first = info.cursor.saturating_sub(rows.saturating_sub(1));
    for (offset, (index, member)) in details
        .members
        .iter()
        .enumerate()
        .skip(first)
        .take(rows)
        .enumerate()
    {
        let row = line_at(5 + offset as u16);
        let selected = index == info.cursor;
        let mut style = Style::new().fg(theme.fg);
        if selected {
            style = style.bg(theme.selection_bg).fg(theme.selection_fg);
        }
        let tag = match (member.invited, member.role.as_str()) {
            (true, _) => "invited",
            (false, role) => role,
        };
        let name_width = 24.min(width / 3);
        let id_width = width.saturating_sub(name_width + tag.width() + 4);
        let line = Line::from(vec![
            Span::styled(
                format!(" {:<name_width$} ", fit(&member.name, name_width)),
                style,
            ),
            Span::styled(
                format!(
                    "{:<id_width$} ",
                    crate::app::short_id(&member.user_id, id_width)
                ),
                style.fg(if selected {
                    theme.selection_fg
                } else {
                    theme.dim
                }),
            ),
            Span::styled(tag.to_owned(), style.fg(theme.accent)),
        ]);
        frame.render_widget(Paragraph::new(line).style(style), row);
        app.hits.push(Hit {
            rect: row,
            action: HitAction::RoomInfo(InfoAction::Select(index)),
        });
    }

    let note = line_at(inner.height.saturating_sub(3));
    let buttons = line_at(inner.height.saturating_sub(1));
    let who = info
        .selected()
        .map(|member| member.name.clone())
        .unwrap_or_default();
    match &info.mode {
        InfoMode::Input(what, input) => {
            let label = match what {
                InfoInput::Invite => " Invite (@name:server): ",
                InfoInput::Name => " Room name: ",
                InfoInput::Topic => " Topic: ",
            };
            let (shown, column) = input.view(width.saturating_sub(label.width() + 1));
            frame.set_cursor_position((note.x + label.width() as u16 + column, note.y));
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(label, Style::new().fg(theme.accent)),
                    Span::styled(shown, Style::new().fg(theme.fg)),
                ])),
                note,
            );
            draw_buttons(
                frame,
                app,
                buttons,
                &[
                    ("Enter  ok", HitAction::RoomInfo(InfoAction::Confirm)),
                    ("Esc  cancel", HitAction::RoomInfo(InfoAction::Back)),
                ],
            );
        }
        InfoMode::Confirm(what) => {
            let (question, yes) = match what {
                InfoConfirm::Kick => (format!(" Remove {who} from this room?"), "y  remove"),
                InfoConfirm::Ban => (
                    format!(" Ban {who}? They cannot come back until unbanned."),
                    "y  ban",
                ),
                InfoConfirm::Leave => (
                    format!(
                        " Leave {}? You need an invitation to return to a private room.",
                        details.name
                    ),
                    "y  leave",
                ),
            };
            frame.render_widget(
                Paragraph::new(Span::styled(
                    fit(&question, width),
                    Style::new().fg(theme.unread),
                )),
                note,
            );
            draw_buttons(
                frame,
                app,
                buttons,
                &[
                    (yes, HitAction::RoomInfo(InfoAction::Confirm)),
                    ("n  no", HitAction::RoomInfo(InfoAction::Back)),
                ],
            );
        }
        InfoMode::List => {
            let mut row: Vec<(&str, HitAction)> = vec![
                ("m  chat", HitAction::RoomInfo(InfoAction::Message)),
                ("v  verify", HitAction::RoomInfo(InfoAction::Verify)),
            ];
            if details.can_invite {
                row.push(("i  invite", HitAction::RoomInfo(InfoAction::Invite)));
            }
            if details.can_kick {
                row.push(("x  remove", HitAction::RoomInfo(InfoAction::Kick)));
            }
            if details.can_ban {
                row.push(("b  ban", HitAction::RoomInfo(InfoAction::Ban)));
            }
            if details.can_rename {
                row.push(("n  name", HitAction::RoomInfo(InfoAction::Rename)));
            }
            if details.can_set_topic {
                row.push(("t  topic", HitAction::RoomInfo(InfoAction::Topic)));
            }
            row.push(("L  leave", HitAction::RoomInfo(InfoAction::Leave)));
            draw_buttons(frame, app, buttons, &row);
        }
    }
}

/// The dialog for putting a room into one of the user's own sections.
fn draw_sections(frame: &mut Frame, app: &mut App, area: Rect, picker: SectionPicker) {
    let theme = app.theme;
    let rows = app.section_picker_rows();
    let room = app
        .state
        .rooms
        .iter()
        .find(|room| room.id == picker.room_id)
        .map(|room| room.name.clone())
        .unwrap_or_default();
    let title = format!("Section for {}", fit(&room, 30));
    let Some(inner) = dialog(frame, app, area, &title, 60, rows.len() as u16 + 5) else {
        return;
    };
    let dim = Style::new().fg(theme.dim);
    let new_row = app.state.sections.len();

    for (index, (label, current)) in rows.iter().enumerate() {
        let row = Rect {
            y: inner.y + 1 + index as u16,
            height: 1,
            ..inner
        };
        if row.y >= inner.bottom() {
            break;
        }
        let mut style = Style::new().fg(theme.fg);
        if index == picker.cursor {
            style = style.bg(theme.selection_bg).fg(theme.selection_fg);
        }
        let line = match picker.naming.as_ref().filter(|_| index == new_row) {
            Some(input) => {
                let (shown, column) = input.view((row.width as usize).saturating_sub(9));
                frame.set_cursor_position((row.x + 9 + column, row.y));
                Line::from(vec![
                    Span::styled("   Name: ", Style::new().fg(theme.accent)),
                    Span::styled(shown, Style::new().fg(theme.fg)),
                ])
            }
            None => {
                let mark = if *current { " ✓ " } else { "   " };
                Line::from(Span::styled(format!("{mark}{label}"), style))
            }
        };
        let row_style = if picker.naming.is_some() && index == new_row {
            Style::new()
        } else {
            style
        };
        frame.render_widget(Paragraph::new(line).style(row_style), row);
        if let Some(target) = app.section_picker_row(index) {
            app.hits.push(Hit {
                rect: row,
                action: HitAction::SectionPick(target),
            });
        }
    }

    let help = Rect {
        y: inner.bottom() - 1,
        height: 1,
        ..inner
    };
    let text = if picker.naming.is_some() {
        " name, or Parent/Name for a subsection · Enter · Esc"
    } else {
        " ↑↓ choose · Enter or click · Esc close"
    };
    frame.render_widget(Paragraph::new(Span::styled(text, dim)), help);
}

/// The menu a right-click on a room opens, next to where the click was.
fn draw_room_menu(frame: &mut Frame, app: &mut App, area: Rect, menu: RoomMenu) {
    let rows = app.room_menu_rows();
    if rows.is_empty() {
        return;
    }
    let theme = app.theme;
    let width = rows
        .iter()
        .map(|(_, label)| label.width())
        .max()
        .unwrap_or(0) as u16
        + 4;
    let height = rows.len() as u16 + 2;
    if area.width < width || area.height < height {
        return;
    }
    let rect = Rect {
        // Just below the click, so the room stays in view.
        x: menu.x.min(area.right() - width),
        y: (menu.y + 1).min(area.bottom() - height),
        width,
        height,
    };
    app.hits.push(Hit {
        rect,
        action: HitAction::Blocked,
    });
    frame.render_widget(Clear, rect);
    let block = panel(&theme, true);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    for (index, (item, label)) in rows.iter().enumerate() {
        let row = Rect {
            y: inner.y + index as u16,
            height: 1,
            ..inner
        };
        let mut style = Style::new().fg(theme.fg);
        if index == menu.cursor {
            style = style.bg(theme.selection_bg).fg(theme.selection_fg);
        }
        frame.render_widget(Paragraph::new(format!(" {label}")).style(style), row);
        app.hits.push(Hit {
            rect: row,
            action: HitAction::RoomMenuPick(*item),
        });
    }
}

/// The dialog for renaming, reordering and removing sections.
fn draw_section_manager(frame: &mut Frame, app: &mut App, area: Rect, manager: SectionManager) {
    let theme = app.theme;
    let sections = app.state.sections.clone();
    let Some(inner) = dialog(frame, app, area, "Sections", 66, sections.len() as u16 + 5) else {
        return;
    };
    let dim = Style::new().fg(theme.dim);
    type MakeEdit = fn(usize) -> SectionEdit;
    let buttons: [(&str, MakeEdit); 4] = [
        (" ↑ ", SectionEdit::Up),
        (" ↓ ", SectionEdit::Down),
        (" rename ", SectionEdit::Rename),
        (" delete ", SectionEdit::Delete),
    ];
    let buttons_width: usize = buttons.iter().map(|(label, _)| label.width()).sum();

    for (index, section) in sections.iter().enumerate() {
        let row = Rect {
            y: inner.y + 1 + index as u16,
            height: 1,
            ..inner
        };
        if row.y >= inner.bottom() {
            break;
        }
        if let Some(input) = manager
            .renaming
            .as_ref()
            .filter(|_| index == manager.cursor)
        {
            let (shown, column) = input.view((row.width as usize).saturating_sub(7));
            frame.set_cursor_position((row.x + 7 + column, row.y));
            let line = Line::from(vec![
                Span::styled(" Name: ", Style::new().fg(theme.accent)),
                Span::styled(shown, Style::new().fg(theme.fg)),
            ]);
            frame.render_widget(Paragraph::new(line), row);
            continue;
        }
        let mut style = Style::new().fg(theme.fg);
        if index == manager.cursor {
            style = style.bg(theme.selection_bg).fg(theme.selection_fg);
        }
        let count = format!("{} ", section.rooms.len());
        let name_width = (row.width as usize).saturating_sub(buttons_width + count.width() + 1);
        let label = match crate::sections::parent_name(&section.name) {
            Some(_) => format!("  └ {}", crate::sections::label(&section.name)),
            None => section.name.clone(),
        };
        let name = fit(&label, name_width);
        let mut spans = vec![
            Span::styled(format!(" {name:<name_width$}"), style),
            Span::styled(count, style.fg(theme.dim)),
        ];
        app.hits.push(Hit {
            rect: row,
            action: HitAction::SectionEdit(SectionEdit::Select(index)),
        });
        let mut x = row.right().saturating_sub(buttons_width as u16);
        for (label, edit) in buttons {
            spans.push(Span::styled(label, style.fg(theme.accent)));
            let width = label.width() as u16;
            app.hits.push(Hit {
                rect: Rect { x, width, ..row },
                action: HitAction::SectionEdit(edit(index)),
            });
            x += width;
        }
        frame.render_widget(Paragraph::new(Line::from(spans)).style(style), row);
    }

    let help = Rect {
        y: inner.bottom() - 1,
        height: 1,
        ..inner
    };
    let text = if manager.renaming.is_some() {
        " new name; Parent/Name nests it · Enter save · Esc cancel"
    } else {
        " ↑↓ choose · J K move · r rename · d delete · Esc close"
    };
    frame.render_widget(Paragraph::new(Span::styled(text, dim)), help);
}

/// The emoji picker: a search line, the grid of emoji with their groups'
/// titles, the name of the one under the cursor.
fn draw_emoji_picker(frame: &mut Frame, app: &mut App, area: Rect, picker: EmojiPicker) {
    let theme = app.theme;
    let dim = Style::new().fg(theme.dim);
    let width = (area.width * 3 / 4).clamp(36, 4 + 16 * CELL_WIDTH);
    let height = (area.height * 4 / 5).max(12);
    let title = match picker.target {
        PickFor::React(_) => "React with",
        PickFor::Insert => "Emoji",
    };
    let Some(inner) = dialog(frame, app, area, title, width, height) else {
        return;
    };

    // Search line.
    let (query, column) = picker.search.view((inner.width as usize).saturating_sub(3));
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" / ", Style::new().fg(theme.accent)),
            Span::styled(query, Style::new().fg(theme.fg)),
        ])),
        Rect { height: 1, ..inner },
    );
    frame.set_cursor_position((inner.x + 3 + column, inner.y));

    // The grid is laid out for the width it has; the picker keeps that for
    // its keys.
    let list = Rect {
        y: inner.y + 2,
        height: inner.height.saturating_sub(4),
        ..inner
    };
    let columns = (list.width / CELL_WIDTH).max(1) as usize;
    let page = (list.height as usize).max(1);
    let resized = picker.columns != columns || picker.page != page;
    if let Some(state) = app.overlays.get_mut::<EmojiPicker>() {
        state.columns = columns;
        state.page = page;
    }
    let grid = app.emoji_grid();
    let Some(state) = app.overlays.get_mut::<EmojiPicker>() else {
        return;
    };
    if resized {
        crate::app::emoji_picker_reveal(state, &grid);
    }
    state.scroll = state.scroll.min(grid.rows.len().saturating_sub(page));
    let (cursor, scroll) = (state.cursor, state.scroll);

    if grid.emoji.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(" No emoji by that name", dim)),
            Rect { height: 1, ..list },
        );
    }
    for (offset, row) in grid.rows.iter().skip(scroll).take(page).enumerate() {
        let y = list.y + offset as u16;
        match row {
            GridRow::Title(title) => frame.render_widget(
                Paragraph::new(Span::styled(
                    format!(" {title}"),
                    Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
                )),
                Rect {
                    y,
                    height: 1,
                    ..list
                },
            ),
            GridRow::Emoji { start, end } => {
                for (column, index) in (*start..*end).enumerate() {
                    let cell = Rect {
                        x: list.x + column as u16 * CELL_WIDTH,
                        y,
                        width: CELL_WIDTH,
                        height: 1,
                    };
                    let style = if index == cursor {
                        Style::new().bg(theme.selection_bg).fg(theme.selection_fg)
                    } else {
                        Style::new().fg(theme.fg)
                    };
                    frame.render_widget(
                        Paragraph::new(format!(" {}", grid.emoji[index].0)).style(style),
                        cell,
                    );
                    app.hits.push(Hit {
                        rect: cell,
                        action: HitAction::EmojiPick(grid.emoji[index].0.clone()),
                    });
                }
            }
        }
    }

    // What the emoji under the cursor is called, and the keys.
    let name_row = Rect {
        y: inner.bottom().saturating_sub(2),
        height: 1,
        ..inner
    };
    if let Some((emoji, _)) = grid.emoji.get(cursor) {
        let name = emojis::get(emoji)
            .map(|found| {
                let codes: Vec<String> =
                    found.shortcodes().map(|code| format!(":{code}:")).collect();
                format!("{}  {}", found.name(), codes.join(" "))
            })
            .unwrap_or_default();
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw(format!(" {emoji}  ")),
                Span::styled(
                    fit(&name, (name_row.width as usize).saturating_sub(6)),
                    Style::new().fg(theme.fg),
                ),
            ])),
            name_row,
        );
    }
    let help = Rect {
        y: inner.bottom().saturating_sub(1),
        height: 1,
        ..inner
    };
    frame.render_widget(
        Paragraph::new(Span::styled(
            fit(
                " Type to search · arrows move · Enter picks · Esc closes",
                help.width as usize,
            ),
            dim,
        )),
        help,
    );
}

fn draw_search(frame: &mut Frame, app: &mut App, area: Rect, search: Search) {
    let theme = app.theme;
    let width = (area.width * 4 / 5).max(50);
    let height = (area.height * 3 / 4).max(12);
    let Some(inner) = dialog(frame, app, area, "Search", width, height) else {
        return;
    };
    let dim = Style::new().fg(theme.dim);

    // Query line.
    let query_row = Rect { height: 1, ..inner };
    let (query, column) = search
        .input
        .view((query_row.width as usize).saturating_sub(3));
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" / ", Style::new().fg(theme.accent)),
            Span::styled(query, Style::new().fg(theme.fg)),
        ])),
        query_row,
    );
    frame.set_cursor_position((query_row.x + 3 + column, query_row.y));

    // Scope switch.
    let scope_row = Rect {
        y: inner.y + 1,
        height: 1,
        ..inner
    };
    let chosen = Style::new()
        .fg(theme.selection_fg)
        .bg(theme.selection_bg)
        .add_modifier(Modifier::BOLD);
    let labels = [(" This chat ", false), (" All chats ", true)];
    let mut spans = vec![Span::raw(" ")];
    let mut x = scope_row.x + 1;
    for (label, all_chats) in labels {
        let style = if search.all_chats == all_chats {
            chosen
        } else {
            dim
        };
        spans.push(Span::styled(label, style));
        spans.push(Span::raw(" "));
        let label_width = label.width() as u16;
        app.hits.push(Hit {
            rect: Rect {
                x,
                width: label_width,
                ..scope_row
            },
            action: HitAction::SearchScope(all_chats),
        });
        x += label_width + 1;
    }
    spans.push(Span::styled(" Tab switches", dim));
    frame.render_widget(Paragraph::new(Line::from(spans)), scope_row);

    // Results.
    let list = Rect {
        y: inner.y + 3,
        height: inner.height.saturating_sub(4),
        ..inner
    };
    let status = if search.waiting {
        "Searching…".to_owned()
    } else if search.searched.is_none() {
        "Type what you are looking for and press Enter.".to_owned()
    } else if search.hits.is_empty() {
        "Nothing found. Only messages this device has loaded, and unencrypted history on the server, can be searched.".to_owned()
    } else {
        String::new()
    };
    if !status.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(format!(" {status}"), dim))
                .wrap(ratatui::widgets::Wrap { trim: true }),
            list,
        );
    } else {
        let visible = list.height as usize;
        let first = (search.selected + 1).saturating_sub(visible);
        let terms = crate::search::terms(search.input.text());
        for (offset, hit) in search.hits.iter().enumerate().skip(first).take(visible) {
            let row = Rect {
                y: list.y + (offset - first) as u16,
                height: 1,
                ..list
            };
            let mut style = Style::new().fg(theme.fg);
            if offset == search.selected {
                style = style.bg(theme.selection_bg).fg(theme.selection_fg);
            }
            let when = short_date(hit.ts_ms);
            let mut head = format!(" {when}  ");
            if search.all_chats {
                head.push_str(&fit(&hit.room_name, 18));
                head.push_str(" · ");
            }
            head.push_str(&fit(&hit.sender, 16));
            head.push_str(": ");
            let room = (row.width as usize).saturating_sub(head.width() + 1);
            let text = crate::search::snippet(&hit.text, &terms, room);
            let line = Line::from(vec![
                Span::styled(
                    head,
                    style.fg(if offset == search.selected {
                        theme.selection_fg
                    } else {
                        theme.dim
                    }),
                ),
                Span::styled(fit(&text, room), style),
            ]);
            frame.render_widget(Paragraph::new(line).style(style), row);
            app.hits.push(Hit {
                rect: row,
                action: HitAction::SearchHit {
                    room_id: hit.room_id.clone(),
                    event_id: hit.event_id.clone(),
                },
            });
        }
    }

    let help = Rect {
        y: inner.bottom() - 1,
        height: 1,
        ..inner
    };
    let count = if search.hits.is_empty() {
        String::new()
    } else {
        format!("{} found · ", search.hits.len())
    };
    frame.render_widget(
        Paragraph::new(Span::styled(
            format!(" {count}Enter search / open · ↑↓ choose · Esc close"),
            dim,
        )),
        help,
    );
}

fn short_date(ts_ms: u64) -> String {
    Local
        .timestamp_millis_opt(ts_ms as i64)
        .single()
        .map(|t| t.format("%d %b %H:%M").to_string())
        .unwrap_or_default()
}

/// Render the UI into plain text (no colours), for tests and `--snapshot`.
pub fn render_to_text(app: &mut App, width: u16, height: u16) -> String {
    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = ratatui::Terminal::new(backend).expect("test backend cannot fail");
    terminal
        .draw(|frame| draw(frame, app))
        .expect("test backend cannot fail");
    let buffer = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..buffer.area.height {
        let mut row = String::new();
        let mut x = 0;
        while x < buffer.area.width {
            let symbol = buffer[(x, y)].symbol();
            row.push_str(symbol);
            // A double-width character also owns the next cell.
            x += symbol.width().max(1) as u16;
        }
        out.push_str(row.trim_end());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    use super::*;
    use crate::{
        matrix::{Command, Update},
        model::{PollAnswer, Preview, Reaction, RoomEntry, VerifyState},
        theme::THEMES,
        verify::VerifyInput,
    };

    fn sample() -> App {
        let mut app = App::new(THEMES[0], "@me:example.org".into());
        app.on_update(Update::Rooms(vec![
            RoomEntry {
                id: "!lunch".into(),
                name: "Lunch Club".into(),
                topic: "Where we eat".into(),
                unread: 3,
                ..RoomEntry::default()
            },
            RoomEntry {
                id: "!dev".into(),
                name: "Dev Chat".into(),
                ..RoomEntry::default()
            },
            RoomEntry {
                id: "!amy".into(),
                name: "Amy Winter".into(),
                is_dm: true,
                mentions: 1,
                unread: 1,
                ..RoomEntry::default()
            },
        ]));
        app.state.accounts[0].sync = SyncStatus::Online;
        app
    }

    #[test]
    fn the_start_screen_shows_the_moth_in_one_piece() {
        for row in MOTH {
            assert_eq!(row.chars().count(), MOTH_WIDTH, "{row:?}");
            // Mirrored: what leans left on one side leans right on the other.
            let mirrored: String = row
                .chars()
                .rev()
                .map(|c| match c {
                    '/' => '\\',
                    '\\' => '/',
                    '(' => ')',
                    ')' => '(',
                    '`' => '\'',
                    '\'' => '`',
                    other => other,
                })
                .collect();
            if !row.contains(" ) ") {
                assert_eq!(mirrored, row);
            }
        }
        // With pictures off, the text moth stands in for the logo.
        let mut app = sample();
        app.graphics = crate::app::Graphics::Off;
        let screen = render_to_text(&mut app, 100, 30);
        assert!(screen.contains(r"( ◆ )   \_ |=| _/   ( ◆ )"), "{screen}");
        assert!(screen.contains("m o t h r i x"), "{screen}");
    }

    #[test]
    fn the_start_screen_shows_the_logo_as_a_picture() {
        let mut app = sample();
        assert_eq!(app.graphics, crate::app::Graphics::Blocks);
        let screen = render_to_text(&mut app, 100, 40);
        assert!(screen.contains('▀'), "{screen}");
        assert!(!screen.contains("|=|"), "no text moth as well: {screen}");
        assert!(screen.contains("m o t h r i x"), "{screen}");
        // Too low for the picture: the text moth if it fits, or none.
        let screen = render_to_text(&mut app, 100, 16);
        assert!(!screen.contains('▀'), "{screen}");
        assert!(screen.contains("m o t h r i x"), "{screen}");
    }

    #[test]
    fn a_link_preview_shows_under_the_message() {
        let mut app = sample();
        app.state.open_room = Some("!lunch".into());
        app.state.link_previews.insert(
            "https://food.example/menu".into(),
            ("This week's menu".into(), "Soup, noodles, cake".into()),
        );
        app.on_update(Update::Timeline {
            room_id: "!lunch".into(),
            items: vec![message("m1", "Amy", "see https://food.example/menu", 1_000)],
        });
        let screen = render_to_text(&mut app, 100, 24);
        assert!(screen.contains("▎ This week's menu"), "{screen}");
        assert!(screen.contains("▎ Soup, noodles, cake"), "{screen}");
        let commands = click(&mut app, "This week's menu", &screen);
        assert!(
            matches!(&commands[..], [Command::OpenLink(_)]),
            "{commands:?}"
        );
    }

    #[test]
    fn links_in_messages_open_on_a_click() {
        let mut app = sample();
        app.state.open_room = Some("!lunch".into());
        app.on_update(Update::Timeline {
            room_id: "!lunch".into(),
            items: vec![message(
                "m1",
                "Amy",
                "menu: [this week](https://food.example/menu) or https://other.example/x",
                1_000,
            )],
        });
        let screen = render_to_text(&mut app, 110, 24);
        assert!(
            screen.contains("menu: this week or https://other.example/x"),
            "{screen}"
        );
        let commands = click(&mut app, "this week", &screen);
        assert!(
            matches!(&commands[..], [Command::OpenLink(a)] if a == "https://food.example/menu"),
            "{commands:?}"
        );
        let commands = click(&mut app, "https://other", &screen);
        assert!(
            matches!(&commands[..], [Command::OpenLink(a)] if a == "https://other.example/x"),
            "{commands:?}"
        );
    }

    #[test]
    fn a_thread_shows_under_its_first_message_and_a_click_opens_it() {
        let mut app = sample();
        app.state.open_room = Some("!lunch".into());
        let mut root = message("root", "Amy", "Where do we eat?", 1_000);
        root.thread = Some((2, Some(("Ben".into(), "The noodle place".into()))));
        app.on_update(Update::Timeline {
            room_id: "!lunch".into(),
            items: vec![root],
        });
        let screen = render_to_text(&mut app, 100, 24);
        assert!(
            screen.contains("↳ 2 replies · Ben: The noodle place"),
            "{screen}"
        );

        let commands = click(&mut app, "↳ 2 replies", &screen);
        assert!(
            matches!(&commands[..], [Command::OpenThread { event_id }] if event_id == "$root"),
            "{commands:?}"
        );
        // The room's conversation no longer belongs on screen; the thread does.
        app.on_update(Update::Timeline {
            room_id: "!lunch".into(),
            items: vec![message("other", "Amy", "not in the thread", 2_000)],
        });
        app.on_update(Update::Thread {
            room_id: "!lunch".into(),
            root: "$root".into(),
            items: vec![
                message("root", "Amy", "Where do we eat?", 1_000),
                message("r1", "Ben", "The noodle place", 1_500),
            ],
        });
        let screen = render_to_text(&mut app, 100, 24);
        assert!(screen.contains("← Thread in Lunch Club"), "{screen}");
        assert!(screen.contains("Reply in thread"), "{screen}");
        assert!(screen.contains("The noodle place"), "{screen}");
        assert!(!screen.contains("not in the thread"), "{screen}");

        // The title leads back.
        let commands = click(&mut app, "← Thread in", &screen);
        assert!(
            matches!(&commands[..], [Command::CloseThread]),
            "{commands:?}"
        );
        assert_eq!(app.state.open_thread, None);
    }

    #[test]
    fn several_accounts_show_tabs_and_a_line_per_account() {
        let mut app = sample();
        let mut work = app.state.accounts[0].clone();
        work.profile = "work".into();
        work.label = "work".into();
        work.sync = SyncStatus::Connecting;
        app.state.accounts.push(work);
        let mut rooms = app.state.rooms.clone();
        rooms.push(RoomEntry {
            id: "work/!standup".into(),
            name: "Standup".into(),
            account: 1,
            unread: 4,
            ..RoomEntry::default()
        });
        app.on_update(Update::Rooms(rooms));

        let screen = render_to_text(&mut app, 100, 24);
        assert!(screen.contains(" all  default  work "), "{screen}");
        assert!(screen.contains("▾ default ─"), "{screen}");
        assert!(screen.contains("▾ work ○ ─"), "{screen}");
        assert!(screen.contains("Standup"), "{screen}");

        // Clicking the "work" tab leaves only its rooms.
        let tab = app
            .hits
            .iter()
            .find(|hit| hit.action == HitAction::AccountTab(Some(1)))
            .expect("a tab for the second account")
            .rect;
        app.on_event(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: tab.x + 1,
            row: tab.y,
            modifiers: KeyModifiers::NONE,
        }));
        let screen = render_to_text(&mut app, 100, 24);
        assert!(screen.contains("Standup"), "{screen}");
        assert!(!screen.contains("Lunch Club"), "{screen}");
        assert!(!screen.contains("▾ work"), "{screen}");
    }

    #[test]
    fn the_unread_button_narrows_the_list_to_unread_chats() {
        let mut app = sample();
        let screen = render_to_text(&mut app, 100, 24);
        assert!(screen.contains("○ unread"), "{screen}");
        assert!(screen.contains("Dev Chat"), "{screen}");
        click(&mut app, "○ unread", &screen);
        let screen = render_to_text(&mut app, 100, 24);
        assert!(screen.contains("● unread"), "{screen}");
        assert!(screen.contains("▾ UNREAD"), "{screen}");
        // Lunch Club (3 unread) and Amy (a mention) stay; Dev Chat goes.
        assert!(
            screen.contains("Lunch Club") && screen.contains("Amy Winter"),
            "{screen}"
        );
        assert!(!screen.contains("Dev Chat"), "{screen}");
    }

    #[test]
    fn clicking_a_section_header_folds_it_for_its_account_only() {
        let mut app = sample();
        let mut work = app.state.accounts[0].clone();
        work.profile = "work".into();
        work.label = "work".into();
        app.state.accounts.push(work);
        let mut rooms = app.state.rooms.clone();
        rooms.push(RoomEntry {
            id: "work/!standup".into(),
            name: "Standup".into(),
            account: 1,
            ..RoomEntry::default()
        });
        app.on_update(Update::Rooms(rooms));
        let screen = render_to_text(&mut app, 100, 24);
        // The second "ROOMS" header on screen is the one under "work".
        let header = app
            .hits
            .iter()
            .filter(|hit| matches!(&hit.action, HitAction::ToggleSection(key) if key.ends_with("ROOMS")))
            .map(|hit| hit.rect)
            .next_back()
            .expect("a ROOMS header");
        app.on_event(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: header.x + 2,
            row: header.y,
            modifiers: KeyModifiers::NONE,
        }));
        let after = render_to_text(&mut app, 100, 24);
        assert!(
            screen.contains("Standup") && !after.contains("Standup"),
            "{after}"
        );
        assert!(after.contains("Lunch Club"), "{after}");
    }

    fn message(key: &str, sender: &str, body: &str, ts_ms: u64) -> Item {
        Item {
            key: key.into(),
            event_id: format!("${key}"),
            sender_id: format!("@{}:example.org", sender.to_lowercase()),
            sender_name: sender.into(),
            body: body.into(),
            ts_ms,
            ..Item::default()
        }
    }

    fn poll_item() -> Item {
        let answer = |id: &str, text: &str, voters: &[&str], mine: bool| PollAnswer {
            id: id.into(),
            text: text.into(),
            voters: voters.iter().map(|v| (*v).to_owned()).collect(),
            self_voted: mine,
            is_winner: false,
        };
        Item {
            kind: ItemKind::Poll(Poll {
                question: "Where should we go for lunch?".into(),
                max_selections: 1,
                is_ended: false,
                results_visible: true,
                voter_count: 4,
                answers: vec![
                    answer("a", "Pizza", &["Amy Winter", "Ben Okafor", "Dan"], false),
                    answer("b", "Sushi", &["me"], true),
                ],
                format: Default::default(),
            }),
            ..message("poll", "Amy", "Where should we go for lunch?", 2_000)
        }
    }

    fn open_lunch(app: &mut App) {
        app.state.open_room = Some("!lunch".into());
        let mut mine = message("4", "Me", "my own message", 800_000);
        mine.is_own = true;
        mine.can_edit = true;
        mine.can_delete = true;
        mine.reactions = vec![Reaction {
            key: "👍".into(),
            count: 2,
            mine: false,
            ..Default::default()
        }];
        app.on_update(Update::Timeline {
            room_id: "!lunch".into(),
            items: vec![
                message("1", "Amy", "Hi all, quick poll about lunch", 1_000),
                message("2", "Amy", "second message, same group", 1_500),
                poll_item(),
                message("3", "Ben", "Pizza it is", 400_000),
                mine,
            ],
        });
    }

    #[test]
    fn sidebar_lists_rooms_and_people_with_unread_badges() {
        let mut app = sample();
        let screen = render_to_text(&mut app, 100, 24);
        assert!(screen.contains("ROOMS"), "{screen}");
        assert!(screen.contains("PEOPLE"), "{screen}");
        let lunch = screen
            .lines()
            .find(|line| line.contains("# Lunch Club"))
            .expect("lunch row");
        assert!(lunch.contains(" 3 "), "{lunch}");
        let amy = screen
            .lines()
            .find(|line| line.contains("@ Amy Winter"))
            .expect("amy row");
        assert!(amy.contains("@1"), "{amy}");
        assert!(screen.contains("Pick a room on the left"), "{screen}");
        assert!(screen.contains("online"), "{screen}");
        assert!(screen.contains("sort: recent"), "{screen}");
    }

    #[test]
    fn pinned_rooms_show_in_their_own_section() {
        let mut app = sample();
        app.state.rooms[2].favourite = true;
        let screen = render_to_text(&mut app, 100, 24);
        let pinned = screen
            .lines()
            .position(|line| line.contains("PINNED"))
            .expect("pinned");
        let amy = screen
            .lines()
            .position(|line| line.contains("@ Amy Winter"))
            .expect("amy");
        let rooms = screen
            .lines()
            .position(|line| line.contains("ROOMS"))
            .expect("rooms");
        assert!(pinned < amy && amy < rooms, "{screen}");
        assert!(!screen.contains("PEOPLE"), "{screen}");
    }

    #[test]
    fn timeline_groups_messages_and_shows_who_voted_for_what() {
        let mut app = sample();
        open_lunch(&mut app);
        let screen = render_to_text(&mut app, 110, 34);
        assert!(screen.contains("Lunch Club"), "{screen}");
        assert!(screen.contains("Where we eat"), "{screen}");
        // One header for Amy's two consecutive messages and her poll.
        assert_eq!(screen.matches("Amy  ").count(), 1, "{screen}");
        assert!(screen.contains("second message, same group"), "{screen}");
        assert!(
            screen.contains("▍ Where should we go for lunch?"),
            "{screen}"
        );
        let pizza = screen
            .lines()
            .find(|line| line.contains("○ Pizza"))
            .expect("pizza row");
        assert!(pizza.contains("3 · 75%"), "{pizza}");
        assert!(screen.contains("Amy Winter, Ben Okafor, Dan"), "{screen}");
        let sushi = screen
            .lines()
            .find(|line| line.contains("● Sushi"))
            .expect("sushi row");
        assert!(sushi.contains("1 · 25%"), "{sushi}");
        assert!(screen.contains("Votes: 4"), "{screen}");
        // Not our poll, so no button to end it.
        assert!(!screen.contains("End poll"), "{screen}");
    }

    fn click_at(app: &mut App, needle: &str, screen: &str, button: MouseButton) -> Vec<Command> {
        let (row, line) = screen
            .lines()
            .enumerate()
            .find(|(_, line)| line.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} not on screen:\n{screen}"));
        // Byte offset → cell column, counting double-width characters.
        let column = line[..line.find(needle).expect("needle")].width();
        app.on_event(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(button),
            column: column as u16,
            row: row as u16,
            modifiers: KeyModifiers::NONE,
        }))
    }

    fn click(app: &mut App, needle: &str, screen: &str) -> Vec<Command> {
        click_at(app, needle, screen, MouseButton::Left)
    }

    #[test]
    fn clicking_a_room_in_the_sidebar_opens_it() {
        let mut app = sample();
        let screen = render_to_text(&mut app, 100, 24);
        let commands = click(&mut app, "Dev Chat", &screen);
        assert!(
            matches!(&commands[..], [Command::Open(id)] if id == "!dev"),
            "{commands:?}"
        );
        assert_eq!(app.focus, Focus::Composer);
    }

    #[test]
    fn a_forward_target_clicked_after_the_room_list_moved_is_the_one_drawn() {
        let mut app = sample();
        open_lunch(&mut app);
        app.overlays.open(crate::app::Forward {
            event_id: "4".into(),
            filter: Default::default(),
            cursor: 0,
        });
        render_to_text(&mut app, 100, 24);
        let rect = app
            .hits
            .iter()
            .find(|hit| matches!(&hit.action, HitAction::ForwardTo(room) if room == "!dev"))
            .map(|hit| hit.rect)
            .expect("Dev Chat drawn");
        // A new message reorders the room list before the click lands.
        let mut rooms = app.state.rooms.clone();
        rooms.rotate_left(1);
        app.on_update(Update::Rooms(rooms));
        let commands = app.on_event(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x + 2,
            row: rect.y,
            modifiers: KeyModifiers::NONE,
        }));
        assert!(
            matches!(&commands[..], [Command::Forward { room_id, .. }] if room_id == "!dev"),
            "{commands:?}"
        );
    }

    #[test]
    fn the_settings_say_which_mothrix_this_is() {
        let mut app = sample();
        app.overlays.open(SettingsDialog::default());
        let screen = render_to_text(&mut app, 100, 40);
        assert!(
            screen.contains(&format!("mothrix {}", crate::VERSION)),
            "{screen}"
        );
        assert!(screen.contains("github.com/r1sq/mothrix"), "{screen}");
        assert!(crate::VERSION.starts_with(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn right_click_menu_pins_a_room_and_clicking_sort_cycles() {
        let mut app = sample();
        let screen = render_to_text(&mut app, 100, 24);
        assert!(click_at(&mut app, "Dev Chat", &screen, MouseButton::Right).is_empty());
        // The menu opens where the click was; its entries are clickable.
        let screen = render_to_text(&mut app, 100, 24);
        assert!(screen.contains("Move to section…"), "{screen}");
        let commands = click(&mut app, "Pin to top", &screen);
        assert!(
            matches!(&commands[..], [Command::SetFavourite { room_id, value: true }] if room_id == "!dev"),
            "{commands:?}"
        );
        let screen = render_to_text(&mut app, 100, 24);
        assert!(!screen.contains("Move to section…"), "{screen}");
        click(&mut app, "sort: recent", &screen);
        let screen = render_to_text(&mut app, 100, 24);
        assert!(screen.contains("sort: name"), "{screen}");
        assert!(screen.contains("PINNED"), "{screen}");

        // A click beside the menu just closes it.
        click_at(&mut app, "Dev Chat", &screen, MouseButton::Right);
        let screen = render_to_text(&mut app, 100, 24);
        assert!(click(&mut app, "online", &screen).is_empty());
        assert!(!app.overlays.is_open::<RoomMenu>());
    }

    #[test]
    fn clicking_a_poll_answer_votes_for_it() {
        let mut app = sample();
        open_lunch(&mut app);
        let screen = render_to_text(&mut app, 110, 34);
        let commands = click(&mut app, "Pizza", &screen);
        assert!(
            matches!(&commands[..], [Command::Vote { poll_event_id, answers, .. }] if poll_event_id == "$poll" && answers == &["a".to_owned()]),
            "{commands:?}"
        );
        assert!(app.selected.is_none(), "voting must not select the poll");
    }

    #[test]
    fn clicking_a_message_shows_its_actions_and_the_buttons_work() {
        let mut app = sample();
        open_lunch(&mut app);
        let screen = render_to_text(&mut app, 110, 34);
        assert!(!screen.contains("r Reply"), "{screen}");

        // Someone else's message: reply and react only.
        assert!(click(&mut app, "Pizza it is", &screen).is_empty());
        assert_eq!(app.selected.as_deref(), Some("$3"));
        let screen = render_to_text(&mut app, 110, 34);
        assert!(
            screen.contains("r Reply") && screen.contains("a React"),
            "{screen}"
        );
        assert!(
            !screen.contains("e Edit") && !screen.contains("d Delete"),
            "{screen}"
        );

        // Our own message: everything, and Delete asks first.
        click(&mut app, "my own message", &screen);
        let screen = render_to_text(&mut app, 110, 34);
        assert!(
            screen.contains("e Edit") && screen.contains("d Delete"),
            "{screen}"
        );
        assert!(click(&mut app, "d Delete", &screen).is_empty());
        let screen = render_to_text(&mut app, 110, 34);
        assert!(screen.contains("Delete this message?"), "{screen}");
        let commands = click(&mut app, "Enter Delete", &screen);
        assert!(matches!(&commands[..], [Command::Delete { event_id }] if event_id == "$4"));
    }

    #[test]
    fn reply_button_switches_the_composer_into_reply_mode() {
        let mut app = sample();
        open_lunch(&mut app);
        let screen = render_to_text(&mut app, 110, 34);
        click(&mut app, "Pizza it is", &screen);
        let screen = render_to_text(&mut app, 110, 34);
        click(&mut app, "r Reply", &screen);
        assert_eq!(app.focus, Focus::Composer);
        let screen = render_to_text(&mut app, 110, 34);
        assert!(screen.contains("Reply to Ben: Pizza it is"), "{screen}");
        // Clicking the title cancels the reply.
        click(&mut app, "Reply to Ben", &screen);
        assert_eq!(app.compose, Compose::Message);
    }

    #[test]
    fn emoji_row_and_existing_reactions_are_clickable() {
        let mut app = sample();
        open_lunch(&mut app);
        let screen = render_to_text(&mut app, 110, 34);
        // Clicking an existing reaction joins it.
        let commands = click(&mut app, "👍 2", &screen);
        assert!(
            matches!(&commands[..], [Command::React { event_id, key }] if event_id == "$4" && key == "👍"),
            "{commands:?}"
        );

        click(&mut app, "Pizza it is", &screen);
        let screen = render_to_text(&mut app, 110, 34);
        click(&mut app, "a React", &screen);
        let screen = render_to_text(&mut app, 110, 34);
        let commands = click(&mut app, "3 😂", &screen);
        assert!(
            matches!(&commands[..], [Command::React { event_id, key }] if event_id == "$3" && key == "😂"),
            "{commands:?}"
        );
    }

    #[test]
    fn a_selected_message_names_who_reacted_and_more_shows_all() {
        let mut app = sample();
        open_lunch(&mut app);
        let mut item = message("5", "Amy", "who is in?", 900_000);
        item.reactions = vec![
            Reaction {
                key: "👍".into(),
                count: 5,
                mine: true,
                senders: vec!["Amy".into(), "Ben".into(), "Cleo".into(), "Dan".into()],
            },
            Reaction {
                key: "❤️".into(),
                count: 1,
                mine: false,
                senders: vec!["Eve".into()],
            },
        ];
        item.read_by = vec!["Ben".into(), "Cleo".into(), "Dan".into(), "Eve".into()];
        let mut items = app.state.items.clone();
        items.push(item);
        app.on_update(Update::Timeline {
            room_id: "!lunch".into(),
            items,
        });

        // Counts until the message is selected; "read by" is cut short.
        let screen = render_to_text(&mut app, 110, 34);
        assert!(screen.contains("👍 5"), "{screen}");
        assert!(screen.contains("✓ read by Ben, Cleo, Dan +1"), "{screen}");
        click(&mut app, "who is in?", &screen);
        let screen = render_to_text(&mut app, 110, 34);
        assert!(screen.contains("👍 you, Amy, Ben +2"), "{screen}");
        assert!(screen.contains("❤️ Eve"), "{screen}");

        // A click on "+2" shows everyone, each reaction on its own line.
        let commands = click(&mut app, "+2", &screen);
        assert!(commands.is_empty(), "{commands:?}");
        let screen = render_to_text(&mut app, 110, 34);
        assert!(screen.contains("👍 you, Amy, Ben, Cleo, Dan"), "{screen}");
        assert!(screen.contains("✓ read by Ben, Cleo, Dan, Eve"), "{screen}");
        // The emoji still reacts.
        let commands = click(&mut app, "❤️", &screen);
        assert!(
            matches!(&commands[..], [Command::React { event_id, key }] if event_id == "$5" && key == "❤️"),
            "{commands:?}"
        );

        // "▴ less" folds them again, as does "x" on the selected message.
        click(&mut app, "▴ less", &screen);
        let screen = render_to_text(&mut app, 110, 34);
        assert!(screen.contains("👍 you, Amy, Ben +2"), "{screen}");
        app.on_event(Event::Key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('x'),
            KeyModifiers::NONE,
        )));
        let screen = render_to_text(&mut app, 110, 34);
        assert!(screen.contains("👍 you, Amy, Ben, Cleo, Dan"), "{screen}");
    }

    #[test]
    fn reactions_that_do_not_fit_go_onto_the_next_line() {
        let reaction = |key: &str, names: &[&str]| Reaction {
            key: key.into(),
            count: names.len(),
            mine: false,
            senders: names.iter().map(|name| (*name).to_owned()).collect(),
        };
        let item = Item {
            event_id: "$r".into(),
            reactions: vec![
                reaction("👍", &["Amy Winter", "Ben Okafor"]),
                reaction("🎉", &["Cleo"]),
            ],
            ..Item::default()
        };
        let text = |lines: Vec<TimelineLine>| -> Vec<String> {
            lines
                .into_iter()
                .map(|line| line.line.to_string())
                .collect()
        };
        let theme = crate::theme::all().remove(0);
        assert_eq!(
            text(reaction_row(&item, true, 30, &theme)),
            ["  👍 Amy Winter, Ben Okafor  ", "  🎉 Cleo  "]
        );
        assert_eq!(
            text(reaction_row(&item, false, 30, &theme)),
            ["  👍 2  🎉 1  "]
        );
    }

    #[test]
    fn the_emoji_button_opens_the_picker_and_a_click_puts_one_in() {
        let mut app = sample();
        open_lunch(&mut app);
        app.composer.set("yum ");
        let screen = render_to_text(&mut app, 110, 34);
        click(&mut app, "+ emoji", &screen);
        let screen = render_to_text(&mut app, 110, 34);
        assert!(screen.contains("Used often"), "{screen}");
        assert!(screen.contains("Smileys"), "{screen}");
        // The name of the emoji under the cursor.
        assert!(screen.contains("thumbs up"), "{screen}");

        for c in "pizza".chars() {
            app.on_event(Event::Key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char(c),
                KeyModifiers::NONE,
            )));
        }
        let screen = render_to_text(&mut app, 110, 34);
        assert!(!screen.contains("Smileys"), "{screen}");
        let commands = click(&mut app, "🍕", &screen);
        assert!(commands.is_empty(), "{commands:?}");
        assert_eq!(app.composer.text(), "yum 🍕");
        assert!(app.overlays.is_empty());
    }

    #[test]
    fn more_on_the_reaction_bar_opens_the_picker_and_a_click_reacts() {
        let mut app = sample();
        open_lunch(&mut app);
        let screen = render_to_text(&mut app, 110, 34);
        click(&mut app, "Pizza it is", &screen);
        let screen = render_to_text(&mut app, 110, 34);
        click(&mut app, "a React", &screen);
        let screen = render_to_text(&mut app, 110, 34);
        click(&mut app, "0 More…", &screen);
        let screen = render_to_text(&mut app, 110, 34);
        assert!(screen.contains("React with"), "{screen}");
        // Not on the bar: the tenth of the favourites.
        let commands = click(&mut app, "🔥", &screen);
        assert!(
            matches!(&commands[..], [Command::React { event_id, key }] if event_id == "$3" && key == "🔥"),
            "{commands:?}"
        );
        assert!(app.overlays.is_empty());
        assert!(app.quick_reactions().contains(&"🔥".to_owned()));
    }

    #[test]
    fn own_open_polls_offer_end_poll_with_confirmation() {
        let mut app = sample();
        open_lunch(&mut app);
        app.state.items[2].is_own = true;
        app.state.items[2].can_delete = true;
        let screen = render_to_text(&mut app, 110, 34);
        assert!(click(&mut app, "[ End poll ]", &screen).is_empty());
        let screen = render_to_text(&mut app, 110, 34);
        assert!(screen.contains("End this poll for everyone?"), "{screen}");
        let commands = click(&mut app, "Enter End poll", &screen);
        assert!(
            matches!(&commands[..], [Command::EndPoll { poll_event_id, .. }] if poll_event_id == "$poll"),
            "{commands:?}"
        );
    }

    #[test]
    fn file_button_opens_path_suggestions() {
        let mut app = sample();
        open_lunch(&mut app);
        let screen = render_to_text(&mut app, 110, 34);
        click(&mut app, "+ file", &screen);
        assert_eq!(app.composer.text(), "/file ~/");
        assert_eq!(app.focus, Focus::Composer);
    }

    #[test]
    fn poll_button_fills_in_a_template() {
        let mut app = sample();
        open_lunch(&mut app);
        let screen = render_to_text(&mut app, 110, 34);
        click(&mut app, "+ poll", &screen);
        assert!(app.composer.text().starts_with("/poll "));
        assert_eq!(app.focus, Focus::Composer);
    }

    #[test]
    fn keyboard_selection_scrolls_the_message_into_view() {
        let mut app = sample();
        app.state.open_room = Some("!lunch".into());
        let items: Vec<Item> = (0..40u64)
            .map(|n| {
                message(
                    &format!("m{n}"),
                    "Amy",
                    &format!("line number {n}"),
                    n * 400_000,
                )
            })
            .collect();
        app.on_update(Update::Timeline {
            room_id: "!lunch".into(),
            items,
        });
        render_to_text(&mut app, 100, 20);
        app.selected = Some("$m2".into());
        app.viewport.reveal_selection();
        let screen = render_to_text(&mut app, 100, 20);
        assert!(screen.contains("line number 2"), "{screen}");
        assert!(!app.viewport.at_bottom());
    }

    #[test]
    fn pictures_are_drawn_under_their_message_once_loaded() {
        let mut app = sample();
        app.state.open_room = Some("!lunch".into());
        let mut picture = message("pic", "Amy", "[image] cat.png", 1_000);
        picture.kind = ItemKind::Media;
        picture.image = true;
        app.on_update(Update::Timeline {
            room_id: "!lunch".into(),
            items: vec![picture],
        });
        let before = render_to_text(&mut app, 100, 24);
        assert!(
            before.contains("[image] cat.png") && !before.contains('▀'),
            "{before}"
        );

        // 4 x 4 pixels: red on top, transparent below.
        let mut pixels = vec![[255, 0, 0, 255]; 8];
        pixels.extend(vec![[0, 0, 0, 0]; 8]);
        app.on_update(Update::Preview {
            event_id: "$pic".into(),
            preview: Preview {
                width: 4,
                height: 4,
                pixels,
                ..Preview::default()
            },
        });
        let after = render_to_text(&mut app, 100, 24);
        let row = after
            .lines()
            .find(|line| line.contains("▀▀▀▀"))
            .expect("picture row");
        assert!(!row.contains("▀▀▀▀▀"), "{row}");
        // The fully transparent second row draws nothing.
        assert_eq!(after.matches("▀▀▀▀").count(), 1, "{after}");
    }

    #[test]
    fn mention_popup_lists_matches_and_is_clickable() {
        let mut app = sample();
        open_lunch(&mut app);
        app.focus = Focus::Composer;
        app.on_update(Update::Members {
            room_id: "!lunch".into(),
            members: vec![
                crate::model::Member {
                    user_id: "@amy:example.org".into(),
                    name: "Amy Winter".into(),
                },
                crate::model::Member {
                    user_id: "@ben:example.org".into(),
                    name: "Ben Okafor".into(),
                },
            ],
        });
        app.composer.set("hey @");
        let screen = render_to_text(&mut app, 110, 34);
        assert!(screen.contains("mention"), "{screen}");
        assert!(
            screen.contains("Ben Okafor") && screen.contains("@ben:example.org"),
            "{screen}"
        );
        click(&mut app, "@ben:example.org", &screen);
        assert_eq!(app.composer.text(), "hey @Ben Okafor ");
        let screen = render_to_text(&mut app, 110, 34);
        assert!(!screen.contains("mention ·"), "{screen}");
    }

    #[test]
    fn verification_dialog_shows_emoji_and_blocks_clicks_underneath() {
        let mut app = sample();
        open_lunch(&mut app);
        app.state.accounts[0].verified = Some(false);
        let screen = render_to_text(&mut app, 110, 34);
        assert!(screen.contains("unverified device"), "{screen}");

        app.on_update(Update::Verify(Some(VerifyState::Emojis(vec![
            ("🐶".into(), "Dog".into()),
            ("🔑".into(), "Key".into()),
        ]))));
        let screen = render_to_text(&mut app, 110, 34);
        assert!(screen.contains("Verify"), "{screen}");
        assert!(screen.contains("Dog") && screen.contains("Key"), "{screen}");

        // A click on the sidebar behind the dialog does nothing.
        assert!(click(&mut app, "Dev Chat", &screen).is_empty());
        assert_eq!(app.state.open_room.as_deref(), Some("!lunch"));

        let commands = click(&mut app, "Enter They match", &screen);
        assert!(
            matches!(&commands[..], [Command::Verify(VerifyInput::Confirm)]),
            "{commands:?}"
        );
    }

    #[test]
    fn kitty_pictures_reserve_placeholder_cells_and_upload_once() {
        let mut app = sample();
        app.graphics = crate::app::Graphics::Kitty;
        app.state.open_room = Some("!lunch".into());
        let mut picture = message("pic", "Amy", "[image] cat.png", 1_000);
        picture.kind = ItemKind::Media;
        picture.image = true;
        app.on_update(Update::Timeline {
            room_id: "!lunch".into(),
            items: vec![picture],
        });
        app.on_update(Update::Preview {
            event_id: "$pic".into(),
            preview: Preview {
                width: 2,
                height: 2,
                pixels: vec![[1, 2, 3, 255]; 4],
                png: b"png".to_vec(),
                png_columns: 6,
                png_rows: 2,
            },
        });

        // The picture is uploaded exactly once, as picture 1 in a 6 x 2 box.
        let uploads = app.take_graphics();
        assert_eq!(uploads.len(), 1);
        assert!(uploads[0].contains("i=1,c=6,r=2"), "{:?}", uploads[0]);
        assert!(app.take_graphics().is_empty());

        let screen = render_to_text(&mut app, 100, 24);
        let rows: Vec<&str> = screen
            .lines()
            .filter(|line| line.contains('\u{10EEEE}'))
            .collect();
        assert_eq!(rows.len(), 2, "{screen}");
        assert_eq!(rows[0].matches('\u{10EEEE}').count(), 6);
        assert!(
            !screen.contains('▀'),
            "no half-block fallback when the terminal draws it"
        );

        // Leaving the room removes the picture from the terminal.
        let mut commands = Vec::new();
        app.on_hit(HitAction::OpenRoom("!dev".into()), &mut commands);
        let removals = app.take_graphics();
        assert!(
            removals
                .iter()
                .any(|sequence| sequence.contains("a=d,d=I,i=1")),
            "{removals:?}"
        );
    }

    #[test]
    fn narrow_terminals_drop_the_sidebar_but_keep_the_chat() {
        let mut app = sample();
        open_lunch(&mut app);
        let screen = render_to_text(&mut app, 50, 20);
        assert!(!screen.contains("ROOMS"), "{screen}");
        assert!(screen.contains("Pizza it is"), "{screen}");
    }

    #[test]
    fn composer_wraps_and_tracks_the_cursor() {
        let (rows, row, col) = composer_layout("abcdefgh", 8, 5);
        assert_eq!(rows, ["abcde", "fgh"]);
        assert_eq!((row, col), (1, 3));
        let (rows, row, col) = composer_layout("abcde", 5, 5);
        assert_eq!(rows.len(), 2);
        assert_eq!((row, col), (1, 0));
        let (_, row, col) = composer_layout("abcdefgh", 2, 5);
        assert_eq!((row, col), (0, 2));
    }

    #[test]
    fn fit_truncates_by_terminal_width() {
        assert_eq!(fit("Lunch Club", 20), "Lunch Club");
        assert_eq!(fit("Lunch Club", 6), "Lunch…");
    }
}
