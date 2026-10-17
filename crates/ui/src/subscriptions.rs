//! The subscriptions of Claude Code and Codex as the window shows them:
//! each window's share used and when it starts again, the context each
//! call read, and what each request took.
//!
//! Both agents report whole percents only. A finer share is estimated:
//! each call's tokens are weighted by the API's price ratios, an
//! assumption since no provider publishes its quota's formula, and a rate,
//! percent per weighted token, is learnt from the history, per model, as
//! models differ, and per plan, as a change of plan starts the quota
//! afresh. Until a plan has added a whole percent, finer shares are left
//! unknown rather than made up, and an estimate never leaves the whole
//! percent reported.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use universe_core::{Agent, Quota, SubscriptionCall};

/// How long the history is kept and read: a week's window and a day more.
pub const KEEP_SECS: u64 = 8 * 24 * 60 * 60;

/// How long a prompt cache lives unread: Codex's, about twenty minutes as
/// measured, and Claude Code's, an hour, as it writes its cache for one.
pub(crate) fn cache_life(agent: Agent) -> u64 {
    match agent {
        Agent::Codex => 20 * 60,
        Agent::ClaudeCode => 60 * 60,
    }
}

/// A call's tokens, weighted as the API prices them against fresh input.
fn weight(call: &SubscriptionCall) -> f64 {
    let (cached, written, output) = match call.agent {
        Agent::Codex => (0.1, 1.0, 8.0),
        Agent::ClaudeCode => (0.1, 2.0, 5.0),
    };
    call.input as f64
        + cached * call.cache_read as f64
        + written * call.cache_written as f64
        + output * call.output as f64
}

/// What universe heard of the agents' quotas, kept across sessions: Claude
/// Code's are seen nowhere else.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Seen {
    /// Every quota reported, oldest first.
    pub quotas: Vec<Quota>,
}

/// The subscriptions as the state holds them.
#[derive(Debug, Clone, Default)]
pub struct Subscriptions {
    seen: Seen,
    /// The agents' calls, as last read from their files.
    calls: Vec<SubscriptionCall>,
    /// `seen` changed since it was kept.
    dirty: bool,
}

/// One window of a subscription, as shown.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowView {
    /// The agent.
    pub agent: Agent,
    /// Its name for the window.
    pub name: String,
    /// How long its period lasts, in minutes.
    pub minutes: u64,
    /// The share used, in percent, as last reported; 0 once its period
    /// ended.
    pub reported: f64,
    /// A finer share, within the whole percent reported, when the rates
    /// are known.
    pub estimate: Option<f64>,
    /// When it starts again, in seconds since 1970; `None` once its period
    /// ended, as the next starts at the first use.
    pub resets_at: Option<u64>,
    /// The shares reported over the current period: when, and how much.
    pub reports: Vec<(u64, f64)>,
}

/// One request an agent worked on, as shown.
#[derive(Debug, Clone, PartialEq)]
pub struct RequestView {
    /// The agent.
    pub agent: Agent,
    /// When it ended, in seconds since 1970.
    pub at: u64,
    /// The models it called.
    pub models: Vec<String>,
    /// The folder of its session.
    pub folder: String,
    /// Whether universe started it.
    pub by_universe: bool,
    /// How many calls it made.
    pub calls: usize,
    /// The tokens read, afresh and from the cache, and written out.
    pub input: u64,
    /// Read from the cache.
    pub cache_read: u64,
    /// Written out.
    pub output: u64,
    /// The share of each window it took, by the window's name and length
    /// in minutes, when known.
    pub shares: Vec<(String, u64, Option<f64>)>,
    /// The tokens read again because the cache had expired, when one of
    /// its calls found it gone.
    pub cache_gone: Option<u64>,
}

/// One session's calls, as shown: the context each read.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionView {
    /// The agent.
    pub agent: Agent,
    /// Its folder.
    pub folder: String,
    /// The model of its last call.
    pub model: String,
    /// Each call: when, and the context it read.
    pub context: Vec<(u64, u64)>,
}

/// A rate learnt: per model, percent per weighted token, by agent and
/// window.
type Rates = BTreeMap<(Agent, String), BTreeMap<String, f64>>;

impl Subscriptions {
    /// What was kept of the last sessions.
    pub(crate) fn loaded(&mut self, seen: Seen) {
        self.seen = seen;
        self.dirty = false;
    }

    /// A quota just reported.
    pub(crate) fn add(&mut self, quota: Quota) {
        let now = quota.at;
        self.seen.quotas.push(quota);
        self.seen.quotas.sort_by_key(|q| q.at);
        self.seen.quotas.retain(|q| q.at + KEEP_SECS >= now);
        self.dirty = true;
    }

