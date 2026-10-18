//! Where a request's time went: its flight log. A request flies one leg at
//! a time, waiting for a model, running tests or other commands, waiting
//! for the person to approve something, or running the checks; the log
//! sums each. The clock of the request stops while the person is asked:
//! that time is theirs, not the request's.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// What a request's time is spent on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Leg {
    /// Waiting for a model or an agent.
    Model,
    /// A command that runs tests.
    Tests,
    /// Any other command.
    Commands,
    /// Waiting for the person to approve something.
    Approvals,
    /// The checks that judge the work.
    Checks,
}

impl Leg {
    /// Every leg, in the order the log shows them.
    pub const ALL: [Self; 5] = [
        Self::Model,
        Self::Tests,
        Self::Commands,
        Self::Approvals,
        Self::Checks,
    ];

    /// Its name, as the log shows it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Tests => "tests",
            Self::Commands => "commands",
            Self::Approvals => "approvals",
            Self::Checks => "checks",
        }
    }

    /// The leg a command flies: tests when one of its words names a test
    /// runner or a test target, other commands otherwise.
    #[must_use]
    pub fn of_command(command: &str) -> Self {
        /// Words that run tests, wherever they stand: `cargo test`,
        /// `npm run test:unit`, `uv run pytest -x`, `go test ./...`.
        const TESTS: [&str; 12] = [
            "test", "tests", "pytest", "jest", "vitest", "mocha", "rspec", "phpunit", "nextest",
            "ctest", "tox", "unittest",
        ];
        let runs_tests = command
            .split(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')'))
            .map(|word| word.rsplit('/').next().unwrap_or(word))
            .map(|word| word.split(':').next().unwrap_or(word))
            .any(|word| TESTS.contains(&word));
        if runs_tests {
            Self::Tests
        } else {
            Self::Commands
        }
    }
}

/// Where a finished request's time went: each leg flown and how long, in
/// milliseconds, in [`Leg::ALL`]'s order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Flight(pub Vec<(Leg, u64)>);

impl Flight {
    /// The legs that took a second or more, as the log shows them:
    /// `model 12s · tests 40s`.
    #[must_use]
    pub fn line(&self) -> String {
        self.0
            .iter()
            .filter(|(_, ms)| *ms >= 1000)
            .map(|(leg, ms)| format!("{} {}", leg.name(), seconds(*ms)))
            .collect::<Vec<_>>()
            .join(" · ")
    }

    /// The whole time, in milliseconds.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.0.iter().map(|(_, ms)| ms).sum()
    }
}

/// `ms` as the log writes it: `4s`, `2m 05s`.
fn seconds(ms: u64) -> String {
    let s = (ms + 500) / 1000;
    if s < 60 {
        format!("{s}s")
    } else {
        format!("{}m {:02}s", s / 60, s % 60)
    }
}

/// The log of a request in flight.
#[derive(Debug, Clone)]
pub struct Log {
    /// Each leg's time so far, the current one's not yet added.
    flown: [Duration; 5],
    /// The leg flown now, and since when.
    now: (Leg, Instant),
    /// The leg to go back to once the person answered.
    before_asking: Option<Leg>,
}

impl Log {
    /// A request that starts at `at`, waiting for its model.
    #[must_use]
    pub fn start(at: Instant) -> Self {
        Self {
            flown: [Duration::ZERO; 5],
            now: (Leg::Model, at),
            before_asking: None,
        }
    }

    /// The request flies `leg` from `at`.
    pub fn switch(&mut self, leg: Leg, at: Instant) {
        let (current, since) = self.now;
        if current == leg {
            return;
        }
        self.flown[index(current)] += at.saturating_duration_since(since);
        self.now = (leg, at);
    }

    /// The person is asked something at `at`; [`Log::answered`] goes back
    /// to what the request was doing.
    pub fn ask(&mut self, at: Instant) {
        if self.now.0 != Leg::Approvals {
            self.before_asking = Some(self.now.0);
        }
        self.switch(Leg::Approvals, at);
    }

    /// The person answered at `at`.
    pub fn answered(&mut self, at: Instant) {
        if let Some(leg) = self.before_asking.take() {
            self.switch(leg, at);
        }
    }

    /// The leg flown now.
    #[must_use]
    pub fn leg(&self) -> Leg {
        self.now.0
    }

    /// How long the request has run at `at`, the person's time left out:
    /// its clock stops while they are asked.
    #[must_use]
    pub fn clock(&self, at: Instant) -> Duration {
        let (current, since) = self.now;
        let running = if current == Leg::Approvals {
            Duration::ZERO
        } else {
            at.saturating_duration_since(since)
        };
        self.flown
            .iter()
            .zip(Leg::ALL)
            .filter(|(_, leg)| *leg != Leg::Approvals)
            .map(|(time, _)| *time)
            .sum::<Duration>()
            + running
    }

    /// The log of the request ended at `at`.
    #[must_use]
    pub fn end(mut self, at: Instant) -> Flight {
        let (current, since) = self.now;
        self.flown[index(current)] += at.saturating_duration_since(since);
        Flight(
            Leg::ALL
                .into_iter()
                .zip(self.flown)
                .filter(|(_, time)| !time.is_zero())
                .map(|(leg, time)| (leg, u64::try_from(time.as_millis()).unwrap_or(u64::MAX)))
                .collect(),
        )
    }
}

fn index(leg: Leg) -> usize {
    Leg::ALL.iter().position(|l| *l == leg).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_that_runs_tests_is_told_from_the_others() {
        for tests in [
            "cargo test -p ui",
            "uv run pytest -x",
            "npm run test:unit",
            "cd web && npx vitest run",
            "go test ./...",
            "./node_modules/.bin/jest",
            "cargo nextest run",
        ] {
            assert_eq!(Leg::of_command(tests), Leg::Tests, "{tests}");
        }
        for other in [
            "cargo build",
            "ls tests_data",
            "git status",
            "grep -rn latest .",
        ] {
            assert_eq!(Leg::of_command(other), Leg::Commands, "{other}");
        }
    }

    #[test]
    fn each_leg_is_summed_and_the_clock_stops_while_the_person_is_asked() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut log = Log::start(t0);
        log.switch(Leg::Tests, at(10));
        log.switch(Leg::Model, at(40));
        log.ask(at(45));
        assert_eq!(log.leg(), Leg::Approvals);
        // Asked for 20 seconds: the clock does not move.
        assert_eq!(log.clock(at(45)), Duration::from_secs(45));
        assert_eq!(log.clock(at(65)), Duration::from_secs(45));
        log.answered(at(65));
        assert_eq!(log.leg(), Leg::Model);
        log.switch(Leg::Checks, at(70));
        let flight = log.end(at(100));
        assert_eq!(
            flight.0,
            [
                (Leg::Model, 20_000),
                (Leg::Tests, 30_000),
                (Leg::Approvals, 20_000),
                (Leg::Checks, 30_000),
            ]
        );
        assert_eq!(
            flight.line(),
            "model 20s · tests 30s · approvals 20s · checks 30s"
        );
        assert_eq!(flight.total(), 100_000);
    }

    #[test]
    fn long_legs_are_written_in_minutes() {
        assert_eq!(seconds(125_000), "2m 05s");
        assert_eq!(seconds(400), "0s");
    }
}
