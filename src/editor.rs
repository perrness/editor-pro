use std::{collections::BTreeMap, io, path::Path};

use crate::{
    buffers::BufferSlot,
    file::{Document, MAX_FILE_BYTES},
    input::Key,
    motion::{self, Motion, Selection},
    picker::FilePicker,
    search::Search,
    text::pop_grapheme,
    theme::ThemeName,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Visual,
    VisualLine,
    Command,
    Search,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "NORMAL",
            Self::Insert => "INSERT",
            Self::Visual => "VISUAL",
            Self::VisualLine => "V-LINE",
            Self::Command => "COMMAND",
            Self::Search => "SEARCH",
        }
    }
}

#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct Viewport {
    pub top: usize,
    pub left: usize,
    pub height: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Operator {
    Delete,
    Change,
    Yank,
}

#[derive(Default)]
struct Pending {
    count: usize,
    operator: Option<Operator>,
    operator_count: usize,
    g: bool,
    object: Option<bool>,
    find: Option<(bool, bool)>,
}

#[derive(Clone, Default)]
pub struct Register {
    pub text: String,
    pub linewise: bool,
}

#[derive(Default, Debug, PartialEq, Eq)]
pub enum View {
    #[default]
    Editing,
    Buffers {
        selected: usize,
    },
    Files,
}

pub struct Editor {
    pub document: Document,
    pub active_buffer_id: usize,
    pub(crate) next_buffer_id: usize,
    pub(crate) buffers: BTreeMap<usize, BufferSlot>,
    pub view: View,
    pub picker: Option<FilePicker>,
    pub theme: ThemeName,
    pub mode: Mode,
    pub viewport: Viewport,
    pub command: String,
    pub message: String,
    pub message_is_error: bool,
    pub quit: bool,
    pending: Pending,
    pub selection_anchor: Option<usize>,
    pub register: Register,
    last_find: Option<Motion>,
    pub search: Search,
    pub search_backward: bool,
    search_mode: Mode,
}

impl Editor {
    pub fn new(document: Document) -> Self {
        Self {
            document,
            active_buffer_id: 1,
            next_buffer_id: 2,
            buffers: BTreeMap::new(),
            view: View::Editing,
            picker: None,
            theme: ThemeName::default(),
            mode: Mode::Normal,
            viewport: Viewport::default(),
            command: String::new(),
            message: "i  insert   :w  save   :q  quit   :help  keys".into(),
            message_is_error: false,
            quit: false,
            pending: Pending::default(),
            selection_anchor: None,
            register: Register::default(),
            last_find: None,
            search: Search::default(),
            search_backward: false,
            search_mode: Mode::Normal,
        }
    }

    pub fn handle_key(&mut self, key: Key) {
        if self.view == View::Files {
            self.picker_key(key);
            return;
        }
        if matches!(self.view, View::Buffers { .. }) {
            self.buffer_list_key(key);
            return;
        }
        if matches!(key, Key::Ctrl('c') | Key::Ctrl('[')) {
            self.escape();
            return;
        }
        match self.mode {
            Mode::Normal | Mode::Visual | Mode::VisualLine => self.normal_key(key),
            Mode::Insert => self.insert_key(key),
            Mode::Command => self.command_key(key),
            Mode::Search => self.search_key(key),
        }
    }

    pub fn selection(&self) -> Option<Selection> {
        self.selection_anchor.map(|anchor| {
            motion::visual(
                &self.document.buffer,
                anchor,
                self.mode == Mode::VisualLine
                    || (self.mode == Mode::Search && self.search_mode == Mode::VisualLine),
            )
        })
    }

    pub fn pending_label(&self) -> String {
        let mut label = String::new();
        if let Some(operator) = self.pending.operator {
            if self.pending.operator_count > 0 {
                label.push_str(&self.pending.operator_count.to_string());
            }
            label.push(match operator {
                Operator::Delete => 'd',
                Operator::Change => 'c',
                Operator::Yank => 'y',
            });
        }
        if self.pending.count > 0 {
            label.push_str(&self.pending.count.to_string());
        }
        if self.pending.g {
            label.push('g');
        }
        if let Some(around) = self.pending.object {
            label.push(if around { 'a' } else { 'i' });
        }
        if let Some((backward, till)) = self.pending.find {
            label.push(match (backward, till) {
                (false, false) => 'f',
                (true, false) => 'F',
                (false, true) => 't',
                (true, true) => 'T',
            });
        }
        label
    }