    /// The agents' calls, as just read from their files.
    pub(crate) fn calls_read(&mut self, calls: Vec<SubscriptionCall>) {
        self.calls = calls;
    }

    /// What to keep, when it changed since it was last kept.
    pub(crate) fn take_to_keep(&mut self) -> Option<Seen> {
        std::mem::take(&mut self.dirty).then(|| self.seen.clone())
    }

    /// Every quota known: those reported to universe and those the agents'
    /// files hold, oldest first.
    fn quotas(&self) -> Vec<&Quota> {
        let mut all: Vec<&Quota> = self
            .seen
            .quotas
            .iter()
            .chain(self.calls.iter().filter_map(|c| c.quota.as_ref()))
            .collect();
        all.sort_by_key(|q| q.at);
        all
    }

    /// The latest quota of each agent.
    fn latest(&self) -> BTreeMap<Agent, &Quota> {
        let mut latest = BTreeMap::new();
        for quota in self.quotas() {
            latest.insert(quota.agent, quota);
        }
        latest
    }

    /// The rates learnt, for the latest plan of each agent.
    fn rates(&self) -> Rates {
        let quotas = self.quotas();
        let latest = self.latest();
        // Per agent and window: per model, the percent it added and its
        // weighted tokens.
        let mut sums: BTreeMap<(Agent, String), BTreeMap<String, (f64, f64)>> = BTreeMap::new();
        for (agent, last) in &latest {
            let reports: Vec<&&Quota> = quotas
                .iter()
                .filter(|q| q.agent == *agent && q.plan == last.plan)
                .collect();
            for window in &last.windows {
                let points: Vec<(u64, f64, u64)> = reports
                    .iter()
                    .filter_map(|q| {
                        let w = q.windows.iter().find(|w| w.name == window.name)?;
                        Some((q.at, w.used, w.resets_at))
                    })
                    .collect();
                let entry = sums.entry((*agent, window.name.clone())).or_default();
                for pair in points.windows(2) {
                    let ((t0, u0, r0), (t1, u1, r1)) = (pair[0], pair[1]);
                    // Within one period only, and never backwards.
                    if r0.abs_diff(r1) > 120 || u1 < u0 || t1 <= t0 {
                        continue;
                    }
                    let span: Vec<&SubscriptionCall> = self
                        .calls
                        .iter()
                        .filter(|c| c.agent == *agent && c.at > t0 && c.at <= t1)
                        .collect();
                    let total: f64 = span.iter().map(|c| weight(c)).sum();
                    if total <= 0.0 {
                        continue;
                    }
                    for call in span {
                        let w = weight(call);
                        let sum = entry.entry(call.model.clone()).or_default();
                        sum.0 += (u1 - u0) * w / total;
                        sum.1 += w;
                    }
                }
            }
        }
        let mut rates: Rates = BTreeMap::new();
        for (key, models) in &sums {
            let learnt: BTreeMap<String, f64> = models
                .iter()
                .filter(|(_, (added, w))| *added > 0.0 && *w > 0.0)
                .map(|(model, (added, w))| (model.clone(), added / w))
                .collect();
            rates.insert(key.clone(), learnt);
        }
        // A model seen on one window only takes the others' ratio between
        // the two.
        let agents: Vec<Agent> = latest.keys().copied().collect();
        for agent in agents {
            let windows: Vec<String> = rates
                .keys()
                .filter(|(a, _)| *a == agent)
                .map(|(_, w)| w.clone())
                .collect();
            for from in &windows {
                for to in &windows {
                    if from == to {
                        continue;
                    }
                    let (Some(a), Some(b)) = (
                        rates.get(&(agent, from.clone())).cloned(),
                        rates.get(&(agent, to.clone())).cloned(),
                    ) else {
                        continue;
                    };
                    let shared: Vec<f64> = a
                        .iter()
                        .filter_map(|(m, ra)| b.get(m).map(|rb| rb / ra))
                        .collect();
                    if shared.is_empty() {
                        continue;
                    }
                    let ratio = shared.iter().sum::<f64>() / shared.len() as f64;
                    let target = rates.entry((agent, to.clone())).or_default();
                    for (model, ra) in &a {
                        target.entry(model.clone()).or_insert(ra * ratio);
                    }
                }
            }
        }
        rates
    }

    /// The rate of `model` on `window`, when learnt.
    fn rate(rates: &Rates, agent: Agent, window: &str, model: &str) -> Option<f64> {
        rates.get(&(agent, window.to_owned()))?.get(model).copied()
    }

