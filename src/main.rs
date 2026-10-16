use std::{
    env,
    io::{self, IsTerminal, stdout},
    path::PathBuf,
    process::ExitCode,
    time::Duration,
};

use editor_pro::{
    config,
    editor::{Editor, Mode, View},
    file::Document,
    input, ui,
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    crossterm::{
        cursor::{SetCursorStyle, Show},
        event::{
            self, DisableBracketedPaste, DisableFocusChange, EnableBracketedPaste,
            EnableFocusChange, Event,
        },
        execute,
        terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
    },
};

const HELP: &str = "editor-pro [FILE ...] | editor-pro DIRECTORY

A terminal-native, Vim-inspired UTF-8 editor. Files may be new paths.
The ep symlink accepts the same arguments. A directory opens its file picker
and becomes the working directory for this session (for example, ep .).

Options: -h/--help, -V/--version, --config-path
Settings: optional config.json in the user configuration directory; --config-path
prints its location. Updates are managed by Homebrew or cargo install.

Normal: hjkl / arrows, w b e W B E, 0 ^ $, gg G, f/F/t/T, ; , %, counts
Editing: i a I A o O, v/V, d/c/y + motion or object, x, p/P, u, Ctrl-r
Objects: iw/aw, iW/aW, quotes, parentheses, brackets, braces (e.g. ciw, di{)
Search: / or ?, Enter, n/N; Escape cancels prompts and selections
Files: Ctrl-p or :files [directory]; :e path, :enew, :bn/:bp, :b ID, :ls
Commands: :w [path], :q[!], :wq [path], :e[!] [path], :bd[!], :noh
Disk: :refresh compares buffer/disk; c compare again, r reload, Esc cancel; y confirms discarding edits
Help: F1 or :help [QUERY]; type to search, arrows scroll, Tab category, Esc returns
Git: :changes, :diff HEAD|index|REV|staged, :gitbase REV, :gitrefresh, :gnext/:gprev
Git keys (Normal): ]c/[c hunks, ]f/[f changed files, Space g d diff, Space g c changes
Diff: ]/[ hunks, Tab paired/inline, Enter source, Esc origin, n/p files, r refresh
Snapshots: :review snapshot, :review full, :review since [ID], :review snapshots
Review: :review help, :review file|hunk reviewed|attention|unreviewed, :notes\nNotes: :note file|hunk|old|new; :note save or :note cancel; :review export PATH\nThemes: :theme tokyo-night|catppuccin|gruvbox

LSP: :LspInfo, :Doctor, :LspInstall [LANGUAGE], :LspInstallCancel, :LspRestart
Navigation: :Hover, :Definition, :References, :Diagnostics
Code: :Outline, :WorkspaceSymbols QUERY, :Calls (i/o calls, Backspace parent)
Project: :grep TEXT, :grepdir PATH, :recent, Ctrl-o/:back, Ctrl-i/:forward
AI: :ai help; :ai context file, :ai ask QUESTION, :ai send; Visual Ctrl-g previews selection
Language: suggestions while typing; Tab/Enter accepts, arrows select, Ctrl-e dismisses
          :Complete (Insert Ctrl-n), :Signature, :Rename NAME, :CodeAction, :Format, :Symbols

Escape leaves Insert. :q checks all buffers; :q! discards all unsaved edits.
Use -- before a filename starting with -. See README.md for the full reference.";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("editor-pro: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> io::Result<()> {
    let mut args = env::args_os().skip(1);
    let first = args.next();
    let path = match first.as_deref().and_then(|arg| arg.to_str()) {
        Some("-h" | "--help") => {
            println!("{HELP}");
            return Ok(());
        }
        Some("-V" | "--version") => {
            println!("editor-pro {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some("--config-path") => {
            let path = config::config_path().ok_or_else(|| {
                io::Error::other("Cannot locate the user configuration directory; set HOME")
            })?;
            println!("{}", path.display());
            return Ok(());
        }
        Some("--") => args.next().map(PathBuf::from),
        Some(flag) if flag.starts_with('-') => {
            return Err(io::Error::other(format!(
                "Unknown option {flag}; use --help"
            )));
        }
        _ => first.map(PathBuf::from),
    };
    let additional: Vec<_> = args.map(PathBuf::from).collect();
    let directory = path.as_deref().filter(|path| path.is_dir());
    if directory.is_some() && !additional.is_empty() {
        return Err(io::Error::other(
            "Pass one directory or one or more files; directories cannot be mixed with files",
        ));
    }
    let settings = config::config_path()
        .as_deref()
        .map(config::load)
        .transpose()?
        .unwrap_or_default();
    let directory = directory.map(std::fs::canonicalize).transpose()?;
    let document = path
        .as_deref()
        .filter(|_| directory.is_none())
        .map_or_else(|| Ok(Document::default()), Document::open)?;
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::other(
            "Run in an interactive terminal (stdin and stdout must be TTYs)",
        ));
    }
    if let Some(directory) = &directory {
        env::set_current_dir(directory)?;
    }
    let mut editor = Editor::new(document);
    editor.theme = settings.theme;
    editor.settings = settings.editor;
    editor.lsp.config.installation.directory = config::lsp_directory();
    editor.lsp.config.installation.auto_install = settings.lsp.auto_install;
    for path in additional {
        editor.open_buffer(&path)?;
    }
    editor.switch_buffer(1)?;
    if let Some(directory) = &directory {
        editor.open_picker(directory)?;
    }
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore_terminal();
        previous_hook(info);
    }));
    // Construct before setup: partial initialization is restored on any error.
    let session = TerminalSession;
    enable_raw_mode()?;
    execute!(
        stdout(),
        EnterAlternateScreen,
        EnableBracketedPaste,
        EnableFocusChange
    )?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    terminal.clear()?;
    let preparation = if settings.lsp.warmup && (path.is_none() || directory.is_some()) {
        env::current_dir().ok()
    } else {
        None
    };
    let result = event_loop(&mut terminal, &mut editor, preparation);
    drop(terminal);
    let restored = restore_terminal();
    // Drop is a fallback for early returns; restoration is deliberately idempotent.
    drop(session);
    result.and(restored)
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    editor: &mut Editor,
    mut preparation: Option<PathBuf>,
) -> io::Result<()> {
    let mut previous_mode = None;
    let mut redraw = true;
    while !editor.quit {
        redraw |= editor.tick();
        if redraw {
            if previous_mode != Some(editor.mode) {
                let shape = if matches!(editor.mode, Mode::Normal | Mode::Visual | Mode::VisualLine)
                {
                    SetCursorStyle::SteadyBlock
                } else {
                    SetCursorStyle::SteadyBar
                };
                execute!(stdout(), shape)?;
                previous_mode = Some(editor.mode);
            }
            let theme = editor.theme.palette();
            terminal.draw(|frame| ui::render(frame, editor, &theme))?;
            redraw = false;
            // Directory inspection starts only after the first frame is visible.
            if let Some(directory) = preparation.take() {
                if let Err(error) = editor.prepare_lsp(Some(directory)) {
                    editor.message = format!("Cannot prepare language servers: {error}");
                    editor.message_is_error = true;
                }
                redraw = true;
            }
        }
        let poll_ms = if editor.help.is_some() {
            200
        } else if editor.view == View::Files
            || editor.git.loading
            || editor.review.busy
            || editor.lsp.pending
            || editor.lsp.completion_scheduled()
            || editor.lsp.installations.values().any(|state| state.busy)
            || editor.project_search.pending
            || editor.ai.pending
            || (editor.disk.view.is_some() && editor.disk.busy)
        {
            16
        } else {
            200
        };
        if event::poll(Duration::from_millis(poll_ms))? {
            match event::read()? {
                Event::Key(key) => {
                    if let Some(key) = input::decode(key) {
                        editor.handle_key(key);
                        redraw = true;
                    }
                }
                Event::Paste(text) => {
                    editor.paste(&text);
                    redraw = true;
                }
                Event::Resize(_, _) => redraw = true,
                Event::FocusGained => {
                    editor.check_external();
                    editor.refresh_git();
                    redraw = true;
                }
                _ => {}
            }
        }
    }
    Ok(())
}

struct TerminalSession;

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = restore_terminal();
    }
}

fn restore_terminal() -> io::Result<()> {
    // Attempt both even if one fails.
    let screen = execute!(
        stdout(),
        SetCursorStyle::DefaultUserShape,
        Show,
        DisableBracketedPaste,
        DisableFocusChange,
        LeaveAlternateScreen
    );
    let raw = disable_raw_mode();
    screen.and(raw)
}
