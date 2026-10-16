//! Read-only shortcut reference. Help owns its query and scroll, never a document.
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    editor::Editor,
    input::Key,
    text::{cell_width, display_column, display_grapheme, pop_grapheme, visible_text},
    theme::Theme,
};

const CATEGORIES: &[&str] = &[
    "Help",
    "Editing",
    "Files",
    "Navigation / LSP",
    "Git",
    "Review",
    "AI / Codex",
];
const MAX_QUERY_BYTES: usize = 256;

#[derive(Clone, Copy)]
struct Shortcut {
    category: &'static str,
    context: &'static str,
    keys: &'static str,
    description: &'static str,
}

const fn entry(
    category: &'static str,
    context: &'static str,
    keys: &'static str,
    description: &'static str,
) -> Shortcut {
    Shortcut {
        category,
        context,
        keys,
        description,
    }
}

struct CommandBinding {
    sequence: &'static [Key],
    command: &'static str,
    shortcut: Shortcut,
}

// These entries drive both Normal-mode dispatch and the reference. Operators,
// counts and mode guards remain in the existing input state machine.
const NORMAL_COMMANDS: &[CommandBinding] = &[
    CommandBinding {
        sequence: &[Key::Char('g'), Key::Char('d')],
        command: "Definition",
        shortcut: entry(
            "Navigation / LSP",
            "Normal",
            "gd",
            "Jump to definition; choose a result if there are several. Requires a ready language server. Ctrl-o returns.",
        ),
    },
    CommandBinding {
        sequence: &[Key::Char('g'), Key::Char('r')],
        command: "References",
        shortcut: entry(
            "Navigation / LSP",
            "Normal",
            "gr",
            "Find references; jump to one location or choose among results. Requires a ready language server.",
        ),
    },
    CommandBinding {
        sequence: &[Key::Char(' '), Key::Char('g'), Key::Char('d')],
        command: "diff",
        shortcut: entry(
            "Git",
            "Normal",
            "Space g d",
            "Inspect the current file's Git diff against the selected base, including unsaved text. Press keys in sequence.",
        ),
    },
    CommandBinding {
        sequence: &[Key::Char(' '), Key::Char('g'), Key::Char('c')],
        command: "changes",
        shortcut: entry(
            "Git",
            "Normal",
            "Space g c",
            "List repository disk/index changes and separately labeled unsaved buffers.",
        ),
    },
    CommandBinding {
        sequence: &[Key::Char(' '), Key::Char('p'), Key::Char('v')],
        command: "tree",
        shortcut: entry(
            "Files",
            "Normal",
            "Space p v",
            "Open the file tree. Press keys in sequence; Esc returns to the original source.",
        ),
    },
];

pub(crate) fn normal_command(sequence: &[Key]) -> Option<&'static str> {
    NORMAL_COMMANDS
        .iter()
        .find(|binding| binding.sequence == sequence)
        .map(|binding| binding.command)
}