    /// Each window of each agent heard of, at `now`.
    #[must_use]
    pub fn windows(&self, now: u64) -> Vec<WindowView> {
        let rates = self.rates();
        let quotas = self.quotas();
        let mut out = Vec::new();
        for (agent, last) in self.latest() {
            for window in &last.windows {
                let over = window.resets_at <= now;
                let reports: Vec<(u64, f64)> = quotas
                    .iter()
                    .filter(|q| q.agent == agent)
                    .filter_map(|q| {
                        let w = q.windows.iter().find(|w| w.name == window.name)?;
                        (w.resets_at.abs_diff(window.resets_at) <= 120).then_some((q.at, w.used))
                    })
                    .collect();
                let estimate = (!over)
                    .then(|| self.estimate(&rates, agent, &window.name, window.used, &reports))
                    .flatten();
                out.push(WindowView {
                    agent,
                    name: window.name.clone(),
                    minutes: window.minutes,
                    reported: if over { 0.0 } else { window.used },
                    estimate,
                    resets_at: (!over).then_some(window.resets_at),
                    reports: if over { Vec::new() } else { reports },
                });
            }
        }
        out
    }

    /// The share used past the whole percent reported: what the calls
    /// since it was first reported added, within that percent.
    fn estimate(
        &self,
        rates: &Rates,
        agent: Agent,
        window: &str,
        used: f64,
        reports: &[(u64, f64)],
    ) -> Option<f64> {
        // Nothing learnt for this window: unknown, not the share reported
        // dressed as an estimate.
        rates
            .get(&(agent, window.to_owned()))
            .filter(|models| !models.is_empty())?;
        let reached = reports
            .iter()
            .find(|(_, u)| *u >= used)
            .map(|(at, _)| *at)?;
        let mut added = 0.0;
        for call in self
            .calls
            .iter()
            .filter(|c| c.agent == agent && c.at > reached)
        {
            added += Self::rate(rates, agent, window, &call.model)? * weight(call);
        }
        Some(used + added.min(0.99))
    }

    /// The requests of the agents, the latest first, `limit` at most.
    #[must_use]
    pub fn requests(&self, limit: usize) -> Vec<RequestView> {
        let rates = self.rates();
        let windows: Vec<(Agent, String, u64)> = self
            .latest()
            .into_iter()
            .flat_map(|(agent, q)| {
                q.windows
                    .iter()
                    .map(move |w| (agent, w.name.clone(), w.minutes))
            })
            .collect();
        // The previous call of each session, for whether the cache had
        // expired since.
        let mut previous: BTreeMap<(Agent, &str), u64> = BTreeMap::new();
        let mut requests: BTreeMap<(Agent, &str, &str), RequestView> = BTreeMap::new();
        for call in &self.calls {
            let gone = previous
                .insert((call.agent, call.session.as_str()), call.at)
                .is_some_and(|before| {
                    let read_again = call.input + call.cache_written;
                    call.at.saturating_sub(before) >= cache_life(call.agent)
                        && read_again > 4 * call.cache_read
                        && read_again > 5_000
                })
                .then_some(call.input + call.cache_written);
            let view = requests
                .entry((call.agent, call.session.as_str(), call.request.as_str()))
                .or_insert_with(|| RequestView {
                    agent: call.agent,
                    at: call.at,
                    models: Vec::new(),
                    folder: call.folder.clone(),
                    by_universe: call.by_universe,
                    calls: 0,
                    input: 0,
                    cache_read: 0,
                    output: 0,
                    shares: windows
                        .iter()
                        .filter(|(a, _, _)| *a == call.agent)
                        .map(|(_, w, minutes)| (w.clone(), *minutes, Some(0.0)))
                        .collect(),
                    cache_gone: None,
                });
            view.at = view.at.max(call.at);
            if !view.models.contains(&call.model) {
                view.models.push(call.model.clone());
            }
            view.calls += 1;
            view.input += call.input + call.cache_written;
            view.cache_read += call.cache_read;
            view.output += call.output;
            for (window, _, share) in &mut view.shares {
                *share = share
                    .zip(Self::rate(&rates, call.agent, window, &call.model))
                    .map(|(s, r)| s + r * weight(call));
            }
            if let Some(tokens) = gone {
                *view.cache_gone.get_or_insert(0) += tokens;
            }
        }
        let mut out: Vec<RequestView> = requests.into_values().collect();
        out.sort_by_key(|r| std::cmp::Reverse(r.at));
        out.truncate(limit);
        out
    }

