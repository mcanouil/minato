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

use crate::actions;
use crate::compare::{Comparison, State};

pub use app::{App, Sort};

/// Which action a key asked for.
///
/// Public so the caller can supply the dispatcher that owns the clone
/// destination and the configuration, which this layer does not hold.
#[derive(Debug)]
pub enum Action {
    /// Fetch the highlighted clone.
    Fetch,
    /// Fast-forward the highlighted clone.
    Update,
    /// Clone the highlighted repository into a group, or into the root when
    /// none is named.
    Clone {
        /// The group to clone into, absent for none.
        group: Option<String>,
    },
}

/// What the browser wants doing after a key.
enum Step {
    /// Keep going.
    Continue,
    /// Leave.
    Quit,
    /// Rebuild the comparison from disk and cache.
    Reload,
    /// Run an action on the highlighted repository.
    Act(Action),
}

/// Runs the browser until the user leaves.
///
/// `load` rebuilds the comparison from disk and cache, so this module does not
/// need to know how one is produced; it is called for the first paint, for a
/// reload, and to refresh after an action. `apply` carries out an action on one
/// repository, so the clone destination and configuration stay with the caller.
///
/// # Errors
///
/// Returns an error when the terminal cannot be driven.
pub fn run(
    mut load: impl FnMut() -> Vec<Comparison>,
    mut apply: impl FnMut(&Action, &Comparison) -> actions::Summary,
) -> io::Result<()> {
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

    // Open the screen before scanning, so the terminal appears at once with a
    // notice rather than staying frozen until the first scan finishes.
    let mut app = App::new(Vec::new());
    app.set_message("Loading…");
    let _ = terminal.draw(|frame| draw(frame, &app));
    app.replace(load());
    app.clear_message();

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
                app.replace(load());
                app.set_message("Reloaded.");
            }
            Ok(Step::Act(action)) => {
                let (message, ran) = act(&app, &action, &mut apply);
                // Rebuild so the acted repository's new state shows at once,
                // then restore the result, which the rebuild would otherwise
                // have cleared.
                if ran {
                    app.replace(load());
                }
                app.set_message(message);
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

    // A status message lasts until the next key, then the footer returns to the
    // key hints.
    app.clear_message();

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

    // While choosing a clone destination, keys type the group name, apart from
    // the two that confirm or cancel.
    if app.is_cloning() {
        match key.code {
            KeyCode::Esc => app.cancel_clone(),
            KeyCode::Enter => {
                let group = app.finish_clone();
                let group = (!group.trim().is_empty()).then(|| group.trim().to_owned());
                return Ok(Step::Act(Action::Clone { group }));
            }
            KeyCode::Backspace => app.pop_group(),
            KeyCode::Char(character) => app.push_group(character),
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
        KeyCode::Char('c') => match app.current().map(|current| &current.state) {
            Some(State::RemoteOnly) => app.start_clone(),
            Some(_) => app.set_message("That repository already has a clone."),
            None => app.set_message("Nothing selected."),
        },
        KeyCode::Char('f') => return Ok(Step::Act(Action::Fetch)),
        KeyCode::Char('u') => return Ok(Step::Act(Action::Update)),
        _ => {}
    }

    Ok(Step::Continue)
}

/// Runs an action on the highlighted repository, through `apply`, which the
/// caller wires to the same functions the commands use.
///
/// Returns the result message and whether anything changed, so the caller knows
/// whether the table is worth rebuilding.
fn act(
    app: &App,
    action: &Action,
    apply: &mut impl FnMut(&Action, &Comparison) -> actions::Summary,
) -> (String, bool) {
    let Some(current) = app.current() else {
        return ("Nothing selected.".to_owned(), false);
    };

    let summary = apply(action, current);

    let Some(report) = summary.reports.first() else {
        return ("Nothing to do for that repository.".to_owned(), false);
    };

    // Only a change is worth a rebuild; a skipped or failed action left the disk
    // as it was.
    let ran = matches!(report.outcome, actions::Outcome::Done { .. });

    let message = match &report.outcome {
        actions::Outcome::Done { detail } => format!("Done: {detail}"),
        actions::Outcome::Would { detail } => format!("Would {detail}"),
        actions::Outcome::Skipped { reason } => format!("Skipped: {reason}"),
        actions::Outcome::Failed { error } => format!("Failed: {error}"),
    };

    (message, ran)
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
            " Minato ",
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

    let footer = if app.is_cloning() {
        let known = app.known_groups();
        let hint = if known.is_empty() {
            "new group".to_owned()
        } else {
            format!("existing: {}", known.join(", "))
        };
        Line::from(format!(
            "Clone into group: {}▏  Enter to clone, Esc to cancel  ({hint})",
            app.group_input()
        ))
    } else if app.is_searching() {
        Line::from(format!("/{}", app.query()))
    } else if let Some(message) = app.message() {
        Line::from(Span::styled(
            message.to_owned(),
            Style::default().fg(Color::Yellow),
        ))
    } else {
        Line::from(Span::styled(
            "j/k move  / search  s sort  c clone  f fetch  u update  r reload  q quit",
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
        State::RemoteOnly => ("not backed up".to_owned(), Style::default().fg(Color::Blue)),
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
                upstream: None,
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
                upstream: None,
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
        assert!(rendered.contains("not backed up"), "{rendered}");
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
        assert!(
            !rendered.contains("q quit"),
            "the message replaces the key hints while it is shown:\n{rendered}"
        );
    }

    #[test]
    fn clearing_the_message_brings_the_key_hints_back() {
        let mut app = App::new(rows());
        app.set_message("Done: fast-forward /code/perso/minato");

        app.clear_message();

        let rendered = screen(&app);
        assert!(rendered.contains("q quit"), "{rendered}");
        assert!(!rendered.contains("Done: fast-forward"), "{rendered}");
    }

    #[test]
    fn any_message_reverts_to_the_hints_when_cleared() {
        let mut app = App::new(rows());

        for message in ["Reloaded.", "Skipped: nope", "Nothing selected."] {
            app.set_message(message);
            assert!(screen(&app).contains(message), "{message}");

            app.clear_message();
            assert!(screen(&app).contains("q quit"), "cleared: {message}");
        }
    }

    #[test]
    fn the_footer_lists_clone_among_the_keys() {
        assert!(screen(&App::new(rows())).contains("c clone"));
    }

    #[test]
    fn choosing_a_clone_group_shows_the_prompt_and_the_existing_groups() {
        let mut app = App::new(rows());
        app.start_clone();
        for character in "arch".chars() {
            app.push_group(character);
        }

        let rendered = screen(&app);

        assert!(rendered.contains("Clone into group: arch"), "{rendered}");
        assert!(
            rendered.contains("existing: perso"),
            "the groups that already hold a clone are offered:\n{rendered}"
        );
    }

    #[test]
    fn a_done_action_reports_it_and_asks_for_a_rebuild() {
        let app = App::new(rows());
        let mut apply = |_: &Action, comparison: &Comparison| actions::Summary {
            reports: vec![actions::Report {
                id: comparison.id.clone(),
                path: None,
                outcome: actions::Outcome::Done {
                    detail: "clone github:mcanouil/minato".to_owned(),
                },
            }],
        };

        let (message, ran) = act(&app, &Action::Clone { group: None }, &mut apply);

        assert!(message.starts_with("Done:"), "{message}");
        assert!(ran, "a change should ask for a rebuild");
    }

    #[test]
    fn a_skipped_action_reports_it_without_a_rebuild() {
        let app = App::new(rows());
        let mut apply = |_: &Action, _: &Comparison| actions::Summary {
            reports: vec![actions::Report {
                id: None,
                path: None,
                outcome: actions::Outcome::Skipped {
                    reason: "no configured root to clone into".to_owned(),
                },
            }],
        };

        let (message, ran) = act(&app, &Action::Clone { group: None }, &mut apply);

        assert!(message.starts_with("Skipped:"), "{message}");
        assert!(!ran, "a skip changed nothing, so no rebuild");
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
        assert!(rendered.contains("not backed up"), "{rendered}");
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
