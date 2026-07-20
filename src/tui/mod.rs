//! An interactive browser over the same comparison the commands use.
//!
//! This layer draws and dispatches. Every action it offers is the same function
//! the corresponding command calls, so nothing can be done here that cannot be
//! done from a script, and the two cannot drift apart.

pub mod app;

use std::io::{self, IsTerminal};

use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};

use crate::actions::{self, Mode};
use crate::compare::{Comparison, State};

pub use app::{App, Sort};

/// What the browser wants doing after a key.
enum Step {
    /// Keep going.
    Continue,
    /// Leave.
    Quit,
    /// Rebuild the comparison from disk and cache.
    Reload,
}

/// Runs the browser until the user leaves.
///
/// `reload` is called when the contents need rebuilding, so this module does
/// not need to know how a comparison is produced.
///
/// # Errors
///
/// Returns an error when the terminal cannot be driven.
pub fn run(rows: Vec<Comparison>, mut reload: impl FnMut() -> Vec<Comparison>) -> io::Result<()> {
    // Without a terminal there is nothing to draw on. Every action offered
    // here is also a command, so saying so is more useful than failing
    // obscurely part-way through setting up a screen that cannot exist.
    if !io::stdout().is_terminal() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "there is no terminal to draw on; use `minato status`, `minato fetch`, and `minato update` instead, which do everything this offers",
        ));
    }

    let mut terminal = ratatui::try_init()?;
    let mut app = App::new(rows);

    let outcome = loop {
        if let Err(error) = terminal.draw(|frame| draw(frame, &app)) {
            break Err(error);
        }

        match handle_event(&mut app) {
            Ok(Step::Quit) => break Ok(()),
            Ok(Step::Continue) => {}
            Ok(Step::Reload) => {
                app.set_message("Reloading…");
                let _ = terminal.draw(|frame| draw(frame, &app));
                app.replace(reload());
                app.set_message("Reloaded.");
            }
            Err(error) => break Err(error),
        }
    };

    ratatui::restore();

    outcome
}

/// Reads one key and applies it.
fn handle_event(app: &mut App) -> io::Result<Step> {
    let Event::Key(key) = event::read()? else {
        return Ok(Step::Continue);
    };

    if key.kind != KeyEventKind::Press {
        return Ok(Step::Continue);
    }

    // While typing a search, keys are text rather than commands, apart from the
    // two that stop typing.
    if app.is_searching() {
        match key.code {
            KeyCode::Esc => app.cancel_search(),
            KeyCode::Enter => app.finish_search(),
            KeyCode::Backspace => app.pop_query(),
            KeyCode::Char(character) => app.push_query(character),
            _ => {}
        }

        return Ok(Step::Continue);
    }

    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => return Ok(Step::Quit),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            return Ok(Step::Quit);
        }
        KeyCode::Char('j') | KeyCode::Down => app.next(),
        KeyCode::Char('k') | KeyCode::Up => app.previous(),
        KeyCode::Char('g') | KeyCode::Home => app.first(),
        KeyCode::Char('G') | KeyCode::End => app.last(),
        KeyCode::Char('/') => app.start_search(),
        KeyCode::Char('s') => app.cycle_sort(),
        KeyCode::Char('r') => return Ok(Step::Reload),
        KeyCode::Char('f') => act(app, &Action::Fetch),
        KeyCode::Char('u') => act(app, &Action::Update),
        _ => {}
    }

    Ok(Step::Continue)
}

/// Which action a key asked for.
enum Action {
    Fetch,
    Update,
}

/// Runs an action on the highlighted repository, through the same function the
/// command uses.
fn act(app: &mut App, action: &Action) {
    let Some(current) = app.current().cloned() else {
        app.set_message("Nothing selected.");
        return;
    };

    let selection = [current];

    let summary = match action {
        Action::Fetch => actions::fetch_all(&selection, Mode::Execute),
        Action::Update => actions::update_all(&selection, Mode::Execute),
    };

    let Some(report) = summary.reports.first() else {
        app.set_message("Nothing to do for that repository.");
        return;
    };

    app.set_message(match &report.outcome {
        actions::Outcome::Done { detail } => format!("Done: {detail}"),
        actions::Outcome::Would { detail } => format!("Would {detail}"),
        actions::Outcome::Skipped { reason } => format!("Skipped: {reason}"),
        actions::Outcome::Failed { error } => format!("Failed: {error}"),
    });
}

