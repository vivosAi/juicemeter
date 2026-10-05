//! Is a limit about to run out, or going to waste? Compares how much of a window is used
//! with how much of its time has passed.

use crate::model::{Outcome, ProviderReport, Window};
use chrono::{DateTime, Utc};
use serde::Serialize;

/// Before this much of a window has passed, usage says little about the rest of it.
const MIN_ELAPSED: f64 = 0.15;
/// Below this much left, a window is running low regardless of pace.
const LOW_LEFT: f64 = 20.0;
/// Pace-based "runs out" only once this much is used, so early bursts don't alarm.
const MIN_USED_FOR_PROJECTION: f64 = 50.0;
/// "Use it" only from this point in the window...
const UNUSED_FROM_ELAPSED: f64 = 0.5;
/// ...and when at least this much would be left unused at the current pace.
const UNUSED_LEFTOVER: f64 = 40.0;
/// Only windows at least this long can go "unused"; idle 5-hour sessions are normal.
const UNUSED_MIN_WINDOW_SECS: u64 = 86400;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Ordered by urgency.
    RunningOut,
    Unused,
    Ok,
}

#[derive(Debug, Clone, Serialize)]
pub struct Pace {
    pub state: State,
    /// Share of the window's time that has passed, 0..1.
    pub elapsed: Option<f64>,
    /// Used share relative to elapsed share; 1.0 = on pace.
    pub ratio: Option<f64>,
    /// Percent that would be left at reset if usage continues at this rate.
    pub projected_left: Option<f64>,
    /// When it hits 100% at this rate, if before the reset.
    pub runs_out_at: Option<DateTime<Utc>>,
}

pub fn window(w: &Window, now: DateTime<Utc>) -> Pace {
    let used = w.used_percent.clamp(0.0, 100.0);
    let left = 100.0 - used;
    let mut p = Pace { state: State::Ok, elapsed: None, ratio: None, projected_left: None, runs_out_at: None };

    if let (Some(secs), Some(reset)) = (w.window_seconds, w.resets_at) {
        let remaining = (reset - now).num_seconds() as f64;
        if remaining > 0.0 && secs > 0 {
            let elapsed = (1.0 - remaining / secs as f64).clamp(0.0, 1.0);
            p.elapsed = Some(elapsed);
            if elapsed >= MIN_ELAPSED {
                let ratio = used / (elapsed * 100.0);
                p.ratio = Some(ratio);
                p.projected_left = Some((100.0 - ratio * 100.0).clamp(0.0, 100.0));
                let elapsed_secs = elapsed * secs as f64;
                if used > 0.0 {
                    let secs_to_full = left / (used / elapsed_secs);
                    if secs_to_full < remaining {
                        p.runs_out_at = Some(now + chrono::Duration::seconds(secs_to_full as i64));
                    }
                }
                let unused = secs >= UNUSED_MIN_WINDOW_SECS
                    && elapsed >= UNUSED_FROM_ELAPSED
                    && p.projected_left.is_some_and(|l| l >= UNUSED_LEFTOVER);
                if unused {
                    p.state = State::Unused;
                }
            }
        }
    }
    if left < LOW_LEFT || (used >= MIN_USED_FOR_PROJECTION && p.runs_out_at.is_some()) {
        p.state = State::RunningOut;
    }
    p
}

/// A perk this close to expiring is worth using now.
pub const PERK_SOON: chrono::Duration = chrono::Duration::days(3);

/// The most urgent state among a report's windows and perks (balances are always `Ok`).
pub fn report(r: &ProviderReport, now: DateTime<Utc>) -> State {
    let Outcome::Ok { snapshot } = &r.outcome else { return State::Ok };
    let windows = snapshot.windows.iter().map(|w| window(w, now).state).min().unwrap_or(State::Ok);
    let perk_soon =
        snapshot.perks.iter().any(|p| p.count > 0 && p.expires_at.is_some_and(|e| e > now && e - now < PERK_SOON));
    if perk_soon {
        windows.min(State::Unused)
    } else {
        windows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    const WEEK: u64 = 7 * 86400;
    const FIVE_H: u64 = 5 * 3600;

    fn w(used: f64, secs: u64, resets_in: Duration) -> Window {
        Window {
            id: "w".into(),
            label: "w".into(),
            used_percent: used,
            window_seconds: Some(secs),
            resets_at: Some(Utc::now() + resets_in),
        }
    }

    #[test]
    fn weekly_barely_used_late_is_unused() {
        // 5 of 7 days gone, 20% used: on pace to leave ~72% unused.
        let p = window(&w(20.0, WEEK, Duration::days(2)), Utc::now());
        assert_eq!(p.state, State::Unused);
        assert!(p.projected_left.unwrap() > 70.0);
    }

    #[test]
    fn idle_session_is_never_unused() {
        assert_eq!(window(&w(0.0, FIVE_H, Duration::minutes(30)), Utc::now()).state, State::Ok);
    }

    #[test]
    fn early_in_window_is_not_judged() {
        let p = window(&w(5.0, WEEK, Duration::days(7) - Duration::hours(2)), Utc::now());
        assert_eq!(p.state, State::Ok);
        assert!(p.ratio.is_none());
    }

    #[test]
    fn fast_burn_runs_out_before_reset() {
        // 2 of 5 hours gone, 60% used: hits 100% in ~1h20m, reset is in 3h.
        let p = window(&w(60.0, FIVE_H, Duration::hours(3)), Utc::now());
        assert_eq!(p.state, State::RunningOut);
        assert!(p.runs_out_at.is_some());
    }

    #[test]
    fn low_left_is_running_out_regardless_of_pace() {
        assert_eq!(window(&w(85.0, WEEK, Duration::hours(1)), Utc::now()).state, State::RunningOut);
    }

    #[test]
    fn on_pace_is_ok() {
        // Half the week gone, half used.
        assert_eq!(window(&w(50.0, WEEK, Duration::hours(84)), Utc::now()).state, State::Ok);
    }
}