// Context is explicit: for example, ]c is a Normal-mode hunk jump, whereas ]
// navigates hunk headers in a diff. This is the canonical in-editor reference.
const SHORTCUTS: &[Shortcut] = &[
    entry(
        "Help",
        "Any view or mode",
        "F1",
        "Open keyboard shortcut help; press F1 or Esc to return to the unchanged context.",
    ),
    entry(
        "Help",
        "Command",
        ":help [QUERY]",
        "Browse all shortcuts or search immediately, for example :help definition, :help diff, or :help Codex.",
    ),
    entry(
        "Help",
        "Help",
        "Type / Backspace / Ctrl-u",
        "Filter by words in keys, descriptions, modes or categories; Backspace removes a grapheme, Ctrl-u clears. Search ignores case.",
    ),
    entry(
        "Help",
        "Help",
        "Up / Down / Ctrl-p / Ctrl-n",
        "Scroll help one wrapped line. Enter never executes a shortcut.",
    ),
    entry(
        "Help",
        "Help",
        "PageUp / PageDown / Home / End",
        "Scroll a page or go to the first/last help line.",
    ),
    entry(
        "Help",
        "Help",
        "Tab / Shift-Tab",
        "Next/previous category, including All. The search stays active.",
    ),
    entry(
        "Help",
        "Help",
        "Esc / Ctrl-[ / Ctrl-c",
        "Close help and resume the original view, mode, cursor, selection and pending input.",
    ),
    entry(
        "Editing",
        "Normal / Visual",
        "h j k l / arrows",
        "Move left, down, up or right by grapheme or line. Prefix motions with a count.",
    ),
    entry(
        "Editing",
        "Normal / Visual",
        "w b e / W B E",
        "Next word, previous word, word end; capitals use whitespace-delimited WORDs.",
    ),
    entry(
        "Editing",
        "Normal / Visual",
        "0 / ^ / $ / gg / G",
        "Line start, first nonblank, line end, first line, last line. 12G goes to line 12.",
    ),
    entry(
        "Editing",
        "Normal / Visual",
        "fX / FX / tX / TX / ; / ,",
        "Find or stop before character X forward/backward; repeat or reverse the last character find.",
    ),
    entry(
        "Editing",
        "Normal / Visual",
        "%",
        "Jump to the matching bracket.",
    ),
    entry(
        "Editing",
        "Normal",
        "Ctrl-u / Ctrl-d / PageUp / PageDown",
        "Move half a screen up or down.",
    ),
    entry(
        "Editing",
        "Normal",
        ":",
        "Enter Command mode. Type a command and press Enter to run it; Esc cancels the prompt.",
    ),
    entry(
        "Editing",
        "Normal",
        "i / a / I / A / o / O",
        "Enter Insert before/after the cursor, at first nonblank/end, or on a new line below/above.",
    ),
    entry(
        "Editing",
        "Normal",
        "v / V",
        "Start characterwise or linewise Visual selection.",
    ),
    entry(
        "Editing",
        "Visual",
        "Motions / o / d / c / y",
        "Extend selection, swap anchor, delete, change or yank the selection.",
    ),
    entry(
        "Editing",
        "Normal",
        "d / c / y + motion or object",
        "Delete, change or yank. Counts multiply: 2d3w deletes six words.",
    ),
    entry(
        "Editing",
        "Operator / Visual",
        "iw / aw / iW / aW",
        "Select inner/around word or whitespace-delimited WORD.",
    ),
    entry(
        "Editing",
        "Operator / Visual",
        "i or a + quote / ( / [ / {",
        "Inner/around quotes or bracket pairs, for example ciw, di{, ya\". ib/ab and iB/aB alias parentheses and braces.",
    ),
    entry(
        "Editing",
        "Normal",
        "dd / cc / yy / D / C / Y",
        "Delete/change/yank lines; D/C operate through line end, Y yanks lines.",
    ),
    entry(
        "Editing",
        "Normal",
        "x / Delete / s / p / P",
        "Delete graphemes; s replaces via Insert. Put the unnamed register after/before the cursor or line.",
    ),
    entry(
        "Editing",
        "Normal",
        "u / Ctrl-r",
        "Undo or redo. Counts are supported; each Insert session is an undo group.",
    ),
    entry(
        "Editing",
        "Insert",
        "Text / Enter / Tab / Backspace / Delete",
        "Insert text, newline or literal tab; delete/join lines. Arrows, Home and End move and start a new undo group.",
    ),
    entry(
        "Editing",
        "Editing / prompt",
        "Esc / Ctrl-[ / Ctrl-c",
        "Leave Insert/Visual or cancel pending input and prompts.",
    ),
    entry(
        "Editing",
        "Normal / Visual",
        "/TEXT / ?TEXT / n / N",
        "Search forward/backward, then Enter; n repeats and N reverses. Literal case-sensitive search wraps. Esc cancels the prompt.",
    ),
    entry(
        "Editing",
        "Command",
        ":noh",
        "Hide source search highlights.",
    ),
    entry(
        "Files",
        "Command",
        ":w [PATH] / :wq [PATH]",
        "Save the active buffer; wq then quits only if all buffers are saved.",
    ),
    entry(
        "Files",
        "Command",
        ":q / :q!",
        "Quit when all buffers are saved; q! explicitly discards unsaved edits.",
    ),
    entry(
        "Files",
        "Command",
        ":e PATH / :e / :e!",
        "Open or switch to a file; without a path, reload. The ! explicitly discards that file's local edits.",
    ),
    entry(
        "Files",
        "Command",
        ":enew / :bn / :bp / :b ID / :ls",
        "New buffer, next/previous/numbered buffer, or buffer list. Other buffers retain unsaved edits.",
    ),
    entry(
        "Files",
        "Command",
        ":bd / :bd!",
        "Close the active buffer; ! discards its unsaved edits.",
    ),
    entry(
        "Files",
        "Normal / command",
        "Ctrl-p / :files [DIRECTORY]",
        "Open the fuzzy file picker; type to filter, arrows or Ctrl-n/Ctrl-p select, Enter opens, Esc returns.",
    ),
    entry(
        "Files",
        "Command",
        ":tree [DIRECTORY]",
        "Toggle the file tree; optionally choose a root directory.",
    ),
    entry(
        "Files",
        "Tree",
        "j k / arrows / Enter / R",
        "Select entries, Right/l expands, Left/h collapses or selects parent, Enter opens a file, R refreshes.",
    ),
    entry(
        "Files",
        "Tree",
        "a / A / r / d",
        "Create a file/directory, rename/move, or delete after typing yes. Esc cancels the prompt.",
    ),
    entry(
        "Files",
        "Buffers / Recent",
        "j k / Up Down / Enter / Esc",
        "Select a buffer/file, open it or return to source.",
    ),
    entry(
        "Files",
        "Command",
        ":theme tokyo-night|catppuccin|gruvbox",
        "Select a session theme; :theme reports the current theme.",
    ),
    entry(
        "Navigation / LSP",
        "Normal / command",
        "Ctrl-o / :back / Tab / Ctrl-i / :forward",
        "Return to an earlier source location or go forward again, including cursor and viewport.",
    ),
    entry(
        "Navigation / LSP",
        "Command",
        ":recent / :grep TEXT / :grepdir DIRECTORY",
        "Browse recent paths, search project disk text, or choose the search root.",
    ),
    entry(
        "Navigation / LSP",
        "Project search",
        "j k / arrows / PageUp PageDown / h l / Enter / r",
        "Select results, scroll long lines, open a match, or repeat the query. Esc cancels and returns.",
    ),
    entry(
        "Navigation / LSP",
        "Command",
        ":Definition / :References",
        "List definition/reference locations from the language server; Enter opens a result.",
    ),
    entry(
        "Navigation / LSP",
        "Command",
        ":Hover / :Signature",
        "Show documentation or signatures at the cursor.",
    ),
    entry(
        "Navigation / LSP",
        "Command",
        ":Diagnostics / :Dnext / :Dprev",
        "List diagnostics across open sources or jump to next/previous diagnostic in this buffer, wrapping.",
    ),
    entry(
        "Navigation / LSP",
        "Command",
        ":Outline / :Symbols / :WorkspaceSymbols [QUERY]",
        "Browse document symbols or search symbols in the current language-server project.",
    ),
    entry(
        "Navigation / LSP",
        "Command / Calls list",
        ":Calls / i / o / Backspace",
        "Open call hierarchy; follow incoming/outgoing calls or return to the parent list. Enter opens the declaration.",
    ),
    entry(
        "Navigation / LSP",
        "Command / Insert",
        ":Complete / Ctrl-n",
        "Request completions; Insert typing also shows suggestions after a short pause.",
    ),
    entry(
        "Navigation / LSP",
        "Completion popup",
        "Up Down / Ctrl-n Ctrl-p / Tab / Enter / Esc / Ctrl-e",
        "Select, accept or dismiss suggestions. Additional edits and commands require a preview.",
    ),
    entry(
        "Navigation / LSP",
        "Completion list",
        "d / Enter / Tab",
        "Resolve documentation or accept the selected suggestion.",
    ),
    entry(
        "Navigation / LSP",
        "Command",
        ":Rename NAME / :CodeAction / :Format",
        "Preview symbol rename, fixes or formatting before applying changes. Source files are not saved automatically.",
    ),
    entry(
        "Navigation / LSP",
        "LSP results / edit preview",
        "j k / arrows / PageUp PageDown / h l / Enter / Esc",
        "Select/scroll results; Enter opens a result or applies an edit preview. Esc cancels and returns to source.",
    ),
    entry(
        "Navigation / LSP",
        "Command",
        ":LspInfo / :LspLog / :Doctor / :LspRestart",
        "Inspect server capabilities, logs, discovery or restart the current language-server session.",
    ),
    entry(
        "Navigation / LSP",
        "Command",
        ":LspInstall [LANGUAGE] / :LspInstallCancel [LANGUAGE]",
        "Install/retry or cancel a language server installation for kotlin, java or csharp.",
    ),
    entry(
        "Git",
        "Normal",
        "]c / [c",
        "Jump to next/previous hunk in the current source. Counts work, for example 2]c; Ctrl-o returns.",
    ),
    entry(
        "Git",
        "Normal / command",
        "]f / [f / :gnext / :gprev",
        "Open next/previous changed file; counts work with the keys and file navigation wraps.",
    ),
    entry(
        "Git",
        "Command",
        ":changes / :diff [HEAD|index|REV]",
        "Browse changed files or inspect a diff against current working content, including unsaved text.",
    ),
    entry(
        "Git",
        "Command",
        ":diff staged / :gitbase HEAD|index|REV / :gitrefresh",
        "Inspect HEAD versus index, choose the source baseline, or request a refresh.",
    ),
    entry(
        "Git",
        "Changes",
        "j k / Up Down / Enter / d / r / Esc",
        "Select a file, open source (historical diff for deleted files), inspect a diff, refresh or return.",
    ),
    entry(
        "Git",
        "Diff",
        "] / [ / n / p",
        "Next/previous hunk header or changed file. These are single keys in the diff view.",
    ),
    entry(
        "Git",
        "Diff",
        "j k / arrows / PageUp PageDown / Ctrl-u Ctrl-d / g G",
        "Scroll rows, move half a screen, or go to first/last row. h/l scroll long lines horizontally.",
    ),
    entry(
        "Git",
        "Diff",
        "Tab / 1 / 2 / 3 / r",
        "Toggle paired/inline layout; choose HEAD/index/staged comparison; refresh. Narrow terminals use inline layout.",
    ),
    entry(
        "Git",
        "Diff",
        "Enter / Esc / :diffclose",
        "Jump to the selected source line, or close the diff and restore its original source location.",
    ),
    entry(
        "Review",
        "Diff",
        "F / R / A",
        "Mark the file reviewed, hunk reviewed, or hunk needs-attention.",
    ),
    entry(
        "Review",
        "Command",
        ":review file|hunk reviewed|attention|unreviewed|clear",
        "Set/change/clear a mark on the current accepted comparison.",
    ),
    entry(
        "Review",
        "Command",
        ":review list [all|stale|attention|unreviewed|notes]",
        "List stored review records, IDs, attachments and freshness.",
    ),
    entry(
        "Review",
        "Command",
        ":review next|prev file|hunk [FILTER]",
        "Navigate review targets; the default filter is unreviewed.",
    ),
    entry(
        "Review",
        "Command",
        ":note file|hunk|old|new",
        "Create a review note on the file, selected hunk or selected old/new diff line.",
    ),
    entry(
        "Review",
        "Note draft",
        ":note save / :note save-new / :note cancel",
        "Use ordinary editing keys for note text, then save, preserve a conflict as a new note, or discard the draft and return.",
    ),
    entry(
        "Review",
        "Command",
        ":notes / :note next|prev / :note edit ID / :note remove ID",
        "List/navigate notes, edit note text or explicitly remove a note.",
    ),
    entry(
        "Review",
        "Review list",
        "j k / Up Down / Enter / e / Esc",
        "Select a record, inspect its comparison, edit a note or return.",
    ),
    entry(
        "Review",
        "Command",
        ":review revalidate ID file|hunk / :review clear ID",
        "Retie a mark to current content or explicitly remove the record.",
    ),
    entry(
        "Review",
        "Command",
        ":note attach ID file|hunk|old|new",
        "Correct a note's attachment to current content.",
    ),
    entry(
        "Review",
        "Command",
        ":review snapshot / :review snapshots",
        "Capture an immutable checkpoint or show the retained checkpoint range.",
    ),
    entry(
        "Review",
        "Command",
        ":review full / :review since [ID]",
        "Compare the full baseline or a checkpoint against current content, including unsaved buffers.",
    ),
    entry(
        "Review",
        "Command",
        ":review export PATH / :review reload / :review retry",
        "Export progress/notes to a new Markdown file outside the worktree, reload metadata or retry a failed write.",
    ),
    entry(
        "AI / Codex",
        "Command",
        ":ai context file",
        "Start a question for the Codex agent: capture the current file, including unsaved text, and inspect the preview.",
    ),
    entry(
        "AI / Codex",
        "Command",
        ":ai ask QUESTION",
        "Set the next agent question, for example :ai ask Explain this code. Inspect context with :ai preview before sending.",
    ),
    entry(
        "AI / Codex",
        "Command",
        ":ai send",
        "Explicitly send captured context and the question to Codex. Requires an installed, authenticated Codex CLI.",
    ),
    entry(
        "AI / Codex",
        "Visual / VisualLine",
        "Ctrl-g",
        "Capture and preview the exact selected code for an agent question.",
    ),
    entry(
        "AI / Codex",
        "Command",
        ":ai context selection|symbol|hunk|diff",
        "Capture the current selection, selected Outline symbol, selected hunk or current diff.",
    ),
    entry(
        "AI / Codex",
        "Command",
        ":ai add file PATH / :ai add tests PATH",
        "Attach an explicitly named source or test file as text.",
    ),
    entry(
        "AI / Codex",
        "Command",
        ":ai add diagnostics|references|feedback",
        "Attach diagnostics, reference source lines or review feedback to the next question.",
    ),
    entry(
        "AI / Codex",
        "Command",
        ":ai / :ai preview",
        "Reopen the conversation or inspect the next question and captured context.",
    ),
    entry(
        "AI / Codex",
        "AI view",
        "j k / arrows / g G / PageUp PageDown / h l / Enter",
        "Select/scroll answers and long lines; Enter opens a current captured source line or citation. Ctrl-o returns after a jump.",
    ),
    entry(
        "AI / Codex",
        "AI view / command",
        "Esc / Ctrl-c / :ai cancel",
        "Esc returns to source while the request continues; Ctrl-c or :ai cancel explicitly cancels inference.",
    ),
    entry(
        "AI / Codex",
        "Command",
        ":ai edits / :ai apply",
        "Preview the latest answer's suggested revisions, then explicitly approve them.",
    ),
    entry(
        "AI / Codex",
        "Command",
        ":ai save [PATH] / :ai load PATH / :ai new",
        "Save the conversation outside the project, restore one or start another. load!/new! explicitly discard an unsaved conversation.",
    ),
];