    fn count(&self) -> usize {
        if self.pending.operator.is_some()
            && (self.pending.operator_count > 0 || self.pending.count > 0)
        {
            self.pending
                .operator_count
                .max(1)
                .saturating_mul(self.pending.count.max(1))
                .min(10_000)
        } else {
            self.pending.count
        }
    }

    fn normal_key(&mut self, key: Key) {
        if key == Key::Escape {
            self.escape();
            return;
        }
        if let Some(around) = self.pending.object.take() {
            if let Key::Char(object) = key {
                if let Some(selection) =
                    motion::text_object(&self.document.buffer, object, around, self.count())
                {
                    if let Some(operator) = self.pending.operator {
                        self.operate(selection, operator, self.document.buffer.cursor());
                    } else {
                        self.selection_anchor = Some(selection.chars.start);
                        let end = self
                            .document
                            .buffer
                            .grapheme_start_before(selection.chars.end);
                        self.document
                            .buffer
                            .set_cursor(end.max(selection.chars.start), false);
                        self.mode = Mode::Visual;
                    }
                } else {
                    self.error("Text object not found");
                }
            }
            self.pending = Pending::default();
            return;
        }
        if let Some((backward, till)) = self.pending.find.take() {
            if let Key::Char(character) = key {
                let motion = Motion::Find {
                    character,
                    backward,
                    till,
                };
                self.last_find = Some(motion);
                self.run_motion(motion);
            } else {
                self.pending = Pending::default();
            }
            return;
        }
        if self.pending.g {
            self.pending.g = false;
            if key == Key::Char('g') {
                self.run_motion(Motion::FirstLine);
            } else {
                self.pending = Pending::default();
            }
            return;
        }
        if let Key::Char(digit @ '0'..='9') = key
            && (digit != '0' || self.pending.count > 0)
        {
            self.pending.count =
                (self.pending.count * 10 + digit.to_digit(10).unwrap() as usize).min(10_000);
            return;
        }
        if key == Key::Char('g') {
            self.pending.g = true;
            return;
        }
        if matches!(key, Key::Char('i' | 'a'))
            && (self.pending.operator.is_some() || self.selection_anchor.is_some())
        {
            self.pending.object = Some(key == Key::Char('a'));
            return;
        }
        if let Key::Char(ch @ ('f' | 'F' | 't' | 'T')) = key {
            self.pending.find = Some((ch.is_uppercase(), matches!(ch, 't' | 'T')));
            return;
        }
        let motion = match key {
            Key::Char('h') | Key::Left => Some(Motion::Left),
            Key::Char('l') | Key::Right => Some(Motion::Right),
            Key::Char('j') | Key::Down => Some(Motion::Down),
            Key::Char('k') | Key::Up => Some(Motion::Up),
            Key::Char('0') | Key::Home => Some(Motion::Start),
            Key::Char('^') => Some(Motion::FirstNonblank),
            Key::Char('$') | Key::End => Some(Motion::End),
            Key::Char('w') => Some(Motion::WordForward(false)),
            Key::Char('W') => Some(Motion::WordForward(true)),
            Key::Char('b') => Some(Motion::WordBackward(false)),
            Key::Char('B') => Some(Motion::WordBackward(true)),
            Key::Char('e') => Some(Motion::WordEnd(false)),
            Key::Char('E') => Some(Motion::WordEnd(true)),
            Key::Char('G') => Some(Motion::LastLine),
            Key::Char('%') => Some(Motion::MatchingPair),
            Key::Char(';') => self.last_find,
            Key::Char(',') => self.last_find.map(|m| match m {
                Motion::Find {
                    character,
                    backward,
                    till,
                } => Motion::Find {
                    character,
                    backward: !backward,
                    till,
                },
                _ => m,
            }),
            _ => None,
        };
        if let Some(motion) = motion {
            self.run_motion(motion);
            return;
        }
        if let Key::Char(ch @ ('d' | 'c' | 'y')) = key {
            let operator = match ch {
                'd' => Operator::Delete,
                'c' => Operator::Change,
                _ => Operator::Yank,
            };
            if let Some(selection) = self.selection() {
                let start = selection.chars.start;
                self.operate(selection, operator, start);
            } else if self.pending.operator == Some(operator) {
                let row = self.document.buffer.position().0;
                let selection =
                    motion::lines(&self.document.buffer, row, row + self.count().max(1) - 1);
                self.operate(selection, operator, self.document.buffer.cursor());
            } else if self.pending.operator.is_none() {
                self.pending.operator = Some(operator);
                self.pending.operator_count = self.pending.count;
                self.pending.count = 0;
            } else {
                self.pending = Pending::default();
            }
            return;
        }
        if self.pending.operator.is_some() {
            self.pending = Pending::default();
            return;
        }
        let count = self.pending.count.max(1);
        self.pending = Pending::default();
        let buffer = &mut self.document.buffer;
        match key {
            Key::Char('v' | 'V') => {
                let mode = if key == Key::Char('V') {
                    Mode::VisualLine
                } else {
                    Mode::Visual
                };
                if self.mode == mode {
                    self.escape();
                } else {
                    self.selection_anchor.get_or_insert(buffer.cursor());
                    self.mode = mode;
                }
            }
            Key::Char('o') if self.selection_anchor.is_some() => {
                let anchor = self.selection_anchor.replace(buffer.cursor()).unwrap();
                buffer.set_cursor(anchor, false);
            }
            Key::Char('x') | Key::Delete | Key::Char('s') => {
                let selection = self.selection().unwrap_or_else(|| {
                    let buffer = &mut self.document.buffer;
                    let start = buffer.cursor();
                    for _ in 0..count {
                        buffer.move_right(true);
                    }
                    let end = buffer.cursor();
                    buffer.set_cursor(start, false);
                    Selection {
                        chars: start..end,
                        linewise: false,
                    }
                });
                let operator = if key == Key::Char('s') {
                    Operator::Change
                } else {
                    Operator::Delete
                };
                self.operate(selection, operator, self.document.buffer.cursor());
            }
            Key::Char('D' | 'C' | 'Y') => {
                self.pending.count = count;
                self.pending.operator = Some(match key {
                    Key::Char('D') => Operator::Delete,
                    Key::Char('C') => Operator::Change,
                    _ => Operator::Yank,
                });
                if key == Key::Char('Y') {
                    let row = self.document.buffer.position().0;
                    let selection = motion::lines(&self.document.buffer, row, row + count - 1);
                    self.operate(selection, Operator::Yank, self.document.buffer.cursor());
                } else {
                    self.run_motion(Motion::End);
                }
            }
            Key::Char('p' | 'P') => self.put(key == Key::Char('P'), count),
            Key::Char('/' | '?') => {
                self.search_mode = self.mode;
                self.search_backward = key == Key::Char('?');
                self.mode = Mode::Search;
                self.command.clear();
            }
            Key::Char('n' | 'N') => self.find_next(key == Key::Char('N'), count),
            _ if self.selection_anchor.is_some() => {}
            Key::Ctrl('p') => {
                if let Err(error) = std::env::current_dir().and_then(|root| self.open_picker(&root))
                {
                    self.error(&error.to_string());
                }
            }
            Key::Ctrl('d') | Key::PageDown => {
                buffer.move_vertical(((self.viewport.height / 2).max(1) * count) as isize, false)
            }
            Key::Ctrl('u') | Key::PageUp => buffer.move_vertical(
                -(((self.viewport.height / 2).max(1) * count) as isize),
                false,
            ),
            Key::Char('i') => self.enter_insert(),
            Key::Char('a') => {
                buffer.move_right(true);
                self.enter_insert();
            }
            Key::Char('I') => {
                buffer.first_nonblank();
                self.enter_insert();
            }
            Key::Char('A') => {
                buffer.line_end(true);
                self.enter_insert();
            }
            Key::Char('o' | 'O') => {
                if self.has_room(2) {
                    self.document.buffer.open_line(key == Key::Char('O'));
                    self.enter_insert();
                }
            }
            Key::Char('u') => {
                for _ in 0..count {
                    if !self.document.buffer.undo() {
                        self.info("Already at oldest change");
                        break;
                    }
                }
            }
            Key::Ctrl('r') => {
                for _ in 0..count {
                    if !self.document.buffer.redo() {
                        self.info("Already at newest change");
                        break;
                    }
                }
            }
            Key::Char(':') => {
                self.mode = Mode::Command;
                self.command.clear();
            }
            _ => {}
        }
    }

