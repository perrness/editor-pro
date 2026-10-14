use std::{io, path::Path};

use crate::{
    file::{Document, MAX_FILE_BYTES},
    input::Key,
    text::pop_grapheme,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Command,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "NORMAL",
            Self::Insert => "INSERT",
            Self::Command => "COMMAND",
        }
    }
}

#[derive(Default)]
pub struct Viewport {
    pub top: usize,
    pub left: usize,
    pub height: usize,
}

pub struct Editor {
    pub document: Document,
    pub mode: Mode,
    pub viewport: Viewport,
    pub command: String,
    pub message: String,
    pub message_is_error: bool,
    pub quit: bool,
    pending_g: bool,
}

impl Editor {
    pub fn new(document: Document) -> Self {
        Self {
            document,
            mode: Mode::Normal,
            viewport: Viewport::default(),
            command: String::new(),
            message: "i  insert   :w  save   :q  quit   :help  keys".into(),
            message_is_error: false,
            quit: false,
            pending_g: false,
        }
    }

    pub fn handle_key(&mut self, key: Key) {
        if matches!(key, Key::Ctrl('c') | Key::Ctrl('[')) {
            self.escape();
            return;
        }
        match self.mode {
            Mode::Normal => self.normal_key(key),
            Mode::Insert => self.insert_key(key),
            Mode::Command => self.command_key(key),
        }
    }

    fn normal_key(&mut self, key: Key) {
        let pending = std::mem::take(&mut self.pending_g);
        let buffer = &mut self.document.buffer;
        match key {
            Key::Char('h') | Key::Left => buffer.move_left(false),
            Key::Char('l') | Key::Right => buffer.move_right(false),
            Key::Char('j') | Key::Down => buffer.move_vertical(1, false),
            Key::Char('k') | Key::Up => buffer.move_vertical(-1, false),
            Key::Char('0') | Key::Home => buffer.line_start(),
            Key::Char('$') | Key::End => buffer.line_end(false),
            Key::Char('g') if pending => buffer.goto_line(0, false),
            Key::Char('g') => self.pending_g = true,
            Key::Char('G') => buffer.goto_line(buffer.line_count() - 1, false),
            Key::Ctrl('d') | Key::PageDown => {
                buffer.move_vertical((self.viewport.height / 2).max(1) as isize, false);
            }
            Key::Ctrl('u') | Key::PageUp => {
                buffer.move_vertical(-((self.viewport.height / 2).max(1) as isize), false);
            }
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
            Key::Char('o') | Key::Char('O') => {
                if buffer.text().len_bytes() as u64 + 2 <= MAX_FILE_BYTES {
                    buffer.open_line(key == Key::Char('O'));
                    self.enter_insert();
                } else {
                    self.error("Milestone 1 supports buffers up to 64 MiB");
                }
            }
            Key::Char('x') | Key::Delete => {
                buffer.delete(false);
                buffer.commit();
            }
            Key::Char('u') => {
                if !buffer.undo() {
                    self.info("Already at oldest change");
                }
            }
            Key::Ctrl('r') => {
                if !buffer.redo() {
                    self.info("Already at newest change");
                }
            }
            Key::Char(':') => {
                self.mode = Mode::Command;
                self.command.clear();
            }
            Key::Escape => self.escape(),
            _ => {}
        }
    }

    fn enter_insert(&mut self) {
        self.document.buffer.begin();
        self.mode = Mode::Insert;
        self.message.clear();
    }

    fn escape(&mut self) {
        if self.mode == Mode::Insert {
            self.document.buffer.commit();
            self.document.buffer.move_left(false);
        }
        self.mode = Mode::Normal;
        self.pending_g = false;
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
        if self.document.buffer.text().len_bytes().saturating_add(bytes) as u64 > MAX_FILE_BYTES {
            self.error("Milestone 1 supports buffers up to 64 MiB");
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
        if self.mode != Mode::Insert {
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
                self.document.save(path)?;
                self.info("Written");
                if name == "wq" {
                    self.quit = true;
                }
            }
            "q" | "q!" if argument.is_empty() => {
                self.ensure_can_discard(name == "q!")?;
                self.quit = true;
            }
            "e" | "e!" => {
                self.ensure_can_discard(name == "e!")?;
                let path = if argument.is_empty() {
                    self.document
                        .path()
                        .ok_or_else(|| io::Error::other("No file name"))?
                } else {
                    Path::new(argument)
                };
                let document = Document::open(path)?;
                self.document = document;
                self.viewport = Viewport::default();
                self.pending_g = false;
                self.info("Opened");
            }
            "help" if argument.is_empty() => self.info(
                "hjkl  0 $ gg G  i a I A o O  x  u  Ctrl-r  :w [path]  :e[!] [path]  :q[!]  :wq",
            ),
            "" => {}
            _ => return Err(io::Error::other(format!("Unknown command: {command}"))),
        }
        Ok(())
    }

    fn ensure_can_discard(&self, force: bool) -> io::Result<()> {
        if self.document.buffer.is_modified() && !force {
            Err(io::Error::other(
                "Unsaved changes. Use :w to save or add ! to discard",
            ))
        } else {
            Ok(())
        }
    }

    pub fn check_external(&mut self) {
        let was_changed = self.document.external_changed;
        match self.document.check_external() {
            Ok(true) if !was_changed => self.error(
                "File changed on disk. :e! reloads; :w NEW_PATH preserves your edits",
            ),
            Err(error) => {
                self.document.external_changed = true;
                self.error(&format!("Cannot check file on disk: {error}"));
            }
            _ => {}
        }
    }

    fn info(&mut self, message: &str) {
        self.message = message.into();
        self.message_is_error = false;
    }

    fn error(&mut self, message: &str) {
        self.message = message.into();
        self.message_is_error = true;
    }
}