fn shortcuts() -> impl Iterator<Item = Shortcut> {
    NORMAL_COMMANDS
        .iter()
        .map(|binding| binding.shortcut)
        .chain(SHORTCUTS.iter().copied())
}

#[derive(Clone, Copy)]
enum RowStyle {
    Category,
    Keys,
    Description,
}

pub struct Help {
    query: String,
    category: usize,
    scroll: usize,
    height: usize,
    width: usize,
    rows: Vec<(String, RowStyle)>,
    matches: usize,
    dirty: bool,
}

impl Help {
    fn new(query: &str) -> Self {
        let mut help = Self {
            query: String::new(),
            category: 0,
            scroll: 0,
            height: 1,
            width: 0,
            rows: Vec::new(),
            matches: 0,
            dirty: true,
        };
        for ch in query.chars().filter(|ch| !ch.is_control()) {
            if help.query.len() + ch.len_utf8() > MAX_QUERY_BYTES {
                break;
            }
            help.query.push(ch);
        }
        help
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn category(&self) -> &'static str {
        self.category
            .checked_sub(1)
            .map_or("All", |index| CATEGORIES[index])
    }

    pub fn scroll(&self) -> usize {
        self.scroll
    }

    pub(crate) fn handle_key(&mut self, key: Key) -> bool {
        match key {
            Key::Escape | Key::F1 | Key::Ctrl('[' | 'c') => return true,
            Key::Tab | Key::BackTab => {
                let count = CATEGORIES.len() + 1;
                self.category =
                    (self.category + if key == Key::Tab { 1 } else { count - 1 }) % count;
                self.dirty = true;
            }
            Key::Ctrl('u') => {
                self.query.clear();
                self.dirty = true;
            }
            Key::Backspace => {
                pop_grapheme(&mut self.query);
                self.dirty = true;
            }
            Key::Char(ch)
                if !ch.is_control() && self.query.len() + ch.len_utf8() <= MAX_QUERY_BYTES =>
            {
                self.query.push(ch);
                self.dirty = true;
            }
            Key::Down | Key::Ctrl('n' | 'j') => self.scroll = self.scroll.saturating_add(1),
            Key::Up | Key::Ctrl('p' | 'k') => self.scroll = self.scroll.saturating_sub(1),
            Key::PageDown => self.scroll = self.scroll.saturating_add(self.height),
            Key::PageUp => self.scroll = self.scroll.saturating_sub(self.height),
            Key::Home => self.scroll = 0,
            Key::End => self.scroll = self.rows.len().saturating_sub(self.height),
            _ => {}
        }
        if self.dirty {
            self.scroll = 0;
        }
        self.scroll = self.scroll.min(self.rows.len().saturating_sub(self.height));
        false
    }