/// Draws the whole screen.
fn draw(frame: &mut Frame, app: &App) {
    let areas = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(frame.area());

    let (shown, total) = app.counts();

    let header = Line::from(vec![
        Span::styled(
            " minato ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            "  {shown}/{total} shown  sorted by {}",
            app.sort().label()
        )),
    ]);

    frame.render_widget(Paragraph::new(header), areas[0]);

    let rows: Vec<Row> = app
        .visible()
        .iter()
        .map(|comparison| {
            let (state, style) = describe_state(&comparison.state);

            Row::new(vec![
                Cell::from(
                    comparison
                        .id
                        .as_ref()
                        .map_or_else(|| "-".to_owned(), ToString::to_string),
                ),
                Cell::from(comparison.group.clone().unwrap_or_else(|| "-".to_owned())),
                Cell::from(state).style(style),
                Cell::from(notes(comparison)),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(45),
            Constraint::Percentage(15),
            Constraint::Percentage(20),
            Constraint::Percentage(20),
        ],
    )
    .header(
        Row::new(["REPOSITORY", "GROUP", "STATE", "NOTES"])
            .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
    .block(Block::default().borders(Borders::TOP));

    let mut state = TableState::default();
    state.select(Some(app.selected()));

    frame.render_stateful_widget(table, areas[1], &mut state);

    let footer = if app.is_searching() {
        Line::from(format!("/{}", app.query()))
    } else if let Some(message) = app.message() {
        Line::from(Span::styled(
            message.to_owned(),
            Style::default().fg(Color::Yellow),
        ))
    } else {
        Line::from(Span::styled(
            "j/k move  / search  s sort  f fetch  u update  r reload  q quit",
            Style::default().fg(Color::DarkGray),
        ))
    };

    frame.render_widget(Paragraph::new(footer), areas[2]);
}

/// How a state reads, and the colour it carries.
fn describe_state(state: &State) -> (String, Style) {
    let yellow = Style::default().fg(Color::Yellow);
    let plain = Style::default();

    match state {
        State::RemoteOnly => ("not cloned".to_owned(), Style::default().fg(Color::Blue)),
        State::InSync => ("in sync".to_owned(), Style::default().fg(Color::Green)),
        State::Ahead { ahead } => (format!("ahead {ahead}"), yellow),
        State::Behind { behind } => (format!("behind {behind}"), yellow),
        State::Diverged { ahead, behind } => (
            format!("diverged +{ahead}/-{behind}"),
            Style::default().fg(Color::Red),
        ),
        State::LocalOnly(_) => ("local only".to_owned(), Style::default().fg(Color::Magenta)),
        State::Incomparable(_) => ("incomparable".to_owned(), plain),
    }
}

/// The flags worth showing beside a state.
fn notes(comparison: &Comparison) -> String {
    let mut notes = Vec::new();

    if let Some(local) = comparison.local {
        if local.dirty {
            notes.push("dirty");
        }
        if local.untracked {
            notes.push("untracked");
        }
    }

    if let Some(remote) = comparison.remote {
        if remote.archived {
            notes.push("archived");
        }
        if remote.private {
            notes.push("private");
        }
        if remote.fork {
            notes.push("fork");
        }
    }

    notes.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_state_carries_both_words_and_a_colour() {
        let (text, _) = describe_state(&State::Behind { behind: 3 });
        assert_eq!(text, "behind 3");

        let (text, style) = describe_state(&State::Diverged {
            ahead: 1,
            behind: 2,
        });
        assert_eq!(text, "diverged +1/-2");
        assert_eq!(
            style.fg,
            Some(Color::Red),
            "the state that needs a decision should stand out"
        );
    }

    #[test]
    fn in_sync_is_not_coloured_like_a_problem() {
        let (_, style) = describe_state(&State::InSync);

        assert_eq!(style.fg, Some(Color::Green));
    }
}

#[cfg(test)]
mod rendering {
    use super::*;
    use crate::compare::LocalFlags;
    use crate::model::{Provider, RepoId};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::path::PathBuf;

    fn rows() -> Vec<Comparison> {
        vec![
            Comparison {
                id: Some(RepoId::new(Provider::GitHub, "mcanouil", "minato")),
                path: Some(PathBuf::from("/code/perso/minato")),
                group: Some("perso".to_owned()),
                state: State::Behind { behind: 3 },
                local: Some(LocalFlags {
                    dirty: true,
                    ..LocalFlags::default()
                }),
                remote: None,
            },
            Comparison {
                id: Some(RepoId::new(Provider::GitHub, "mcanouil", "other")),
                path: None,
                group: None,
                state: State::RemoteOnly,
                local: None,
                remote: None,
            },
        ]
    }

    /// Renders once and returns everything on screen as text.
    fn screen(app: &App) -> String {
        let mut terminal =
            Terminal::new(TestBackend::new(100, 12)).expect("a terminal to render into");

        terminal
            .draw(|frame| draw(frame, app))
            .expect("the frame to draw");

        terminal
            .backend()
            .buffer()
            .content()
            .chunks(100)
            .map(|line| {
                line.iter()
                    .map(ratatui::buffer::Cell::symbol)
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_screen_shows_the_repositories_and_their_states() {
        let rendered = screen(&App::new(rows()));

        assert!(rendered.contains("minato"), "{rendered}");
        assert!(rendered.contains("behind 3"), "{rendered}");
        assert!(rendered.contains("not cloned"), "{rendered}");
        assert!(rendered.contains("perso"), "{rendered}");
        assert!(rendered.contains("dirty"), "{rendered}");
    }

    #[test]
    fn the_footer_shows_the_keys_until_something_happens() {
        let mut app = App::new(rows());

        assert!(screen(&app).contains("q quit"));

        app.set_message("Done: fast-forward /code/perso/minato");

        let rendered = screen(&app);
        assert!(rendered.contains("Done: fast-forward"), "{rendered}");
    }

    #[test]
    fn typing_a_search_shows_it_and_narrows_the_screen() {
        let mut app = App::new(rows());
        app.start_search();
        for character in "other".chars() {
            app.push_query(character);
        }

        let rendered = screen(&app);

        assert!(rendered.contains("/other"), "{rendered}");
        assert!(rendered.contains("not cloned"), "{rendered}");
        assert!(
            !rendered.contains("behind 3"),
            "the row that does not match should be gone:\n{rendered}"
        );
    }

    #[test]
    fn the_header_counts_what_is_shown_against_what_exists() {
        let mut app = App::new(rows());
        assert!(screen(&app).contains("2/2 shown"));

        app.push_query('o');
        app.push_query('t');

        assert!(screen(&app).contains("1/2 shown"));
    }

    #[test]
    fn an_empty_list_still_renders_rather_than_panicking() {
        let rendered = screen(&App::new(Vec::new()));

        assert!(rendered.contains("0/0 shown"), "{rendered}");
    }
}
