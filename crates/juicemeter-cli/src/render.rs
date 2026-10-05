use chrono::{DateTime, Local, Utc};
use juicemeter_core::config::Show;
use juicemeter_core::pace::{self, State};
use juicemeter_core::{Account, Outcome, ProviderReport, Report, Snapshot, Source, Window};
use std::collections::BTreeMap;

struct Style {
    color: bool,
    show: Show,
}

impl Style {
    fn paint(&self, code: &str, s: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
    fn dim(&self, s: &str) -> String {
        self.paint("2", s)
    }
    fn bold(&self, s: &str) -> String {
        self.paint("1", s)
    }
}

/// `Studio-Mac.local` -> `studio-mac`
pub fn short_host(host: &str) -> String {
    host.trim_end_matches(".local").to_lowercase()
}

/// One entry per account; `multi` adds which machines hold it.
pub fn merged(entries: &[juicemeter_core::merge::Merged], multi: bool, color: bool, show: Show) {
    let st = Style { color, show };
    let now = Utc::now();
    let mut entries: Vec<_> = entries.iter().collect();
    entries.sort_by_key(|m| pace::report(&m.report, now)); // stable: provider order within a state
    for (i, m) in entries.into_iter().enumerate() {
        if i > 0 {
            println!();
        }
        provider(&st, &m.report, now);
        if multi && !m.seen_on.is_empty() {
            let mut hosts: Vec<String> = m.seen_on.iter().map(|(h, _)| short_host(h)).collect();
            hosts.dedup();
            println!("  {}", st.dim(&format!("on {}", hosts.join(", "))));
        }
    }
}

pub fn by_host(reports: &[Report], color: bool, show: Show) {
    let st = Style { color, show };
    let now = Utc::now();
    for (i, r) in reports.iter().enumerate() {
        if i > 0 {
            println!();
        }
        println!("{}", st.paint("1;4", &short_host(&r.host)));
        for (j, p) in r.providers.iter().enumerate() {
            if j > 0 {
                println!();
            }
            provider(&st, p, now);
        }
    }
}

fn provider(st: &Style, p: &ProviderReport, now: DateTime<Utc>) {
    let who = p.account.as_ref().map(Account::display).unwrap_or(&p.source);
    match &p.outcome {
        Outcome::Ok { snapshot } => snapshot_block(st, p, who, snapshot, now),
        Outcome::NotConfigured { hint } => {
            println!("{}  {}", st.bold(&p.name), st.dim("not configured"));
            println!("  {}", st.dim(hint));
        }
        Outcome::Error { message } => {
            println!("{} · {who}  {}", st.bold(&p.name), st.paint("31", "error"));
            println!("  {message}");
        }
    }
}

fn snapshot_block(st: &Style, p: &ProviderReport, who: &str, s: &Snapshot, now: DateTime<Utc>) {
    let plan = s.plan.as_deref().map(|p| format!(" ({p})")).unwrap_or_default();
    let source = match s.source {
        Source::Live => format!("live · {}", s.as_of.with_timezone(&Local).format("%H:%M")),
        Source::LocalLog => format!("from log · {} ago", human(now - s.as_of)),
    };
    let badge = match pace::report(p, now) {
        State::RunningOut => format!("{}  ", st.paint("1;31", "▼ SQUEEZED DRY")),
        State::Unused => format!("{}  ", st.paint("1;33", "▲ DRINK UP")),
        State::Ok => String::new(),
    };
    println!("{badge}{}{} · {who}  {}", st.bold(&p.name), plan, st.dim(&source));
    for w in &s.windows {
        println!("  {}", window_line(st, w, now));
    }
    for b in &s.balances {
        println!("  {:<16} {}", b.label, b.display());
    }
    for p in s.perks.iter().filter(|p| p.count > 0) {
        let when = p.expires_at.map(|e| format!(" · next expires in {}", human(e - now))).unwrap_or_default();
        let soon = p.expires_at.is_some_and(|e| e - now < juicemeter_core::pace::PERK_SOON);
        let text = format!("{} available{when}", p.count);
        println!("  {:<16} {}", p.label, if soon { st.paint("1;33", &text) } else { text });
    }
    if let Some(n) = &s.note {
        println!("  {}", st.dim(n));
    }
}

fn window_line(st: &Style, w: &Window, now: DateTime<Utc>) -> String {
    const WIDTH: usize = 20;
    let reset_passed = w.resets_at.is_some_and(|r| r <= now);
    let pace = pace::window(w, now);
    let used = w.used_percent.clamp(0.0, 100.0);
    let left = 100.0 - used;
    // The bar fills with what's shown; the tick marks the matching share of time, so
    // fill past the tick means ahead of the clock.
    let (fill, time, pct) = match st.show {
        Show::Remaining => (left, pace.elapsed.map(|e| 1.0 - e), format!("{left:>3.0}% left")),
        Show::Used => (used, pace.elapsed, format!("{used:>3.0}% used")),
    };
    let filled = ((fill / 100.0) * WIDTH as f64).round() as usize;
    let mut cells: Vec<&str> = (0..WIDTH).map(|i| if i < filled { "█" } else { "░" }).collect();
    if let Some(t) = time {
        let i = ((t * WIDTH as f64).round() as usize).min(WIDTH - 1);
        cells[i] = "│";
    }
    let bar: String = cells.concat();
    let code = match pace.state {
        State::RunningOut if left < 10.0 => "31",
        State::RunningOut => "33",
        State::Unused => "33",
        State::Ok => "32",
    };
    let resets = w.resets_at.map(|r| format!("refills in {}", human(r - now)));
    let status = match (pace.state, reset_passed) {
        (_, true) => "window has reset since".to_string(),
        (State::RunningOut, _) => match pace.runs_out_at {
            Some(t) => format!("squeezed dry in ~{} · {}", human(t - now), resets.unwrap_or_default()),
            None => resets.unwrap_or_default(),
        },
        (State::Unused, _) => {
            format!("~{:.0}% goes to waste · {}", pace.projected_left.unwrap_or(left), resets.unwrap_or_default())
        }
        (State::Ok, _) => resets.unwrap_or_default(),
    };
    let bar = if reset_passed { st.dim(&bar) } else { st.paint(code, &bar) };
    format!("{:<16} {} {pct}  {}", w.label, bar, st.dim(&status))
}

fn human(d: chrono::TimeDelta) -> String {
    let m = d.num_minutes().max(0);
    match (m / 1440, (m % 1440) / 60, m % 60) {
        (0, 0, m) => format!("{m}m"),
        (0, h, m) => format!("{h}h{m:02}m"),
        (d, h, _) => format!("{d}d{h}h"),
    }
}

struct Row {
    account: Account,
    name: String,
    plan: Option<String>,
    seen: Vec<String>,
}

/// One row per account, merged across machines.
pub fn accounts(reports: &[Report], color: bool) {
    let st = Style { color, show: Show::default() };
    let mut rows: BTreeMap<String, Row> = BTreeMap::new();
    let mut unidentified = Vec::new();
    for r in reports {
        let host = short_host(&r.host);
        for p in &r.providers {
            let Some(a) = &p.account else {
                if !matches!(p.outcome, Outcome::NotConfigured { .. }) {
                    unidentified.push(format!("{} on {host} ({})", p.name, p.source));
                }
                continue;
            };
            let plan = match &p.outcome {
                Outcome::Ok { snapshot } => snapshot.plan.clone(),
                _ => None,
            };
            let row = rows.entry(a.id.clone()).or_insert_with(|| Row {
                account: a.clone(),
                name: p.name.clone(),
                plan: None,
                seen: vec![],
            });
            // Prefer whichever copy carries the most detail.
            row.account.email = row.account.email.take().or(a.email.clone());
            row.account.label = row.account.label.take().or(a.label.clone());
            row.plan = row.plan.take().or(plan);
            row.seen.push(format!("{host} {}", st.dim(&format!("({})", p.source))));
        }
    }
    if rows.is_empty() {
        println!("no accounts found");
    } else {
        let mut sorted: Vec<_> = rows.into_values().collect();
        sorted.sort_by(|a, b| (&a.name, a.account.display()).cmp(&(&b.name, b.account.display())));
        let width = |f: &dyn Fn(&Row) -> usize, header: &str| sorted.iter().map(f).max().unwrap_or(0).max(header.len());
        let wa = width(&|r| r.account.display().chars().count(), "ACCOUNT");
        let wn = width(&|r| r.name.len(), "PROVIDER");
        let wp = width(&|r| r.plan.as_deref().map_or(1, str::len), "PLAN");
        let wi = width(&|r| r.account.id.len(), "ID");
        println!(
            "{}",
            st.dim(&format!("{:<wa$}  {:<wn$}  {:<wp$}  {:<wi$}  SEEN ON", "ACCOUNT", "PROVIDER", "PLAN", "ID"))
        );
        for r in sorted {
            println!(
                "{:<wa$}  {:<wn$}  {:<wp$}  {:<wi$}  {}",
                r.account.display(),
                r.name,
                r.plan.as_deref().unwrap_or("-"),
                r.account.id,
                r.seen.join(", ")
            );
        }
    }
    for u in unidentified {
        println!("{}", st.dim(&format!("could not identify: {u}")));
    }
}