    fn layout(&mut self, width: usize, height: usize) {
        self.height = height;
        if self.dirty || self.width != width {
            self.width = width;
            self.rows.clear();
            self.matches = 0;
            let query = self.query.to_lowercase();
            for category in CATEGORIES {
                if self.category != 0 && self.category() != *category {
                    continue;
                }
                let mut heading = false;
                for shortcut in shortcuts().filter(|shortcut| shortcut.category == *category) {
                    let searchable = format!(
                        "{} {} {} {}",
                        shortcut.category, shortcut.context, shortcut.keys, shortcut.description
                    )
                    .to_lowercase();
                    if !query
                        .split_whitespace()
                        .all(|word| searchable.contains(word))
                    {
                        continue;
                    }
                    if !heading {
                        self.add_rows(category, RowStyle::Category);
                        heading = true;
                    }
                    self.matches += 1;
                    self.add_rows(
                        &format!("{}  ({})", shortcut.keys, shortcut.context),
                        RowStyle::Keys,
                    );
                    self.add_rows(shortcut.description, RowStyle::Description);
                    self.rows.push((String::new(), RowStyle::Description));
                }
            }
            if self.matches == 0 {
                self.add_rows(
                    "No shortcuts match. Ctrl-u clears search; Tab changes category.",
                    RowStyle::Description,
                );
            }
            self.dirty = false;
        }
        self.scroll = self.scroll.min(self.rows.len().saturating_sub(self.height));
    }

