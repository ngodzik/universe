//! What an agent's subscription allows and what its calls took from it, as
//! the agent reports it: its windows of quota, each a share used of a fixed
//! period, and each call's tokens.

use serde::{Deserialize, Serialize};

use crate::types::Agent;

/// One window of a subscription's quota.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuotaWindow {
    /// The agent's name for it, such as `five_hour` or `primary`: two
    /// windows may last as long.
    pub name: String,
    /// How long its period lasts, in minutes: 300 for five hours, 10080 for
    /// a week. A period starts at the first use after the last one ended.
    pub minutes: u64,
    /// The share of it used, in percent, as reported: whole percents, from
    /// both agents.
    pub used: f64,
    /// When its period ends and it starts again, in seconds since 1970.
    pub resets_at: u64,
}

/// A subscription's quota as an agent reported it at one moment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Quota {
    /// The agent whose subscription it is.
    pub agent: Agent,
    /// The plan, when the agent says, such as `plus` or `pro`: a change of
    /// plan starts the quota afresh.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// Its windows, the shortest first.
    pub windows: Vec<QuotaWindow>,
    /// When it was reported, in seconds since 1970.
    pub at: u64,
}

/// One call an agent made to its model under a subscription, as its own
/// files keep it: whoever started it, universe or the person.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubscriptionCall {
    /// The agent.
    pub agent: Agent,
    /// When it ended, in seconds since 1970.
    pub at: u64,
    /// The model it called.
    pub model: String,
    /// The agent's session it belongs to.
    pub session: String,
    /// The request of that session it belongs to, as the agent names it.
    pub request: String,
    /// The folder the session works in.
    pub folder: String,
    /// Whether universe started the session, rather than the person in the
    /// agent's own interface.
    pub by_universe: bool,
    /// Input tokens read afresh, the cache's apart.
    pub input: u64,
    /// Input tokens read from the prompt cache.
    pub cache_read: u64,
    /// Input tokens written to the prompt cache, when the agent says.
    pub cache_written: u64,
    /// Output tokens, reasoning included.
    pub output: u64,
    /// The quota as reported with the call, when it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota: Option<Quota>,
}