    /// The latest sessions of the agents, `limit` at most, each with the
    /// context its calls read.
    #[must_use]
    pub fn sessions(&self, limit: usize) -> Vec<SessionView> {
        let mut sessions: BTreeMap<(Agent, &str), SessionView> = BTreeMap::new();
        for call in &self.calls {
            let view = sessions
                .entry((call.agent, call.session.as_str()))
                .or_insert_with(|| SessionView {
                    agent: call.agent,
                    folder: call.folder.clone(),
                    model: String::new(),
                    context: Vec::new(),
                });
            view.model.clone_from(&call.model);
            view.context
                .push((call.at, call.input + call.cache_read + call.cache_written));
        }
        let mut out: Vec<SessionView> = sessions.into_values().collect();
        out.sort_by_key(|s| std::cmp::Reverse(s.context.last().map_or(0, |c| c.0)));
        out.truncate(limit);
        out
    }

    /// The status line's word on the subscriptions: each window's share and
    /// when it starts again, in local time.
    #[must_use]
    pub fn line(&self, now: u64) -> Option<String> {
        let mut agents: BTreeMap<Agent, Vec<String>> = BTreeMap::new();
        for w in self.windows(now) {
            let Some(at) = w.resets_at else {
                continue;
            };
            let share = w.estimate.unwrap_or(w.reported);
            agents.entry(w.agent).or_default().push(format!(
                "{} {} → {}",
                span(w.minutes),
                percent(share),
                back_at(at, w.minutes)
            ));
        }
        let parts: Vec<String> = agents
            .into_iter()
            .map(|(agent, windows)| format!("{} {}", short(agent), windows.join(", ")))
            .collect();
        (!parts.is_empty()).then(|| parts.join(" · "))
    }
}

/// An agent's name in a few letters.
#[must_use]
pub fn short(agent: Agent) -> &'static str {
    match agent {
        Agent::ClaudeCode => "Claude",
        Agent::Codex => "Codex",
    }
}

/// A window's length: `5h`, `7d`.
#[must_use]
pub fn span(minutes: u64) -> String {
    if minutes.is_multiple_of(24 * 60) {
        format!("{}d", minutes / (24 * 60))
    } else if minutes.is_multiple_of(60) {
        format!("{}h", minutes / 60)
    } else {
        format!("{minutes}m")
    }
}

/// A share in percent: whole, or with a tenth when estimated finer.
#[must_use]
pub fn percent(share: f64) -> String {
    if (share - share.round()).abs() < 0.05 {
        format!("{share:.0}%")
    } else {
        format!("{share:.1}%")
    }
}

/// When a window starts again, in local time: the time of day for a short
/// window, the day and date for a weekly one.
#[must_use]
pub fn back_at(at: u64, minutes: u64) -> String {
    let Ok(stamp) = jiff::Timestamp::from_second(i64::try_from(at).unwrap_or(i64::MAX)) else {
        return String::new();
    };
    let local = stamp.to_zoned(jiff::tz::TimeZone::system());
    if minutes >= 24 * 60 {
        local.strftime("%a %-d %b").to_string()
    } else {
        local.strftime("%H:%M").to_string()
    }
}