    fn add_rows(&mut self, text: &str, style: RowStyle) {
        self.rows
            .extend(wrap(text, self.width).into_iter().map(|row| (row, style)));
    }
}

impl Editor {
    pub fn open_help(&mut self, query: &str) {
        // Do not reset context, commit Insert, clear a selection, or move a cursor.
        self.help = Some(Help::new(query));
    }
}

// Word wrap with grapheme-safe breaks for commands longer than the terminal.
// Materializing the small static catalog makes every wrapped line scrollable,
// including in one-column terminals, without u16 scroll truncation.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut rows = Vec::new();
    let mut row = String::new();
    let mut cells = 0;
    for word in text.split_whitespace() {
        if cells > 0 {
            if cells + 1 + display_column(word, usize::MAX) > width {
                rows.push(std::mem::take(&mut row));
                cells = 0;
            } else {
                row.push(' ');
                cells += 1;
            }
        }
        for grapheme in word.graphemes(true) {
            let size = cell_width(grapheme, cells);
            if cells > 0 && cells + size > width {
                rows.push(std::mem::take(&mut row));
                cells = 0;
            }
            if size > width {
                row.push('�');
                cells += 1;
            } else {
                row.push_str(&display_grapheme(grapheme, cells));
                cells += size;
            }
        }
    }
    if !row.is_empty() {
        rows.push(row);
    }
    rows
}

