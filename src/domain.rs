use anyhow::{bail, Result};
use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub title: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    #[serde(default)]
    pub all_day: bool,
    #[serde(default)]
    pub calendar: String,
    #[serde(default)]
    pub deadline: Option<DateTime<Utc>>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub title: String,
    pub start: DateTime<Utc>,
    pub end: Option<DateTime<Utc>>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Requirement {
    pub title: String,
    pub minutes: i64,
    pub weekly: bool,
    pub enabled: bool,
    /// Monday = 0. Empty means every day.
    #[serde(default)]
    pub days: Vec<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Task {
    pub title: String,
    pub minutes: i64,
    pub deadline: Option<DateTime<Utc>>,
    pub done: bool,
    #[serde(default)]
    pub google_id: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Calendar {
    pub id: String,
    pub title: String,
    pub visible: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub timezone: Tz,
    pub sessions: Vec<Session>,
    pub requirements: Vec<Requirement>,
    pub tasks: Vec<Task>,
    pub events: Vec<Event>,
    pub calendars: Vec<Calendar>,
    pub plan: Vec<Event>,
    pub journal: Vec<String>,
    pub suggested_order: Vec<String>,
    pub omaspeak: String,
    pub voice_model: String,
    pub ollama_url: String,
    pub ollama_model: String,
    pub google_credentials: String,
    pub last_sync: Option<DateTime<Utc>>,
}
impl Default for State {
    fn default() -> Self {
        let mut requirements: Vec<_> = ["DI", "DJ", "VI", "DS", "Drawing"]
            .into_iter()
            .map(|s| Requirement {
                title: s.into(),
                minutes: if s == "Drawing" { 120 } else { 60 },
                weekly: false,
                enabled: true,
                days: vec![],
            })
            .collect();
        requirements.push(Requirement {
            title: "Weightlifting".into(),
            minutes: 60,
            weekly: false,
            enabled: true,
            days: vec![0, 2, 4],
        });
        requirements.push(Requirement {
            title: "Cardio".into(),
            minutes: 60,
            weekly: false,
            enabled: true,
            days: vec![1, 3, 5, 6],
        });
        Self {
            timezone: chrono_tz::Europe::Lisbon,
            sessions: vec![],
            requirements,
            tasks: vec![],
            events: vec![],
            calendars: vec![],
            plan: vec![],
            journal: vec![],
            suggested_order: vec![],
            omaspeak: "handy".into(),
            voice_model: String::new(),
            ollama_url: "http://127.0.0.1:11434".into(),
            ollama_model: "qwen2.5:1.5b".into(),
            google_credentials: String::new(),
            last_sync: None,
        }
    }
}
impl State {
    pub fn midnight(&self, date: NaiveDate) -> DateTime<Utc> {
        // Midnight is unambiguous in the default Lisbon timezone.
        self.timezone
            .from_local_datetime(&date.and_hms_opt(0, 0, 0).expect("midnight"))
            .earliest()
            .unwrap_or_else(|| {
                self.timezone
                    .from_utc_datetime(&date.and_hms_opt(0, 0, 0).expect("midnight"))
            })
            .with_timezone(&Utc)
    }
    pub fn window(&self, now: DateTime<Utc>, weekly: bool) -> (DateTime<Utc>, DateTime<Utc>) {
        let mut d = now.with_timezone(&self.timezone).date_naive();
        if weekly {
            d -= Duration::days(d.weekday().num_days_from_monday() as i64);
        }
        (
            self.midnight(d),
            self.midnight(d + Duration::days(if weekly { 7 } else { 1 })),
        )
    }
    pub fn replace_calendar(
        &mut self,
        calendars: Vec<Calendar>,
        events: Vec<Event>,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        now: DateTime<Utc>,
    ) {
        let source = |e: &Event| format!("{}/{}", e.calendar, e.id);
        self.tasks.retain(|t| {
            t.google_id.as_ref().is_none_or(|id| {
                !self.events.iter().any(|e| {
                    source(e) == *id
                        && e.start >= from
                        && e.start < to
                        && !events
                            .iter()
                            .any(|new| source(new) == *id && new.deadline.is_some())
                })
            })
        });
        for e in &events {
            if let Some(deadline) = e.deadline {
                let id = source(e);
                if let Some(t) = self
                    .tasks
                    .iter_mut()
                    .find(|t| t.google_id.as_ref() == Some(&id))
                {
                    t.title = e.title.clone();
                    t.deadline = Some(deadline);
                } else {
                    self.tasks.push(Task {
                        title: e.title.clone(),
                        minutes: 120,
                        deadline: Some(deadline),
                        done: false,
                        google_id: Some(id),
                    });
                }
            }
        }
        self.calendars = calendars;
        self.events = events;
        self.last_sync = Some(now);
        self.plan.clear();
    }
    pub fn visible_events(&self) -> Vec<&Event> {
        self.events
            .iter()
            .filter(|e| {
                self.calendars
                    .iter()
                    .find(|c| c.id == e.calendar)
                    .is_none_or(|c| c.visible)
            })
            .collect()
    }
    pub fn active(&self) -> Option<&Session> {
        self.sessions.iter().find(|s| s.end.is_none())
    }
    pub fn start(&mut self, title: String, now: DateTime<Utc>) {
        self.stop(now);
        self.sessions.push(Session {
            title,
            start: now,
            end: None,
        });
        self.plan.clear();
    }
    pub fn stop(&mut self, now: DateTime<Utc>) {
        for s in &mut self.sessions {
            if s.end.is_none() {
                s.end = Some(now.max(s.start));
            }
        }
    }
    pub fn minutes(
        &self,
        title: &str,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> i64 {
        let mut intervals: Vec<_> = self
            .sessions
            .iter()
            .filter(|s| s.title.eq_ignore_ascii_case(title))
            .filter_map(|s| {
                let a = s.start.max(from);
                let b = s.end.unwrap_or(now).min(to).min(now);
                (b > a).then_some((a, b))
            })
            .collect();
        intervals.sort();
        let mut merged: Vec<(DateTime<Utc>, DateTime<Utc>)> = vec![];
        for (a, b) in intervals {
            if let Some(last) = merged.last_mut() {
                if a <= last.1 {
                    last.1 = last.1.max(b);
                    continue;
                }
            }
            merged.push((a, b));
        }
        merged
            .iter()
            .map(|(a, b)| (*b - *a).num_seconds())
            .sum::<i64>()
            / 60
    }
    pub fn applicable(&self, r: &Requirement, now: DateTime<Utc>) -> bool {
        r.enabled
            && (r.weekly
                || r.days.is_empty()
                || r.days.contains(
                    &now.with_timezone(&self.timezone)
                        .weekday()
                        .num_days_from_monday(),
                ))
    }
    pub fn remaining(&self, r: &Requirement, now: DateTime<Utc>) -> i64 {
        let (a, b) = self.window(now, r.weekly);
        (r.minutes - self.minutes(&r.title, a, b, now)).max(0)
    }
    pub fn queue(&self, now: DateTime<Utc>) -> Vec<(String, i64)> {
        let mut tasks: Vec<_> = self
            .tasks
            .iter()
            .filter(|t| {
                !t.done
                    && t.google_id.as_ref().is_none_or(|id| {
                        self.calendars
                            .iter()
                            .find(|c| id.starts_with(&format!("{}/", c.id)))
                            .is_none_or(|c| c.visible)
                    })
            })
            .collect();
        // Urgent deadlines first; manual ordering is retained for the rest.
        tasks.sort_by_key(|t| {
            t.deadline
                .filter(|d| *d <= now + Duration::hours(72))
                .unwrap_or(DateTime::<Utc>::MAX_UTC)
        });
        let mut q: Vec<_> = tasks
            .iter()
            .map(|t| (t.title.clone(), t.minutes.clamp(1, 120)))
            .collect();
        for r in &self.requirements {
            if self.applicable(r, now) {
                let n = self.remaining(r, now);
                if n > 0 {
                    q.push((r.title.clone(), n.min(120)));
                }
            }
        }
        let (a, b) = self.window(now, true);
        let docs = self.minutes("Docs", a, b, now);
        let games = self.minutes("Game development", a, b, now);
        let pair = if docs <= games {
            ["Docs", "Game development"]
        } else {
            ["Game development", "Docs"]
        };
        for s in pair {
            q.push((s.into(), 60));
        }
        q.sort_by_key(|(title, _)| {
            let urgent = self
                .tasks
                .iter()
                .find(|t| !t.done && t.title == *title)
                .and_then(|t| t.deadline)
                .filter(|d| *d <= now + Duration::hours(72));
            (
                urgent.is_none(),
                urgent.unwrap_or(DateTime::<Utc>::MAX_UTC),
                self.suggested_order
                    .iter()
                    .position(|t| t == title)
                    .unwrap_or(usize::MAX),
            )
        });
        q
    }
    pub fn next(&self, now: DateTime<Utc>) -> String {
        if let Some(s) = self.active() {
            if s.title.eq_ignore_ascii_case("Sleep") {
                return "Sleeping · wake when ready".into();
            }
        }
        if let Some(e) = self
            .visible_events()
            .into_iter()
            .filter(|e| !e.all_day && e.end > now && e.start <= now + Duration::minutes(20))
            .min_by_key(|e| e.start)
        {
            return format!(
                "{} · {}",
                e.title,
                e.start.with_timezone(&self.timezone).format("%H:%M")
            );
        }
        self.queue(now)
            .first()
            .map(|(s, m)| format!("{s} · {m} min"))
            .unwrap_or_default()
    }
    /// Explicitly requested intentions only: fixed commitments are never moved.
    pub fn fill(&mut self, now: DateTime<Utc>, hours: i64) {
        self.plan.clear();
        if self
            .active()
            .is_some_and(|s| s.title.eq_ignore_ascii_case("Sleep"))
        {
            return;
        }
        let end = now + Duration::hours(hours.clamp(1, 24));
        let mut cursor = now;
        let mut busy: Vec<_> = self
            .visible_events()
            .into_iter()
            .filter(|e| !e.all_day && e.end > now && e.start < end)
            .map(|e| (e.start, e.end))
            .collect();
        busy.sort();
        let mut q = self.queue(now).into_iter();
        let mut work = q.next();
        while cursor < end {
            if let Some((_, b)) = busy.iter().find(|(a, b)| *a <= cursor && *b > cursor) {
                cursor = *b;
                continue;
            }
            let gap = busy
                .iter()
                .filter(|(a, _)| *a > cursor)
                .map(|(a, _)| *a)
                .min()
                .unwrap_or(end)
                .min(end);
            let (title, remaining) = work.take().unwrap_or_else(|| {
                let (a, b) = self.window(now, true);
                let projected = |name: &str| {
                    self.minutes(name, a, b, now)
                        + self
                            .plan
                            .iter()
                            .filter(|e| e.title == name)
                            .map(|e| (e.end - e.start).num_minutes())
                            .sum::<i64>()
                };
                (
                    if projected("Docs") <= projected("Game development") {
                        "Docs"
                    } else {
                        "Game development"
                    }
                    .into(),
                    60,
                )
            });
            let n = remaining.min((gap - cursor).num_minutes());
            if n <= 0 {
                cursor = gap;
                work = Some((title, remaining));
                continue;
            }
            let next = cursor + Duration::minutes(n);
            self.plan.push(Event {
                id: format!("plan-{}", self.plan.len()),
                title: title.clone(),
                start: cursor,
                end: next,
                all_day: false,
                calendar: String::new(),
                deadline: None,
            });
            cursor = next;
            work = if n < remaining {
                Some((title, remaining - n))
            } else {
                q.next()
            };
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    Start {
        title: String,
    },
    Stop,
    Sleep,
    Wake,
    Plan {
        order: Vec<String>,
    },
    Log {
        title: String,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    },
    Task {
        title: String,
        minutes: i64,
        deadline: Option<DateTime<Utc>>,
    },
    Requirement {
        title: String,
        minutes: i64,
        weekly: bool,
    },
}
pub fn validate(actions: &[Action], now: DateTime<Utc>) -> Result<()> {
    if actions.is_empty() || actions.len() > 30 {
        bail!("Expected 1–30 actions");
    }
    for a in actions {
        match a {
            Action::Start { title }
            | Action::Task { title, .. }
            | Action::Requirement { title, .. }
            | Action::Log { title, .. }
                if title.trim().is_empty() || title.len() > 200 =>
            {
                bail!("Activity needs a short title")
            }
            Action::Task { minutes, .. } | Action::Requirement { minutes, .. }
                if *minutes <= 0 || *minutes > 10080 =>
            {
                bail!("Minutes must be between 1 and 10080")
            }
            Action::Log { start, end, .. }
                if end <= start
                    || *end > now + Duration::minutes(1)
                    || *end - *start > Duration::days(7) =>
            {
                bail!("Reported activity must be a past interval of at most seven days")
            }
            _ => {}
        }
    }
    Ok(())
}
pub fn apply(state: &mut State, actions: &[Action], now: DateTime<Utc>) -> Result<()> {
    validate(actions, now)?;
    for a in actions {
        if let Action::Plan { order } = a {
            let available = state.queue(now);
            if order.len() > 100
                || order
                    .iter()
                    .any(|t| !available.iter().any(|(name, _)| name == t))
            {
                bail!("Planner returned an unknown activity");
            }
        }
    }
    for a in actions {
        match a {
            Action::Plan { order } => {
                let available = state.queue(now);
                if order.len() > 100
                    || order
                        .iter()
                        .any(|t| !available.iter().any(|(name, _)| name == t))
                {
                    bail!("Planner returned an unknown activity");
                }
                state.suggested_order = order.clone();
            }
            Action::Start { title } => state.start(title.clone(), now),
            Action::Stop | Action::Wake => state.stop(now),
            Action::Sleep => state.start("Sleep".into(), now),
            Action::Log { title, start, end } => state.sessions.push(Session {
                title: title.clone(),
                start: *start,
                end: Some(*end),
            }),
            Action::Task {
                title,
                minutes,
                deadline,
            } => state.tasks.push(Task {
                title: title.clone(),
                minutes: *minutes,
                deadline: *deadline,
                done: false,
                google_id: None,
            }),
            Action::Requirement {
                title,
                minutes,
                weekly,
            } => {
                if let Some(r) = state
                    .requirements
                    .iter_mut()
                    .find(|r| r.title.eq_ignore_ascii_case(title) && r.weekly == *weekly)
                {
                    r.minutes = *minutes;
                    r.enabled = true;
                } else {
                    state.requirements.push(Requirement {
                        title: title.clone(),
                        minutes: *minutes,
                        weekly: *weekly,
                        enabled: true,
                        days: vec![],
                    });
                }
            }
        }
    }
    state.plan.clear();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn time(s: &str) -> DateTime<Utc> {
        s.parse().expect("test timestamp")
    }
    fn event(a: &str, b: &str) -> Event {
        Event {
            id: "fixed".into(),
            title: "Class".into(),
            start: time(a),
            end: time(b),
            all_day: false,
            calendar: "uni".into(),
            deadline: None,
        }
    }
    #[test]
    fn calendar_deadlines_sync_without_duplicates_and_preserve_completion() {
        let mut s = State::default();
        let now = time("2026-10-19T08:00:00Z");
        let mut e = event("2026-10-19T13:00:00Z", "2026-10-19T14:00:00Z");
        e.title = "Entrega VI".into();
        e.deadline = Some(e.start);
        let from = now - Duration::days(1);
        let to = now + Duration::days(8);
        s.replace_calendar(vec![], vec![e.clone()], from, to, now);
        assert_eq!(s.tasks.len(), 1);
        assert_eq!(s.queue(now)[0].0, "Entrega VI");
        s.tasks[0].done = true;
        s.replace_calendar(vec![], vec![e], from, to, now);
        assert_eq!(s.tasks.len(), 1);
        assert!(s.tasks[0].done);
        s.replace_calendar(vec![], vec![], from, to, now);
        assert!(s.tasks.is_empty());
    }
    #[test]
    fn sleeping_suppresses_work_and_gap_fill() {
        let mut s = State::default();
        let now = time("2026-10-17T18:00:00Z");
        s.start("Sleep".into(), now);
        assert!(s.next(now).starts_with("Sleeping"));
        s.fill(now, 8);
        assert!(s.plan.is_empty());
        s.stop(now + Duration::hours(8));
        assert!(s.active().is_none());
    }
    #[test]
    fn gap_fill_splits_around_fixed_commitments() {
        let mut s = State::default();
        let now = time("2026-10-19T08:00:00Z");
        s.events
            .push(event("2026-10-19T08:30:00Z", "2026-10-19T10:00:00Z"));
        s.fill(now, 4);
        assert_eq!(s.plan[0].end, time("2026-10-19T08:30:00Z"));
        assert_eq!(s.plan[1].start, time("2026-10-19T10:00:00Z"));
        for p in &s.plan {
            assert!(p.end <= s.events[0].start || p.start >= s.events[0].end);
            assert!(p.end <= now + Duration::hours(4));
        }
    }
    #[test]
    fn reports_are_clipped_at_midnight_and_overlap_is_not_double_counted() {
        let mut s = State::default();
        let now = time("2026-10-18T02:00:00Z");
        s.sessions = vec![
            Session {
                title: "Docs".into(),
                start: time("2026-10-17T22:00:00Z"),
                end: Some(now),
            },
            Session {
                title: "Docs".into(),
                start: time("2026-10-18T00:00:00Z"),
                end: Some(now),
            },
        ];
        let (a, b) = s.window(now, false);
        assert_eq!(s.minutes("Docs", a, b, now), 180);
    }
    #[test]
    fn weekly_progress_survives_day_change_and_dst() {
        let mut s = State::default();
        let now = time("2026-10-25T12:00:00Z");
        let (a, b) = s.window(now, true);
        assert_eq!((b - a).num_hours(), 169);
        s.sessions.push(Session {
            title: "Cardio".into(),
            start: time("2026-10-20T12:00:00Z"),
            end: Some(time("2026-10-20T13:00:00Z")),
        });
        let r = Requirement {
            title: "Cardio".into(),
            minutes: 180,
            weekly: true,
            enabled: true,
            days: vec![],
        };
        assert_eq!(s.remaining(&r, now), 120);
    }
    #[test]
    fn fixed_event_takes_precedence_and_hidden_calendars_are_excluded() {
        let mut s = State::default();
        let now = time("2026-10-19T08:00:00Z");
        s.events
            .push(event("2026-10-19T07:00:00Z", "2026-10-19T09:00:00Z"));
        assert!(s.next(now).starts_with("Class"));
        s.calendars.push(Calendar {
            id: "uni".into(),
            title: "Uni".into(),
            visible: false,
        });
        assert!(!s.next(now).starts_with("Class"));
    }
    #[test]
    fn deadlines_override_model_order_but_manual_order_is_stable() {
        let mut s = State::default();
        let now = time("2026-10-19T08:00:00Z");
        s.tasks = vec![
            Task {
                title: "Later".into(),
                minutes: 60,
                deadline: None,
                done: false,
                google_id: None,
            },
            Task {
                title: "VI deadline".into(),
                minutes: 120,
                deadline: Some(now + Duration::days(1)),
                done: false,
                google_id: None,
            },
        ];
        s.suggested_order = vec!["Later".into(), "VI deadline".into()];
        assert_eq!(s.queue(now)[0].0, "VI deadline");
        s.tasks[1].done = true;
        assert_eq!(s.queue(now)[0].0, "Later");
    }
    #[test]
    fn docs_games_balance_uses_actual_weekly_work() {
        let mut s = State::default();
        s.requirements.clear();
        let now = time("2026-10-19T18:00:00Z");
        s.sessions.push(Session {
            title: "Docs".into(),
            start: now - Duration::hours(2),
            end: Some(now),
        });
        assert_eq!(s.queue(now)[0].0, "Game development");
    }
    #[test]
    fn invalid_model_batch_does_not_partially_apply() {
        let mut s = State::default();
        let now = time("2026-10-19T18:00:00Z");
        let a = vec![
            Action::Start {
                title: "Docs".into(),
            },
            Action::Log {
                title: "Drawing".into(),
                start: now,
                end: now + Duration::hours(2),
            },
        ];
        assert!(apply(&mut s, &a, now).is_err());
        assert!(s.sessions.is_empty());
        let a = vec![
            Action::Start {
                title: "Docs".into(),
            },
            Action::Plan {
                order: vec!["Invented activity".into()],
            },
        ];
        assert!(apply(&mut s, &a, now).is_err());
        assert!(s.sessions.is_empty());
    }
    #[test]
    fn reporting_requirement_updates_existing_target() {
        let mut s = State::default();
        let now = time("2026-10-19T18:00:00Z");
        apply(
            &mut s,
            &[Action::Requirement {
                title: "Drawing".into(),
                minutes: 180,
                weekly: false,
            }],
            now,
        )
        .expect("valid action");
        assert_eq!(
            s.requirements
                .iter()
                .filter(|r| r.title == "Drawing")
                .count(),
            1
        );
        assert_eq!(
            s.requirements
                .iter()
                .find(|r| r.title == "Drawing")
                .expect("drawing")
                .minutes,
            180
        );
    }
}