    fn run_motion(&mut self, motion: Motion) {
        let origin = self.document.buffer.cursor();
        let operator = self.pending.operator;
        let count = self.count();
        // cw changes through the current word's end, preserving following space.
        if let Motion::WordForward(big) = motion
            && operator == Some(Operator::Change)
            && origin < self.document.buffer.text().len_chars()
            && !self.document.buffer.text().char(origin).is_whitespace()
            && let Some(word) = motion::text_object(
                &self.document.buffer,
                if big { 'W' } else { 'w' },
                false,
                count,
            )
        {
            self.operate(
                Selection {
                    chars: origin..word.chars.end,
                    linewise: false,
                },
                Operator::Change,
                origin,
            );
            return;
        }
        // Keep the motion value explicit; all coordinate changes live in motion.rs.
        let result = motion::apply(&mut self.document.buffer, motion, count, operator.is_some());
        if let Some(extent) = result {
            if let Some(operator) = operator {
                let selection = motion::operated(&self.document.buffer, origin, extent);
                self.document.buffer.set_cursor(origin, false);
                self.operate(selection, operator, origin);
            }
        } else {
            self.info("Motion target not found");
        }
        self.pending = Pending::default();
    }

    fn operate(&mut self, mut selection: Selection, operator: Operator, yank_cursor: usize) {
        self.pending = Pending::default();
        self.selection_anchor = None;
        self.mode = Mode::Normal;
        let buffer = &mut self.document.buffer;
        if selection.chars.is_empty() && !selection.linewise {
            if operator == Operator::Change {
                self.enter_insert();
            }
            return;
        }
        let mut text = buffer.text().slice(selection.chars.clone()).to_string();
        if selection.linewise && !text.ends_with('\n') {
            text.push_str(buffer.newline());
        }
        self.register = Register {
            text,
            linewise: selection.linewise,
        };
        if operator == Operator::Yank {
            buffer.set_cursor(yank_cursor, false);
            return;
        }
        let row = buffer.text().char_to_line(selection.chars.start);
        if operator == Operator::Delete
            && selection.linewise
            && selection.chars.end == buffer.text().len_chars()
            && selection.chars.start > 0
            && !buffer
                .text()
                .slice(selection.chars.clone())
                .to_string()
                .ends_with('\n')
        {
            selection.chars.start = buffer.grapheme_start_before(selection.chars.start);
        }
        let start = selection.chars.start;
        let replacement = if operator == Operator::Change
            && selection.linewise
            && buffer
                .text()
                .slice(selection.chars.clone())
                .to_string()
                .ends_with('\n')
        {
            buffer.newline()
        } else {
            ""
        };
        buffer.replace_range(selection.chars, replacement);
        buffer.set_cursor(start, operator == Operator::Change);
        if operator == Operator::Change {
            self.enter_insert();
        } else {
            buffer.commit();
            if selection.linewise {
                buffer.goto_line(row.min(buffer.line_count() - 1), false);
                buffer.first_nonblank();
                buffer.clamp_cursor(false);
            }
        }
    }

