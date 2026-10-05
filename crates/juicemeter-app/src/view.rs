//! What the panel and the menu bar show, derived from merged reports.

use chrono::{DateTime, Utc};
use juicemeter_core::config::{BarNames, BarValue, Config, Mode, Show};
use juicemeter_core::merge::Merged;
use juicemeter_core::pace::{self, State};
use juicemeter_core::{Balance, Outcome, Perk};
use serde::Serialize;
use std::collections::HashSet;

/// A session window joins the menu bar at this much left, and leaves once it's back above
/// `SESSION_HIDE`, so it doesn't flicker around one value.
const SESSION_SHOW: f64 = 10.0;
const SESSION_HIDE: f64 = 20.0;
/// A free reset expiring this soon is mentioned in the menu bar.
const PERK_BAR: chrono::Duration = chrono::Duration::hours(3);

#[derive(Debug, Clone, Serialize)]
pub struct View {
    pub show: Show,
    pub mode: Mode,
    pub bar_value: BarValue,
    pub bar_names: BarNames,
    pub updated_at: DateTime<Utc>,
    /// Other machines from the config, for the settings page.
    pub hosts: Vec<String>,
    pub machines: Vec<Machine>,
    pub entries: Vec<Entry>,
    /// Accounts whose session window is shown in the menu bar (see `sticky_sessions`).
    #[serde(skip)]
    pub sessions: HashSet<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Machine {
    pub name: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    /// Account id, or `provider@source` when the account is unknown.
    pub key: String,
    pub provider: String,
    pub name: String,
    pub who: String,
    /// The user's name for the account, if they gave one.
    pub label: Option<String>,
    pub plan: Option<String>,
    pub state: State,
    pub pinned: bool,
    /// `ok`, `error` or `not_configured`.
    pub status: &'static str,
    pub message: Option<String>,
    pub from_log: bool,
    pub as_of: Option<DateTime<Utc>>,
    pub machines: Vec<String>,
    pub windows: Vec<WindowView>,
    pub balances: Vec<Balance>,
    pub perks: Vec<Perk>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WindowView {
    pub label: String,
    pub used: f64,
    pub left: f64,
    pub resets_at: Option<DateTime<Utc>>,
    pub elapsed: Option<f64>,
    pub state: State,
    pub runs_out_at: Option<DateTime<Utc>>,
    pub projected_left: Option<f64>,
    /// Long windows (a day or more) are the ones "use it" applies to.
    pub long: bool,
    /// Limited to one model (e.g. Claude's "Weekly · Fable"), next to a general limit.
    pub scoped: bool,
}

pub fn short_host(host: &str) -> String {
    host.trim_end_matches(".local").to_lowercase()
}

/// Accounts the menu bar shows when nothing needs attention: the configured pins, else
/// (never configured) the first Claude account, else the first entry with limits.
fn pinned_keys(entries: &[Entry], pins: Option<&[String]>) -> Vec<String> {
    match pins {
        Some(p) => p.iter().filter(|k| entries.iter().any(|e| &e.key == *k)).cloned().collect(),
        None => entries
            .iter()
            .find(|e| e.provider == "claude" && e.status == "ok")
            .or_else(|| entries.iter().find(|e| !e.windows.is_empty()))
            .map(|e| vec![e.key.clone()])
            .unwrap_or_default(),
    }
}

pub fn build(merged: &[Merged], machines: Vec<Machine>, config: &Config, now: DateTime<Utc>) -> View {
    let pins = config.pinned();
    let mut entries: Vec<Entry> = merged
        .iter()
        .map(|m| {
            let r = &m.report;
            let key =
                r.account.as_ref().map(|a| a.id.clone()).unwrap_or_else(|| format!("{}@{}", r.provider, r.source));
            let who = r.account.as_ref().map(|a| a.display().to_string()).unwrap_or_else(|| r.source.clone());
            let mut hosts: Vec<String> = m.seen_on.iter().map(|(h, _)| short_host(h)).collect();
            hosts.dedup();
            let base = Entry {
                key,
                provider: r.provider.clone(),
                name: r.name.clone(),
                who,
                label: r.account.as_ref().and_then(|a| a.label.clone()),
                plan: None,
                state: pace::report(r, now),
                pinned: false,
                status: "ok",
                message: None,
                from_log: false,
                as_of: None,
                machines: hosts,
                windows: vec![],
                balances: vec![],
                perks: vec![],
            };
            match &r.outcome {
                Outcome::Ok { snapshot: s } => Entry {
                    plan: s.plan.clone(),
                    message: s.note.clone(),
                    from_log: s.source == juicemeter_core::Source::LocalLog,
                    as_of: Some(s.as_of),
                    windows: s
                        .windows
                        .iter()
                        .map(|w| {
                            let p = pace::window(w, now);
                            let used = w.used_percent.clamp(0.0, 100.0);
                            WindowView {
                                label: w.label.clone(),
                                used,
                                left: 100.0 - used,
                                resets_at: w.resets_at,
                                elapsed: p.elapsed,
                                state: p.state,
                                runs_out_at: p.runs_out_at,
                                projected_left: p.projected_left,
                                long: w.window_seconds.is_some_and(|s| s >= 86400),
                                scoped: w.id.contains("scoped"),
                            }
                        })
                        .collect(),
                    balances: s.balances.clone(),
                    perks: s.perks.clone(),
                    ..base
                },
                Outcome::Error { message } => Entry { status: "error", message: Some(message.clone()), ..base },
                Outcome::NotConfigured { hint } => {
                    Entry { status: "not_configured", message: Some(hint.clone()), ..base }
                }
            }
        })
        .collect();
    let keys = pinned_keys(&entries, pins.as_deref());
    entries.iter_mut().for_each(|e| e.pinned = keys.contains(&e.key));
    // Urgent first, then the pinned account, then everything else; not-configured last.
    entries.sort_by_key(|e| (e.status == "not_configured", e.state, !e.pinned));
    View {
        show: config.show,
        mode: config.mode,
        bar_value: config.bar_value,
        bar_names: config.bar_names,
        updated_at: now,
        hosts: config.hosts.clone(),
        machines,
        entries,
        sessions: HashSet::new(),
    }
}

/// What the menu bar shows: optional text and how full the battery icon is.
#[derive(Debug, Clone, PartialEq)]
pub struct Headline {
    pub title: Option<String>,
    pub fill: f64,
}

/// A name short enough for the menu bar: the user's label, else the provider's name.
fn short_name(e: &Entry) -> String {
    match &e.label {
        Some(l) if l.chars().count() <= 14 => l.clone(),
        _ => e.name.clone(),
    }
}

/// `Claude` → `Cl`, `Codex-A` → `Co-A`, `Work ChatGPT` → `Wo-C`; four letters or fewer stay.
fn abbreviate(name: &str) -> String {
    if name.chars().count() <= 4 {
        return name.to_string();
    }
    let mut words = name.split(['-', ' ', '_']).filter(|w| !w.is_empty());
    let first: String = words.next().unwrap_or(name).chars().take(2).collect();
    match words.next_back().and_then(|w| w.chars().next()) {
        Some(c) => format!("{first}-{c}"),
        None => first,
    }
}

/// The name as configured: full, or abbreviated. Two accounts never share a short name:
/// ones that would are numbered, in a stable order.
fn bar_name(v: &View, e: &Entry) -> String {
    let name = short_name(e);
    if v.bar_names == BarNames::Full {
        return name;
    }
    let short = abbreviate(&name);
    let mut same: Vec<&str> =
        v.entries.iter().filter(|o| abbreviate(&short_name(o)) == short).map(|o| o.key.as_str()).collect();
    if same.len() < 2 {
        return short;
    }
    same.sort();
    let n = same.iter().position(|k| *k == e.key).unwrap_or(0) + 1;
    format!("{short}{n}")
}

/// `▲79% ↻4d`, `▲79%` or `▲↻4d`, as configured; the flag sits on the value it's about.
/// Without a time, the percentage.
fn amount(v: &View, mark: &str, left: f64, when: Option<DateTime<Utc>>, now: DateTime<Utc>) -> String {
    let p = format!("{mark}{}", pct(left, v.show));
    match (v.bar_value, when.map(|t| compact(t - now))) {
        (BarValue::Both, Some(t)) => format!("{p} ↻{t}"),
        (BarValue::Time, Some(t)) => format!("{mark}↻{t}"),
        _ => p,
    }
}

/// Short, rounded countdown: `3d` from two days up, then `37h`, then `40m`.
pub fn compact(d: chrono::TimeDelta) -> String {
    let secs = d.num_seconds().max(0) as f64;
    let (h, d) = (secs / 3600.0, secs / 86400.0);
    if d >= 2.0 {
        format!("{:.0}d", d)
    } else if h >= 1.0 {
        format!("{:.0}h", h)
    } else {
        format!("{:.0}m", secs / 60.0)
    }
}

fn pct(left: f64, show: Show) -> String {
    match show {
        Show::Remaining => format!("{left:.0}%"),
        Show::Used => format!("{:.0}%", 100.0 - left),
    }
}

/// The windows the menu bar may show for an entry: its general weekly limits. Per-model
/// limits only when there's no general one, and sessions only when there's nothing longer.
fn bar_windows(e: &Entry, now: DateTime<Utc>) -> Vec<&WindowView> {
    let live: Vec<&WindowView> = e.windows.iter().filter(|w| live(w, now)).collect();
    let general: Vec<&WindowView> = live.iter().copied().filter(|w| w.long && !w.scoped).collect();
    if !general.is_empty() {
        return general;
    }
    let long: Vec<&WindowView> = live.iter().copied().filter(|w| w.long).collect();
    if long.is_empty() {
        live
    } else {
        long
    }
}

/// The window the menu bar shows for an entry: of `bar_windows`, the one with the least left.
fn tightest(e: &Entry, now: DateTime<Utc>) -> Option<&WindowView> {
    bar_windows(e, now).into_iter().min_by(|a, b| a.left.total_cmp(&b.left))
}

/// The session (short) window of an entry with the least left.
fn session(e: &Entry, now: DateTime<Utc>) -> Option<&WindowView> {
    e.windows.iter().filter(|w| !w.long && live(w, now)).min_by(|a, b| a.left.total_cmp(&b.left))
}

/// Accounts whose session should show next to their weekly value: at `SESSION_SHOW` left or
/// less, and, once shown (`before`), until it's back above `SESSION_HIDE`.
pub fn sticky_sessions(v: &View, before: &HashSet<String>, now: DateTime<Utc>) -> HashSet<String> {
    v.entries
        .iter()
        .filter(|e| e.windows.iter().any(|w| w.long))
        .filter(|e| {
            session(e, now)
                .is_some_and(|w| w.left <= SESSION_SHOW || (before.contains(&e.key) && w.left <= SESSION_HIDE))
        })
        .map(|e| e.key.clone())
        .collect()
}

/// One account's piece of the menu bar text.
struct Part {
    key: String,
    text: String,
    fill: f64,
}

/// `Codex-A 89% ↻5d/5h 4% ↻53m, reset exp3h`: the weekly value, then the session when it
/// is shown, then a free reset about to expire.
fn part(v: &View, e: &Entry, w: &WindowView, mark: &str, when: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Part {
    let mut text = format!("{} {}", bar_name(v, e), amount(v, mark, w.left, when, now));
    let mut fill = w.left;
    if w.long && v.sessions.contains(&e.key) {
        if let Some(s) = session(e, now) {
            let empty = if s.left < 1.0 { "▼" } else { "" };
            text += &format!("/5h {}", amount(v, empty, s.left, s.resets_at, now));
            fill = fill.min(s.left);
        }
    }
    let soon = e.perks.iter().filter(|p| p.count > 0).filter_map(|p| p.expires_at).filter(|t| *t > now).min();
    if let Some(t) = soon.filter(|t| *t - now <= PERK_BAR) {
        text += &format!(", reset exp{}", compact(t - now));
    }
    Part { key: e.key.clone(), text, fill: fill / 100.0 }
}

/// A flagged window: ▼ running out (with whichever comes first, empty or refill), ▲ going to waste.
fn flagged(v: &View, e: &Entry, w: &WindowView, now: DateTime<Utc>) -> Part {
    let (mark, when) = match w.state {
        State::RunningOut => ("▼", [w.runs_out_at, w.resets_at].into_iter().flatten().min()),
        _ => ("▲", w.resets_at),
    };
    part(v, e, w, mark, when, now)
}

/// `Claude 70% ↻2d`: how much is left and when that window refills.
fn plain(v: &View, e: &Entry, w: &WindowView, now: DateTime<Utc>) -> Part {
    part(v, e, w, "", w.resets_at, now)
}

fn live(w: &WindowView, now: DateTime<Utc>) -> bool {
    w.resets_at.is_none_or(|r| r > now)
}

/// The most urgent limit running out anywhere.
fn running_out(v: &View, now: DateTime<Utc>) -> Option<Part> {
    let (e, w) = v
        .entries
        .iter()
        .flat_map(|e| bar_windows(e, now).into_iter().map(move |w| (e, w)))
        .filter(|(_, w)| w.long && w.state == State::RunningOut)
        .min_by(|a, b| a.1.left.total_cmp(&b.1.left))?;
    Some(flagged(v, e, w, now))
}

/// The biggest allowance about to go to waste anywhere.
fn unused(v: &View, now: DateTime<Utc>) -> Option<Part> {
    let (e, w) = v
        .entries
        .iter()
        .flat_map(|e| bar_windows(e, now).into_iter().map(move |w| (e, w)))
        .filter(|(_, w)| w.state == State::Unused)
        .max_by(|a, b| a.1.projected_left.unwrap_or(0.0).total_cmp(&b.1.projected_left.unwrap_or(0.0)))?;
    Some(flagged(v, e, w, now))
}

/// Every pinned account, flagged ones marked.
fn pinned(v: &View, now: DateTime<Utc>) -> Vec<Part> {
    v.entries
        .iter()
        .filter(|e| e.pinned)
        .filter_map(|e| {
            let flag = |s: State| {
                bar_windows(e, now)
                    .into_iter()
                    .filter(|w| w.long && w.state == s)
                    .min_by(|a, b| a.left.total_cmp(&b.left))
            };
            if let Some(w) = flag(State::RunningOut).or_else(|| flag(State::Unused)) {
                return Some(flagged(v, e, w, now));
            }
            if let Some(w) = tightest(e, now) {
                return Some(plain(v, e, w, now));
            }
            let b = e.balances.first()?;
            Some(Part { key: e.key.clone(), text: format!("{} {}", bar_name(v, e), b.display()), fill: 1.0 })
        })
        .collect()
}

/// An alert about an unpinned account goes in front of the pinned ones.
fn compose(alert: Option<Part>, pins: Vec<Part>) -> Option<Headline> {
    let mut parts: Vec<Part> = alert.into_iter().filter(|a| !pins.iter().any(|p| p.key == a.key)).collect();
    parts.extend(pins);
    if parts.is_empty() {
        return None;
    }
    let fill = parts.iter().map(|p| p.fill).fold(1.0, f64::min);
    Some(Headline { title: Some(parts.iter().map(|p| p.text.as_str()).collect::<Vec<_>>().join(" · ")), fill })
}

/// No text; the icon shows the emptiest limit.
fn quiet(v: &View, now: DateTime<Utc>) -> Headline {
    Headline { title: None, fill: lowest(v, now).map_or(1.0, |h| h.fill) }
}

fn lowest(v: &View, now: DateTime<Utc>) -> Option<Headline> {
    let (e, w) = v
        .entries
        .iter()
        .filter_map(|e| tightest(e, now).map(|w| (e, w)))
        .min_by(|a, b| a.1.left.total_cmp(&b.1.left))?;
    let mark = if w.state == State::RunningOut { "▼" } else { "" };
    let p = part(v, e, w, mark, w.resets_at, now);
    Some(Headline { title: Some(p.text), fill: p.fill })
}

pub fn headline(v: &View, now: DateTime<Utc>) -> Headline {
    let watch = || compose(running_out(v, now).or_else(|| unused(v, now)), pinned(v, now));
    match v.mode {
        Mode::Watch => watch().unwrap_or_else(|| quiet(v, now)),
        Mode::Lowest => lowest(v, now).unwrap_or_else(|| quiet(v, now)),
        Mode::UseIt => compose(unused(v, now), pinned(v, now)).unwrap_or_else(|| quiet(v, now)),
        Mode::Minimal => Headline { title: None, ..watch().unwrap_or_else(|| quiet(v, now)) },
        Mode::Everything => {
            let parts: Vec<String> = v
                .entries
                .iter()
                .filter_map(|e| {
                    let abbr: String = short_name(e).chars().take(2).collect();
                    match (tightest(e, now), e.balances.first()) {
                        (Some(w), _) => Some(format!("{abbr} {}", pct(w.left, v.show).trim_end_matches('%'))),
                        (None, Some(b)) => Some(format!("{abbr} {}", b.display())),
                        _ => None,
                    }
                })
                .collect();
            let fill = lowest(v, now).map_or(1.0, |h| h.fill);
            Headline { title: (!parts.is_empty()).then(|| parts.join(" · ")), fill }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn win(label: &str, left: f64, state: State, long: bool, resets_in: Duration) -> WindowView {
        WindowView {
            label: label.into(),
            used: 100.0 - left,
            left,
            resets_at: Some(Utc::now() + resets_in),
            elapsed: Some(0.5),
            state,
            runs_out_at: None,
            projected_left: Some(left - 10.0),
            long,
            scoped: label.contains('·'),
        }
    }

    fn entry(key: &str, provider: &str, who: &str, windows: Vec<WindowView>) -> Entry {
        let state = windows.iter().map(|w| w.state).min().unwrap_or(State::Ok);
        Entry {
            key: key.into(),
            provider: provider.into(),
            name: provider[..1].to_uppercase() + &provider[1..],
            who: who.into(),
            label: (!who.contains('@')).then(|| who.to_string()),
            plan: None,
            state,
            pinned: false,
            status: "ok",
            message: None,
            from_log: false,
            as_of: None,
            machines: vec![],
            windows,
            balances: vec![],
            perks: vec![],
        }
    }

    fn view(mode: Mode, mut entries: Vec<Entry>, pins: Option<&[&str]>) -> View {
        let pins: Option<Vec<String>> = pins.map(|p| p.iter().map(|s| s.to_string()).collect());
        let keys = pinned_keys(&entries, pins.as_deref());
        entries.iter_mut().for_each(|e| e.pinned = keys.contains(&e.key));
        View {
            show: Show::Remaining,
            mode,
            bar_value: BarValue::Both,
            bar_names: BarNames::Full,
            updated_at: Utc::now(),
            hosts: vec![],
            machines: vec![],
            entries,
            sessions: HashSet::new(),
        }
    }

    fn calm() -> Vec<Entry> {
        vec![
            entry("claude:a", "claude", "me@x.com", vec![win("Weekly", 80.0, State::Ok, true, Duration::days(3))]),
            entry("codex:b", "codex", "Work", vec![win("Weekly", 60.0, State::Ok, true, Duration::days(3))]),
        ]
    }

    #[test]
    fn watch_shows_pinned_claude_when_calm() {
        let h = headline(&view(Mode::Watch, calm(), None), Utc::now());
        assert_eq!(h.title.as_deref(), Some("Claude 80% ↻3d"));
    }

    #[test]
    fn running_out_takes_over_watch() {
        let mut e = calm();
        e[1].windows.push(win("Weekly", 8.0, State::RunningOut, true, Duration::minutes(40)));
        let h = headline(&view(Mode::Watch, e, None), Utc::now());
        assert!(h.title.unwrap().starts_with("Work ▼8% ↻40m · "));
    }

    #[test]
    fn unused_takes_over_when_nothing_runs_out() {
        let mut e = calm();
        e[0].windows[0].state = State::Unused;
        let h = headline(&view(Mode::Watch, e, None), Utc::now());
        assert!(h.title.unwrap().starts_with("Claude ▲80% ↻"));
    }

    #[test]
    fn configured_pin_wins_and_lowest_finds_minimum() {
        let h = headline(&view(Mode::Watch, calm(), Some(&["codex:b"])), Utc::now());
        assert_eq!(h.title.as_deref(), Some("Work 60% ↻3d"));
        let h = headline(&view(Mode::Lowest, calm(), None), Utc::now());
        assert_eq!(h.title.as_deref(), Some("Work 60% ↻3d"));
    }

    #[test]
    fn alerts_join_pins_instead_of_replacing_them() {
        let mut e = calm();
        e[1].windows.push(win("Weekly", 8.0, State::RunningOut, true, Duration::minutes(40)));
        // Unpinned Codex alert goes in front of the pinned Claude.
        let t = headline(&view(Mode::Watch, e.clone(), Some(&["claude:a"])), Utc::now()).title.unwrap();
        assert!(t.starts_with("Work ▼8% ↻40m · ") && t.ends_with(" · Claude 80% ↻3d"), "{t}");
        // A pinned account that is flagged is marked in place, not repeated.
        let t = headline(&view(Mode::Watch, e, Some(&["claude:a", "codex:b"])), Utc::now()).title.unwrap();
        assert!(t.starts_with("Claude 80% ↻3d · Work ▼8% ↻40m") && t.matches("Work").count() == 1, "{t}");
    }

    #[test]
    fn several_pins_side_by_side_and_none_is_quiet() {
        let h = headline(&view(Mode::Watch, calm(), Some(&["claude:a", "codex:b"])), Utc::now());
        assert_eq!((h.title.as_deref(), h.fill), (Some("Claude 80% ↻3d · Work 60% ↻3d"), 0.6));
        let h = headline(&view(Mode::Watch, calm(), Some(&[])), Utc::now());
        assert_eq!((h.title, h.fill), (None, 0.6));
    }

    #[test]
    fn key_hints_and_emails_are_not_used_as_names() {
        let mut e = entry("deepseek:k", "deepseek", "sk-…a1b2", vec![]);
        e.label = None;
        assert_eq!(short_name(&e), "Deepseek");
        let e = entry("claude:a", "claude", "me@x.com", vec![]);
        assert_eq!(short_name(&e), "Claude");
    }

    #[test]
    fn session_joins_the_weekly_value_only_when_low_and_without_flicker() {
        let now = Utc::now();
        let with_session = |left: f64| {
            let mut e = calm();
            e[1].windows.push(win("5-hour", left, State::RunningOut, false, Duration::minutes(53)));
            view(Mode::Watch, e, Some(&["claude:a", "codex:b"]))
        };
        let shown = |left: f64, before: &HashSet<String>| {
            let mut v = with_session(left);
            v.sessions = sticky_sessions(&v, before, now);
            headline(&v, now).title.unwrap()
        };
        let none = HashSet::new();
        let was: HashSet<String> = ["codex:b".to_string()].into();
        // Healthy session: weekly only. At 4% it joins; empty, it's flagged.
        assert_eq!(shown(50.0, &none), "Claude 80% ↻3d · Work 60% ↻3d");
        assert_eq!(shown(4.0, &none), "Claude 80% ↻3d · Work 60% ↻3d/5h 4% ↻53m");
        assert!(shown(0.0, &none).ends_with("/5h ▼0% ↻53m"));
        // Once shown it stays up to 20%, then leaves; it doesn't join at 15%.
        assert!(shown(15.0, &was).contains("/5h 15%"));
        assert!(!shown(15.0, &none).contains("/5h"));
        assert!(!shown(25.0, &was).contains("/5h"));
    }

    #[test]
    fn per_model_limits_never_drive_the_menu_bar() {
        // Claude: general weekly 50% left, Fable 88% left and going to waste.
        let mut e = calm();
        e[0].windows = vec![
            win("Weekly", 50.0, State::Ok, true, Duration::days(3)),
            win("Weekly · Fable", 88.0, State::Unused, true, Duration::days(3)),
        ];
        let t = headline(&view(Mode::Watch, e.clone(), Some(&["claude:a"])), Utc::now()).title.unwrap();
        assert_eq!(t, "Claude 50% ↻3d");
        // Even when the per-model one is lower.
        e[0].windows[1].left = 5.0;
        e[0].windows[1].state = State::RunningOut;
        let t = headline(&view(Mode::Watch, e, Some(&["claude:a"])), Utc::now()).title.unwrap();
        assert_eq!(t, "Claude 50% ↻3d");
    }

    #[test]
    fn free_reset_expiring_soon_is_mentioned() {
        let mut e = calm();
        let soon = Utc::now() + Duration::minutes(170);
        e[1].perks = vec![Perk { label: "Free resets".into(), count: 3, expires_at: Some(soon) }];
        let t = headline(&view(Mode::Watch, e.clone(), Some(&["codex:b"])), Utc::now()).title.unwrap();
        assert_eq!(t, "Work 60% ↻3d, reset exp3h");
        e[1].perks[0].expires_at = Some(Utc::now() + Duration::hours(20));
        let t = headline(&view(Mode::Watch, e, Some(&["codex:b"])), Utc::now()).title.unwrap();
        assert_eq!(t, "Work 60% ↻3d");
    }

    #[test]
    fn short_names_never_collide() {
        let abbr = (abbreviate("Claude"), abbreviate("Codex-A"), abbreviate("Work ChatGPT"));
        assert_eq!(abbr, ("Cl".into(), "Co-A".into(), "Wo-C".into()));
        let mut e = calm();
        e.push(entry("codex:c", "codex", "Codex-E", vec![win("Weekly", 41.0, State::Ok, true, Duration::days(5))]));
        e[1].label = Some("Codex-A".into());
        let mut v = view(Mode::Watch, e, Some(&["claude:a", "codex:b", "codex:c"]));
        v.bar_names = BarNames::Short;
        assert_eq!(headline(&v, Utc::now()).title.as_deref(), Some("Cl 80% ↻3d · Co-A 60% ↻3d · Co-E 41% ↻5d"));
        // Unlabelled twins are numbered.
        v.entries.iter_mut().filter(|e| e.provider == "codex").for_each(|e| e.label = None);
        let t = headline(&v, Utc::now()).title.unwrap();
        assert!(t.contains("Co1 60%") && t.contains("Co2 41%"), "{t}");
    }

    #[test]
    fn menu_bar_value_and_name_options() {
        let mut v = view(Mode::Watch, calm(), Some(&["claude:a", "codex:b"]));
        v.bar_value = BarValue::Percent;
        assert_eq!(headline(&v, Utc::now()).title.as_deref(), Some("Claude 80% · Work 60%"));
        v.bar_value = BarValue::Time;
        v.bar_names = BarNames::Short;
        assert_eq!(headline(&v, Utc::now()).title.as_deref(), Some("Cl ↻3d · Work ↻3d"), "4-letter labels stay");
    }

    #[test]
    fn countdowns_round_to_the_nearest_unit() {
        let c = |m: i64| compact(Duration::minutes(m));
        assert_eq!((c(3 * 1440 - 1), c(37 * 60), c(90), c(40)), ("3d".into(), "37h".into(), "2h".into(), "40m".into()));
    }

    #[test]
    fn minimal_has_no_text_and_everything_lists_all() {
        let h = headline(&view(Mode::Minimal, calm(), None), Utc::now());
        assert_eq!((h.title, h.fill), (None, 0.8));
        let h = headline(&view(Mode::Everything, calm(), None), Utc::now());
        assert_eq!(h.title.as_deref(), Some("Cl 80 · Wo 60"));
    }
}
