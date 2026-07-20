//! What the interactive browser is showing and pointing at.
//!
//! All of it is plain data with no terminal involved, so every behaviour the
//! interface offers is reachable in a test. The drawing layer reads this and
//! renders it; it decides nothing.

use crate::compare::{Comparison, State};

/// How the list is ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    /// By repository name.
    #[default]
    Name,
    /// By state, so what needs attention rises to the top.
    State,
    /// By group, keeping a category together.
    Group,
}

impl Sort {
    /// The next ordering, cycling round.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Name => Self::State,
            Self::State => Self::Group,
            Self::Group => Self::Name,
        }
    }

    /// How to describe this ordering in the interface.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::State => "state",
            Self::Group => "group",
        }
    }
}

/// How urgent a state is, so sorting by state puts work first.
const fn urgency(state: &State) -> u8 {
    match state {
        State::Diverged { .. } => 0,
        State::Behind { .. } => 1,
        State::Ahead { .. } => 2,
        State::Incomparable(_) => 3,
        State::LocalOnly(_) => 4,
        State::RemoteOnly => 5,
        State::InSync => 6,
    }
}

/// What the browser is showing.
#[derive(Debug, Default)]
pub struct App {
    rows: Vec<Comparison>,
    visible: Vec<usize>,
    selected: usize,
    query: String,
    sort: Sort,
    message: Option<String>,
    searching: bool,
}

impl App {
    /// Builds a browser over these comparisons.
    #[must_use]
    pub fn new(rows: Vec<Comparison>) -> Self {
        let mut app = Self {
            rows,
            ..Self::default()
        };
        app.refilter();

        app
    }

    /// The comparisons currently shown, in order.
    #[must_use]
    pub fn visible(&self) -> Vec<&Comparison> {
        self.visible
            .iter()
            .filter_map(|index| self.rows.get(*index))
            .collect()
    }

    /// How many are shown out of how many exist.
    #[must_use]
    pub fn counts(&self) -> (usize, usize) {
        (self.visible.len(), self.rows.len())
    }

    /// Which row is highlighted.
    #[must_use]
    pub const fn selected(&self) -> usize {
        self.selected
    }

    /// The highlighted comparison, if anything is shown.
    #[must_use]
    pub fn current(&self) -> Option<&Comparison> {
        self.visible
            .get(self.selected)
            .and_then(|index| self.rows.get(*index))
    }

    /// The current search text.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Whether the search line is being typed into.
    #[must_use]
    pub const fn is_searching(&self) -> bool {
        self.searching
    }

    /// The current ordering.
    #[must_use]
    pub const fn sort(&self) -> Sort {
        self.sort
    }

    /// The last thing that happened, if anything.
    #[must_use]
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// Reports what just happened, shown until something else does.
    pub fn set_message(&mut self, message: impl Into<String>) {
        self.message = Some(message.into());
    }

    /// Moves the highlight down, stopping at the end rather than wrapping.
    ///
    /// Wrapping would make a long list feel like it had lost your place.
    pub fn next(&mut self) {
        if !self.visible.is_empty() {
            self.selected = (self.selected + 1).min(self.visible.len() - 1);
        }
    }

    /// Moves the highlight up, stopping at the start.
    pub const fn previous(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Jumps to the first row.
    pub const fn first(&mut self) {
        self.selected = 0;
    }

    /// Jumps to the last row.
    pub fn last(&mut self) {
        self.selected = self.visible.len().saturating_sub(1);
    }

    /// Starts typing a search.
    pub fn start_search(&mut self) {
        self.searching = true;
    }

    /// Stops typing, keeping what was typed.
    pub const fn finish_search(&mut self) {
        self.searching = false;
    }

    /// Stops typing and clears the search.
    pub fn cancel_search(&mut self) {
        self.searching = false;
        self.query.clear();
        self.refilter();
    }

    /// Adds a character to the search.
    pub fn push_query(&mut self, character: char) {
        self.query.push(character);
        self.refilter();
    }

    /// Removes the last character from the search.
    pub fn pop_query(&mut self) {
        self.query.pop();
        self.refilter();
    }

    /// Cycles to the next ordering.
    pub fn cycle_sort(&mut self) {
        self.sort = self.sort.next();
        self.refilter();
    }

    /// Replaces the contents, keeping the search and ordering.
    pub fn replace(&mut self, rows: Vec<Comparison>) {
        self.rows = rows;
        self.refilter();
    }

    /// Recomputes what is shown, and keeps the highlight in range.
    fn refilter(&mut self) {
        let query = self.query.to_lowercase();

        let mut visible: Vec<usize> = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| matches(row, &query))
            .map(|(index, _)| index)
            .collect();

        visible.sort_by(|left, right| {
            let (left, right) = (&self.rows[*left], &self.rows[*right]);

            match self.sort {
                Sort::Name => name_of(left).cmp(&name_of(right)),
                Sort::State => urgency(&left.state)
                    .cmp(&urgency(&right.state))
                    .then_with(|| name_of(left).cmp(&name_of(right))),
                Sort::Group => left
                    .group
                    .cmp(&right.group)
                    .then_with(|| name_of(left).cmp(&name_of(right))),
            }
        });

        self.visible = visible;
        self.selected = self.selected.min(self.visible.len().saturating_sub(1));
    }
}

/// The text a row is identified by, for sorting and searching.
fn name_of(comparison: &Comparison) -> String {
    comparison.id.as_ref().map_or_else(
        || {
            comparison
                .path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default()
        },
        ToString::to_string,
    )
}