    fn put(&mut self, before: bool, count: usize) {
        if self.register.text.is_empty() {
            self.info("Register is empty");
            return;
        }
        if self.selection_anchor.is_some() {
            self.info("Escape Visual mode before putting text");
            return;
        }
        let newline = self.document.buffer.newline();
        let normalized = self
            .register
            .text
            .replace("\r\n", "\n")
            .replace('\n', newline);
        let Some(bytes) = normalized
            .len()
            .checked_mul(count)
            .and_then(|n| n.checked_add(2))
        else {
            self.error("Paste is too large");
            return;
        };
        if !self.has_room(bytes) {
            return;
        }
        let mut text = normalized.repeat(count);
        let buffer = &mut self.document.buffer;
        buffer.begin();
        if self.register.linewise {
            let row = buffer.position().0;
            let inserted_line = if before || buffer.text().len_chars() == 0 {
                row
            } else {
                row + 1
            };
            let mut start = buffer.text().line_to_char(row);
            if !before {
                if row + 1 < buffer.line_count() {
                    start = buffer.text().line_to_char(row + 1);
                } else {
                    start = buffer.text().len_chars();
                    if text.ends_with(newline) {
                        text.truncate(text.len() - newline.len());
                    }
                    if start > 0 {
                        text.insert_str(0, newline);
                    }
                }
            }
            buffer.set_cursor(start, true);
            buffer.insert(&text);
            buffer.goto_line(inserted_line, false);
            buffer.first_nonblank();
            buffer.clamp_cursor(false);
        } else {
            if !before {
                buffer.move_right(true);
            }
            buffer.insert(&text);
            buffer.move_left(false);
        }
        buffer.commit();
    }