/// When something happened, in local time: the day and the time of day.
#[must_use]
pub fn local_time(at: u64) -> String {
    let Ok(stamp) = jiff::Timestamp::from_second(i64::try_from(at).unwrap_or(i64::MAX)) else {
        return String::new();
    };
    stamp
        .to_zoned(jiff::tz::TimeZone::system())
        .strftime("%a %H:%M")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use universe_core::QuotaWindow;

    fn quota(at: u64, used: f64, resets_at: u64) -> Quota {
        Quota {
            agent: Agent::Codex,
            plan: Some("plus".into()),
            windows: vec![QuotaWindow {
                name: "primary".into(),
                minutes: 300,
                used,
                resets_at,
            }],
            at,
        }
    }

    fn call(at: u64, model: &str, input: u64, request: &str) -> SubscriptionCall {
        SubscriptionCall {
            agent: Agent::Codex,
            at,
            model: model.into(),
            session: "s".into(),
            request: request.into(),
            folder: "/p".into(),
            by_universe: true,
            input,
            cache_read: 0,
            cache_written: 0,
            output: 0,
            quota: None,
        }
    }

    #[test]
    fn a_finer_share_is_learnt_per_model_and_stays_within_the_percent_reported() {
        let mut subscriptions = Subscriptions::default();
        // 1 000 weighted tokens of model a took 1%, then of b 2%.
        subscriptions.add(quota(100, 3.0, 10_000));
        subscriptions.add(quota(200, 4.0, 10_000));
        subscriptions.add(quota(300, 6.0, 10_000));
        subscriptions.calls_read(vec![
            call(150, "a", 1_000, "r1"),
            call(250, "b", 1_000, "r2"),
            // Since 6% was reported: half a percent more, by a's rate.
            call(350, "a", 500, "r3"),
        ]);
        let windows = subscriptions.windows(400);
        assert_eq!(windows[0].reported, 6.0);
        let estimate = windows[0].estimate.unwrap();
        assert!((estimate - 6.5).abs() < 1e-9, "{estimate}");
        // Each request's share, by its model's rate.
        let requests = subscriptions.requests(10);
        let share = |r: &RequestView| r.shares[0].2.unwrap();
        assert!((share(&requests[0]) - 0.5).abs() < 1e-9);
        assert!((share(&requests[1]) - 2.0).abs() < 1e-9);
        // However much was used since, the estimate keeps within 6%.
        subscriptions.calls_read(vec![
            call(150, "a", 1_000, "r1"),
            call(250, "b", 1_000, "r2"),
            call(350, "a", 50_000, "r3"),
        ]);
        assert!(subscriptions.windows(400)[0].estimate.unwrap() < 7.0);
    }

    #[test]
    fn until_a_plan_added_a_whole_percent_shares_are_unknown() {
        let mut subscriptions = Subscriptions::default();
        subscriptions.add(quota(100, 3.0, 10_000));
        subscriptions.add(quota(200, 3.0, 10_000));
        subscriptions.calls_read(vec![call(150, "a", 1_000, "r1")]);
        assert_eq!(subscriptions.windows(300)[0].estimate, None);
        assert_eq!(subscriptions.requests(10)[0].shares[0].2, None);
        // A new plan starts afresh: what the old one learnt does not count.
        let mut subscriptions = Subscriptions::default();
        subscriptions.add(quota(100, 3.0, 10_000));
        subscriptions.add(quota(200, 4.0, 10_000));
        let mut pro = quota(300, 0.0, 20_000);
        pro.plan = Some("pro".into());
        subscriptions.add(pro);
        subscriptions.calls_read(vec![call(150, "a", 1_000, "r1"), call(350, "a", 9, "r2")]);
        assert_eq!(subscriptions.windows(400)[0].estimate, None);
    }

    #[test]
    fn an_ended_period_shows_nothing_used_and_no_time_to_start_again() {
        let mut subscriptions = Subscriptions::default();
        subscriptions.add(quota(100, 40.0, 500));
        let window = &subscriptions.windows(600)[0];
        assert_eq!((window.reported, window.resets_at), (0.0, None));
        assert_eq!(subscriptions.line(600), None);
        assert!(
            subscriptions
                .line(400)
                .unwrap()
                .starts_with("Codex 5h 40% → ")
        );
        // Each agent's windows together.
        let mut both = quota(100, 40.0, 500);
        let mut weekly = both.windows[0].clone();
        weekly.name = "secondary".into();
        weekly.minutes = 10_080;
        weekly.used = 3.0;
        weekly.resets_at = 900;
        both.windows.push(weekly);
        let mut subscriptions = Subscriptions::default();
        subscriptions.add(both);
        let line = subscriptions.line(400).unwrap();
        assert!(line.starts_with("Codex 5h 40% → "), "{line}");
        assert!(line.contains(", 7d 3% → "), "{line}");
        assert_eq!(line.matches("Codex").count(), 1);
    }

    #[test]
    fn a_request_after_the_cache_expired_says_what_it_read_again() {
        let mut subscriptions = Subscriptions::default();
        subscriptions.add(quota(100, 1.0, 100_000));
        let mut warm = call(1_000, "a", 200, "r1");
        warm.cache_read = 30_000;
        let cold = call(1_000 + 25 * 60, "a", 30_000, "r2");
        subscriptions.calls_read(vec![warm, cold]);
        let requests = subscriptions.requests(10);
        assert_eq!(requests[0].cache_gone, Some(30_000));
        assert_eq!(requests[1].cache_gone, None);
    }

    #[test]
    fn shares_and_spans_read_short() {
        assert_eq!(span(300), "5h");
        assert_eq!(span(10_080), "7d");
        assert_eq!(percent(12.0), "12%");
        assert_eq!(percent(12.46), "12.5%");
        // The time of day for five hours, the day for a week.
        assert_eq!(back_at(0, 300).len(), 5);
        assert!(back_at(0, 10_080).contains(' '));
    }
}