/// Whether a row matches the search, which looks at identity, group, and path.
fn matches(comparison: &Comparison, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }

    let haystack = format!(
        "{} {} {}",
        name_of(comparison),
        comparison.group.clone().unwrap_or_default(),
        comparison
            .path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default()
    )
    .to_lowercase();

    haystack.contains(query)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Provider, RepoId};
    use std::path::PathBuf;

    fn row(name: &str, group: Option<&str>, state: State) -> Comparison {
        Comparison {
            id: Some(RepoId::new(Provider::GitHub, "mcanouil", name)),
            path: Some(PathBuf::from(format!(
                "/code/{}/{name}",
                group.unwrap_or("loose")
            ))),
            group: group.map(ToOwned::to_owned),
            state,
            local: None,
            remote: None,
        }
    }

    fn sample() -> App {
        App::new(vec![
            row("zebra", Some("perso"), State::InSync),
            row("alpha", Some("demo"), State::Behind { behind: 2 }),
            row("middle", Some("perso"), State::RemoteOnly),
        ])
    }

    #[test]
    fn everything_is_shown_before_anything_is_typed() {
        assert_eq!(sample().counts(), (3, 3));
    }

    #[test]
    fn rows_are_ordered_by_name_to_begin_with() {
        let names: Vec<_> = sample()
            .visible()
            .iter()
            .map(|row| row.id.as_ref().expect("an id").name.clone())
            .collect();

        assert_eq!(names, ["alpha", "middle", "zebra"]);
    }

    #[test]
    fn sorting_by_state_puts_what_needs_attention_first() {
        let mut app = sample();
        app.cycle_sort();

        assert_eq!(app.sort(), Sort::State);
        assert_eq!(
            app.visible()[0].id.as_ref().expect("an id").name,
            "alpha",
            "the repository that is behind should come first"
        );
        assert_eq!(
            app.visible()[2].id.as_ref().expect("an id").name,
            "zebra",
            "what is in sync should come last"
        );
    }

    #[test]
    fn sorting_cycles_back_round() {
        let mut app = sample();

        app.cycle_sort();
        app.cycle_sort();
        assert_eq!(app.sort(), Sort::Group);

        app.cycle_sort();
        assert_eq!(app.sort(), Sort::Name);
    }

    #[test]
    fn searching_narrows_to_what_matches() {
        let mut app = sample();

        for character in "alph".chars() {
            app.push_query(character);
        }

        assert_eq!(app.counts(), (1, 3));
        assert_eq!(app.visible()[0].id.as_ref().expect("an id").name, "alpha");
    }

    #[test]
    fn searching_looks_at_the_group_as_well_as_the_name() {
        let mut app = sample();

        for character in "demo".chars() {
            app.push_query(character);
        }

        assert_eq!(app.counts(), (1, 3));
    }

    #[test]
    fn searching_ignores_case() {
        let mut app = sample();

        for character in "ALPHA".chars() {
            app.push_query(character);
        }

        assert_eq!(app.counts(), (1, 3));
    }

    #[test]
    fn removing_a_character_widens_the_search_again() {
        let mut app = sample();

        for character in "alpha".chars() {
            app.push_query(character);
        }
        assert_eq!(app.counts().0, 1);

        for _ in 0..5 {
            app.pop_query();
        }

        assert_eq!(app.counts(), (3, 3));
    }

    #[test]
    fn cancelling_a_search_restores_everything() {
        let mut app = sample();
        app.start_search();
        app.push_query('z');
        assert_eq!(app.counts().0, 1);

        app.cancel_search();

        assert_eq!(app.counts(), (3, 3));
        assert!(!app.is_searching());
        assert_eq!(app.query(), "");
    }

    #[test]
    fn the_highlight_stops_at_the_ends_rather_than_wrapping() {
        let mut app = sample();

        app.previous();
        assert_eq!(app.selected(), 0, "moving up from the top stays put");

        for _ in 0..10 {
            app.next();
        }

        assert_eq!(app.selected(), 2, "moving down past the end stays put");
    }

    #[test]
    fn the_highlight_never_points_past_the_end_after_a_search() {
        let mut app = sample();
        app.last();
        assert_eq!(app.selected(), 2);

        for character in "alpha".chars() {
            app.push_query(character);
        }

        assert_eq!(
            app.selected(),
            0,
            "narrowing to one row must not leave the highlight beyond it"
        );
        assert!(app.current().is_some());
    }

    #[test]
    fn nothing_is_current_when_nothing_matches() {
        let mut app = sample();

        for character in "no-such-thing".chars() {
            app.push_query(character);
        }

        assert_eq!(app.counts().0, 0);
        assert!(app.current().is_none(), "there is nothing to act on");
    }

    #[test]
    fn moving_within_an_empty_list_is_harmless() {
        let mut app = App::new(Vec::new());

        app.next();
        app.previous();
        app.last();

        assert!(app.current().is_none());
    }

    #[test]
    fn replacing_the_contents_keeps_the_search_and_ordering() {
        let mut app = sample();
        app.cycle_sort();
        for character in "perso".chars() {
            app.push_query(character);
        }

        app.replace(vec![
            row("alpha", Some("perso"), State::InSync),
            row("beta", Some("demo"), State::InSync),
        ]);

        assert_eq!(app.sort(), Sort::State, "the ordering should survive");
        assert_eq!(
            app.counts(),
            (1, 2),
            "the search should still be applied to the new contents"
        );
    }

    #[test]
    fn the_current_row_is_the_highlighted_one() {
        let mut app = sample();
        app.next();

        assert_eq!(
            app.current()
                .expect("a row")
                .id
                .as_ref()
                .expect("an id")
                .name,
            "middle"
        );
    }
}