    fn enter_insert(&mut self) {
        self.document.buffer.begin();
        self.mode = Mode::Insert;
        self.message.clear();
    }

    fn escape(&mut self) {
        if self.mode == Mode::Search {
            self.mode = self.search_mode;
            self.command.clear();
            return;
        }
        if self.mode == Mode::Insert {
            self.document.buffer.commit();
            self.document.buffer.move_left(false);
        }
        self.mode = Mode::Normal;
        self.pending = Pending::default();
        self.selection_anchor = None;
        self.command.clear();
    }

    fn insert_key(&mut self, key: Key) {
        match key {
            Key::Escape => self.escape(),
            Key::Char(character) if !character.is_control() => {
                self.insert_text(&character.to_string());
            }
            Key::Enter => {
                if self.has_room(2) {
                    self.document.buffer.insert_newline();
                }
            }
            Key::Tab => self.insert_text("\t"),
            Key::Backspace => self.document.buffer.backspace(),
            Key::Delete => self.document.buffer.delete(true),
            Key::Left | Key::Right | Key::Up | Key::Down | Key::Home | Key::End => {
                // Moving during Insert starts a new undo group on the next edit.
                self.document.buffer.commit();
                match key {
                    Key::Left => self.document.buffer.move_left(true),
                    Key::Right => self.document.buffer.move_right(true),
                    Key::Up => self.document.buffer.move_vertical(-1, true),
                    Key::Down => self.document.buffer.move_vertical(1, true),
                    Key::Home => self.document.buffer.line_start(),
                    Key::End => self.document.buffer.line_end(true),
                    _ => unreachable!(),
                }
            }
            _ => {}
        }
    }

    fn has_room(&mut self, bytes: usize) -> bool {
        if self
            .document
            .buffer
            .text()
            .len_bytes()
            .saturating_add(bytes) as u64
            > MAX_FILE_BYTES
        {
            self.error("Supports buffers up to 64 MiB");
            false
        } else {
            true
        }
    }

    fn insert_text(&mut self, text: &str) {
        if self.has_room(text.len()) {
            self.document.buffer.insert(text);
        }
    }

    pub fn paste(&mut self, text: &str) {
        if self.view != View::Editing || self.mode != Mode::Insert {
            self.info("Enter Insert mode before pasting");
            return;
        }
        if text.contains('\0') {
            self.error("Paste rejected: NUL bytes are not supported");
            return;
        }
        // Terminal paste commonly uses CR or CRLF; preserve the buffer's EOL style.
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        let normalized = if self.document.buffer.newline_name() == "CRLF" {
            normalized.replace('\n', "\r\n")
        } else {
            normalized
        };
        self.document.buffer.commit();
        self.insert_text(&normalized);
        self.document.buffer.commit();
    }

    fn search_key(&mut self, key: Key) {
        match key {
            Key::Escape => self.escape(),
            Key::Char(character) if !character.is_control() => self.command.push(character),
            Key::Backspace => {
                if self.command.is_empty() {
                    self.escape();
                } else {
                    pop_grapheme(&mut self.command);
                }
            }
            Key::Enter => {
                let query = std::mem::take(&mut self.command);
                self.mode = self.search_mode;
                if !query.is_empty() {
                    self.search.set(&query, self.search_backward);
                } else {
                    self.search.backward = self.search_backward;
                }
                self.find_next(false, 1);
            }
            _ => {}
        }
    }

    fn find_next(&mut self, reverse: bool, count: usize) {
        if self.search.query.is_empty() {
            self.error("No previous search");
            return;
        }
        if let Some((index, wrapped)) =
            self.search
                .navigate(&mut self.document.buffer, reverse, count)
        {
            self.info(&format!(
                "Match {}/{}{}{}",
                index + 1,
                self.search.matches.len(),
                if self.search.truncated {
                    "+ (result limit)"
                } else {
                    ""
                },
                if wrapped { " (wrapped)" } else { "" }
            ));
        } else {
            self.error("Pattern not found");
        }
    }

