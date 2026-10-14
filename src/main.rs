use std::{
    env,
    io::{self, IsTerminal, stdout},
    path::PathBuf,
    process::ExitCode,
    time::{Duration, Instant},
};

use editor_pro::{
    editor::{Editor, Mode},
    file::Document,
    input,
    theme::Theme,
    ui,
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

const HELP: &str = "editor-pro [FILE]

A minimal terminal code editor. FILE may be a new path.

Normal: hjkl / arrows, 0 $, gg G, Ctrl-u/d, i a I A o O, x, u, Ctrl-r
Insert: type, Enter, Tab, Backspace, Delete; Escape returns to Normal
Commands: :w [path], :q, :q!, :wq [path], :e[!] [path], :help

Only one UTF-8 buffer is open at a time. Use -- before a filename starting with -.";

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
        Some("--") => args.next().map(PathBuf::from),
        Some(flag) if flag.starts_with('-') => {
            return Err(io::Error::other(format!(
                "Unknown option {flag}; use --help"
            )));
        }
        _ => first.map(PathBuf::from),
    };
    if args.next().is_some() {
        return Err(io::Error::other("Milestone 1 opens one file at a time"));
    }
    let document = path
        .as_deref()
        .map_or_else(|| Ok(Document::default()), Document::open)?;
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::other(
            "Run in an interactive terminal (stdin and stdout must be TTYs)",
        ));
    }
    let mut editor = Editor::new(document);
    let theme = Theme::night();
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
    let result = event_loop(&mut terminal, &mut editor, &theme);
    drop(terminal);
    let restored = restore_terminal();
    // Drop is a fallback for early returns; restoration is deliberately idempotent.
    drop(session);
    result.and(restored)
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    editor: &mut Editor,
    theme: &Theme,
) -> io::Result<()> {
    let mut last_check = Instant::now();
    let mut previous_mode = None;
    let mut redraw = true;
    while !editor.quit {
        if redraw {
            if previous_mode != Some(editor.mode) {
                let shape = if editor.mode == Mode::Normal {
                    SetCursorStyle::SteadyBlock
                } else {
                    SetCursorStyle::SteadyBar
                };
                execute!(stdout(), shape)?;
                previous_mode = Some(editor.mode);
            }
            terminal.draw(|frame| ui::render(frame, editor, theme))?;
            redraw = false;
        }
        if event::poll(Duration::from_millis(200))? {
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
                    redraw = true;
                }
                _ => {}
            }
        }
        if last_check.elapsed() >= Duration::from_secs(2) {
            let before = editor.document.external_changed;
            editor.check_external();
            redraw |= before != editor.document.external_changed;
            last_check = Instant::now();
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
