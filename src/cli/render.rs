//! Turning results into something to read.
//!
//! Two renderings of the same data: a table for a person, and JSON for a script
//! or an agent. Neither is derived from the other, so neither can quietly
//! become the "real" one.

use std::fmt::Write as _;

use jiff::SignedDuration;

/// A table that sizes its columns to their contents.
#[derive(Debug, Default)]
pub struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
}

impl Table {
    /// Starts a table with the given column headings.
    #[must_use]
    pub fn new(headers: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            headers: headers.into_iter().map(Into::into).collect(),
            rows: Vec::new(),
        }
    }

    /// Adds a row. A row with too few cells is padded.
    pub fn push(&mut self, row: impl IntoIterator<Item = impl Into<String>>) {
        self.rows.push(row.into_iter().map(Into::into).collect());
    }

    /// Whether anything was added.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

impl std::fmt::Display for Table {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let widths: Vec<usize> = self
            .headers
            .iter()
            .enumerate()
            .map(|(column, header)| {
                self.rows
                    .iter()
                    .filter_map(|row| row.get(column))
                    .map(|cell| cell.chars().count())
                    .chain(std::iter::once(header.chars().count()))
                    .max()
                    .unwrap_or_default()
            })
            .collect();

        let mut out = String::new();

        write_row(&mut out, &self.headers, &widths);

        for row in &self.rows {
            write_row(&mut out, row, &widths);
        }

        f.write_str(&out)
    }
}

fn write_row(out: &mut String, cells: &[String], widths: &[usize]) {
    let last = widths.len().saturating_sub(1);

    for (column, width) in widths.iter().enumerate() {
        let cell = cells.get(column).map_or("", String::as_str);

        // The final column is not padded, so lines carry no trailing spaces.
        if column == last {
            let _ = writeln!(out, "{cell}");
        } else {
            let padding = width.saturating_sub(cell.chars().count());
            let _ = write!(out, "{cell}{:padding$}  ", "", padding = padding);
        }
    }
}

/// Describes a duration the way a person would say it.
#[must_use]
pub fn describe_age(age: SignedDuration) -> String {
    let seconds = age.as_secs().max(0);

    match seconds {
        0..=59 => "just now".to_owned(),
        60..=3599 => plural(seconds / 60, "minute"),
        3600..=86_399 => plural(seconds / 3600, "hour"),
        _ => plural(seconds / 86_400, "day"),
    }
}

fn plural(count: i64, unit: &str) -> String {
    if count == 1 {
        format!("1 {unit} ago")
    } else {
        format!("{count} {unit}s ago")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_column_starts_at_the_same_offset_on_every_line() {
        let mut table = Table::new(["ID", "STATE"]);
        table.push(["a-very-long-identifier", "behind"]);
        table.push(["short", "in sync"]);

        let rendered = table.to_string();

        let starts: Vec<usize> = rendered
            .lines()
            .map(|line| {
                let gap = line.find(' ').unwrap_or(line.len());
                let rest = &line[gap..];

                gap + (rest.len() - rest.trim_start().len())
            })
            .collect();

        assert!(
            starts.windows(2).all(|pair| pair[0] == pair[1]),
            "the second column should begin at one offset throughout:\n{rendered}"
        );
    }

    #[test]
    fn no_line_carries_trailing_whitespace() {
        let mut table = Table::new(["ID", "STATE"]);
        table.push(["one", "behind"]);
        table.push(["a-much-longer-one", "ok"]);

        for line in table.to_string().lines() {
            assert_eq!(line, line.trim_end(), "trailing whitespace in `{line}`");
        }
    }

    #[test]
    fn a_short_row_does_not_break_the_layout() {
        let mut table = Table::new(["A", "B", "C"]);
        table.push(["only-one"]);

        assert_eq!(table.to_string().lines().count(), 2);
    }

    #[test]
    fn ages_read_the_way_a_person_would_say_them() {
        let cases = [
            (0, "just now"),
            (59, "just now"),
            (60, "1 minute ago"),
            (600, "10 minutes ago"),
            (3600, "1 hour ago"),
            (7200, "2 hours ago"),
            (86_400, "1 day ago"),
            (172_800, "2 days ago"),
        ];

        for (seconds, expected) in cases {
            assert_eq!(
                describe_age(SignedDuration::from_secs(seconds)),
                expected,
                "{seconds} seconds"
            );
        }
    }

    #[test]
    fn a_clock_that_went_backwards_does_not_produce_nonsense() {
        assert_eq!(describe_age(SignedDuration::from_secs(-30)), "just now");
    }
}