pub(crate) fn render(frame: &mut Frame<'_>, help: &mut Help, theme: &Theme) {
    let area = frame.area();
    let header = if area.height >= 4 { 2 } else { 0 };
    let footer = u16::from(area.height >= 2);
    let body = Rect::new(
        area.x,
        area.y + header,
        area.width,
        area.height - header - footer,
    );
    help.layout(usize::from(area.width), usize::from(body.height));
    let base = Style::default().fg(theme.foreground).bg(theme.background);
    let lines: Vec<_> = help
        .rows
        .iter()
        .skip(help.scroll)
        .take(usize::from(body.height))
        .map(|(text, kind)| {
            let style = match kind {
                RowStyle::Category => base.fg(theme.command).add_modifier(Modifier::BOLD),
                RowStyle::Keys => base.fg(theme.normal).add_modifier(Modifier::BOLD),
                RowStyle::Description => base,
            };
            Line::from(Span::styled(text.as_str(), style))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), body);
    if header > 0 {
        let title = format!("Help | {} | {} shortcuts", help.category(), help.matches);
        frame.render_widget(
            Paragraph::new(visible_text(&title, 0, area.width as usize))
                .style(base.bg(theme.status)),
            Rect::new(area.x, area.y, area.width, 1),
        );
        let prompt = if help.query.is_empty() {
            "Type to search".to_string()
        } else {
            format!("/{}", help.query)
        };
        let cells = display_column(&prompt, usize::MAX);
        let left = if help.query.is_empty() {
            0
        } else {
            cells.saturating_sub(area.width as usize - 1)
        };
        frame.render_widget(
            Paragraph::new(visible_text(&prompt, left, area.width as usize))
                .style(base.fg(theme.muted)),
            Rect::new(area.x, area.y + 1, area.width, 1),
        );
        let cursor = if help.query.is_empty() {
            0
        } else {
            cells - left
        };
        frame.set_cursor_position((area.x + cursor as u16, area.y + 1));
    }
    if footer > 0 {
        let hint = if area.width < 60 {
            "Esc back | Tab group".to_string()
        } else {
            format!(
                "Esc return | Up/Down PgUp/PgDn | Tab group | Ctrl-u clear | {}/{}",
                help.scroll + 1,
                help.rows.len()
            )
        };
        frame.render_widget(
            Paragraph::new(visible_text(&hint, 0, area.width as usize))
                .style(base.bg(theme.status)),
            Rect::new(area.x, area.y + area.height - 1, area.width, 1),
        );
    }
}