    fn command_key(&mut self, key: Key) {
        match key {
            Key::Escape => self.escape(),
            Key::Char(character) if !character.is_control() => self.command.push(character),
            Key::Backspace => {
                if self.command.is_empty() {
                    self.escape();
                } else {
                    pop_grapheme(&mut self.command);
                }
            }
            Key::Enter => {
                let command = std::mem::take(&mut self.command);
                self.mode = Mode::Normal;
                if let Err(error) = self.execute_command(&command) {
                    self.error(&error.to_string());
                }
            }
            _ => {}
        }
    }

    pub fn execute_command(&mut self, command: &str) -> io::Result<()> {
        let command = command.trim();
        let (name, argument) = command
            .split_once(char::is_whitespace)
            .map_or((command, ""), |(name, argument)| (name, argument.trim()));
        match name {
            "w" | "wq" => {
                let path = if argument.is_empty() {
                    None
                } else {
                    Some(Path::new(argument))
                };
                self.save_current(path)?;
                self.info("Written");
                if name == "wq" {
                    self.ensure_all_saved(false)?;
                    self.quit = true;
                }
            }
            "q" | "q!" if argument.is_empty() => {
                self.ensure_all_saved(name == "q!")?;
                self.quit = true;
            }
            "e" | "e!" => self.edit_command(argument, name == "e!")?,
            "files" => {
                let root = if argument.is_empty() {
                    std::env::current_dir()?
                } else {
                    argument.into()
                };
                self.open_picker(&root)?;
            }
            "theme" => {
                if !argument.is_empty() {
                    self.theme = ThemeName::parse(argument).ok_or_else(|| {
                        io::Error::other("Themes: tokyo-night, catppuccin, gruvbox")
                    })?;
                }
                self.info(&format!(
                    "Theme: {}. Available: tokyo-night, catppuccin, gruvbox",
                    self.theme.name()
                ));
            }
            "enew" if argument.is_empty() => self.new_buffer(),
            "bn" | "bnext" if argument.is_empty() => self.cycle_buffer(false)?,
            "bp" | "bprevious" if argument.is_empty() => self.cycle_buffer(true)?,
            "b" | "buffer" => {
                let id = argument
                    .parse()
                    .map_err(|_| io::Error::other("Use :b BUFFER_NUMBER; :ls lists buffers"))?;
                self.switch_buffer(id)?;
            }
            "bd" | "bdelete" | "bd!" | "bdelete!" if argument.is_empty() => {
                self.close_buffer(name.ends_with('!'))?
            }
            "ls" | "buffers" if argument.is_empty() => {
                let selected = self
                    .buffer_infos()
                    .iter()
                    .position(|info| info.active)
                    .unwrap();
                self.view = View::Buffers { selected };
            }
            "noh" | "nohlsearch" if argument.is_empty() => self.search.visible = false,
            "help" if argument.is_empty() => self.info(
                "hjkl  0 $ gg G  i a I A o O  x  u  Ctrl-r  :w [path]  :e[!] [path]  :q[!]  :wq",
            ),
            "" => {}
            _ => return Err(io::Error::other(format!("Unknown command: {command}"))),
        }
        Ok(())
    }

    pub(crate) fn ensure_can_discard(&self, force: bool) -> io::Result<()> {
        if self.document.buffer.is_modified() && !force {
            Err(io::Error::other(
                "Unsaved changes. Use :w to save or add ! to discard",
            ))
        } else {
            Ok(())
        }
    }

    pub(crate) fn reset_context(&mut self) {
        self.mode = Mode::Normal;
        self.pending = Pending::default();
        self.selection_anchor = None;
        self.command.clear();
        self.view = View::Editing;
        self.picker = None;
        self.search.invalidate();
        self.document.buffer.clamp_cursor(false);
    }

    pub(crate) fn info(&mut self, message: &str) {
        self.message = message.into();
        self.message_is_error = false;
    }

    pub(crate) fn error(&mut self, message: &str) {
        self.message = message.into();
        self.message_is_error = true;
    }
}
