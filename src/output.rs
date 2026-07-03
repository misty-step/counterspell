use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::model::{StatusOutput, StatusRow, StatusSummary, WatchRow, WatchStore};
use crate::util::{human_age, unix_to_utc};

pub(crate) fn print_status(rows: &[StatusRow]) {
    println!("recent sessions");

    let headers = [
        "SESSION", "PROJECT", "CWD", "PANE", "AGENT", "STATE", "WATCH", "TARGET", "MODEL", "DRIFT",
        "UPDATED",
    ];
    let widths = status_widths(rows, &headers);
    print_row(&headers, &widths);
    print_row(
        &widths
            .iter()
            .map(|width| "-".repeat(*width))
            .collect::<Vec<_>>(),
        &widths,
    );

    for row in rows {
        print_row(
            &[
                row.session_id.as_str(),
                row.project.as_str(),
                row.cwd.as_str(),
                row.pane.as_str(),
                row.agent.as_str(),
                row.state.as_str(),
                row.watch.as_str(),
                row.target.as_str(),
                row.model.as_str(),
                row.drift.as_str(),
                row.updated.as_str(),
            ],
            &widths,
        );
    }
}

pub(crate) fn print_status_json(
    rows: &[StatusRow],
    store: &WatchStore,
    now: DateTime<Utc>,
) -> Result<()> {
    let output = StatusOutput {
        summary: status_summary(rows, store, now),
        rows,
    };
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

pub(crate) fn status_summary(
    rows: &[StatusRow],
    store: &WatchStore,
    now: DateTime<Utc>,
) -> StatusSummary {
    let last_trigger_unix = store
        .sessions
        .values()
        .filter_map(|session| session.last_action_unix)
        .max();
    StatusSummary {
        total: rows.len(),
        watched: rows.iter().filter(|row| row.watch == "watched").count(),
        ignored: rows.iter().filter(|row| row.watch == "ignored").count(),
        mapped: rows
            .iter()
            .filter(|row| row.pane != "not-open" && !row.session_id.starts_with("pane:"))
            .count(),
        live_panes: rows
            .iter()
            .filter(|row| row.project == "herdr-live-pane")
            .count(),
        last_trigger_event: last_trigger_unix
            .and_then(unix_to_utc)
            .map(|timestamp| human_age(timestamp, now)),
        last_trigger_unix,
    }
}

pub(crate) fn print_watch(rows: &[WatchRow]) {
    println!("watch pass");

    let headers = ["SESSION", "PANE", "MODEL", "TARGET", "GATE", "ACTIONS"];
    let widths = watch_widths(rows, &headers);
    print_row(&headers, &widths);
    print_row(
        &widths
            .iter()
            .map(|width| "-".repeat(*width))
            .collect::<Vec<_>>(),
        &widths,
    );

    for row in rows {
        print_row(
            &[
                row.session_id.as_str(),
                row.pane.as_str(),
                row.model.as_str(),
                row.target.as_str(),
                row.gate.as_str(),
                row.actions.as_str(),
            ],
            &widths,
        );
    }
}

fn status_widths(rows: &[StatusRow], headers: &[&str; 11]) -> [usize; 11] {
    let mut widths = headers.map(str::len);
    for row in rows {
        let cells = [
            row.session_id.as_str(),
            row.project.as_str(),
            row.cwd.as_str(),
            row.pane.as_str(),
            row.agent.as_str(),
            row.state.as_str(),
            row.watch.as_str(),
            row.target.as_str(),
            row.model.as_str(),
            row.drift.as_str(),
            row.updated.as_str(),
        ];
        widen(&mut widths, &cells);
    }
    widths
}

fn watch_widths(rows: &[WatchRow], headers: &[&str; 6]) -> [usize; 6] {
    let mut widths = headers.map(str::len);
    for row in rows {
        let cells = [
            row.session_id.as_str(),
            row.pane.as_str(),
            row.model.as_str(),
            row.target.as_str(),
            row.gate.as_str(),
            row.actions.as_str(),
        ];
        widen(&mut widths, &cells);
    }
    widths
}

fn widen(widths: &mut [usize], cells: &[&str]) {
    for (index, cell) in cells.iter().enumerate() {
        widths[index] = widths[index].max(cell.len());
    }
}

fn print_row<T: AsRef<str>>(cells: &[T], widths: &[usize]) {
    for (index, cell) in cells.iter().enumerate() {
        if index > 0 {
            print!("  ");
        }
        print!("{:<width$}", cell.as_ref(), width = widths[index]);
    }
    println!();
}
