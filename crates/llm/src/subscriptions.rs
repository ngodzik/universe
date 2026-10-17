//! What Claude Code and Codex say of their subscriptions: their quota, as
//! each reports it, and every call they made under it, as their own files
//! keep it, whoever started it. None of this is a documented interface:
//! each field is read defensively, and what is not understood is passed
//! over.
//!
//! Claude Code reports its quota in its stream, `rate_limit_event`, once a
//! request, and keeps each message's tokens under `~/.claude/projects`.
//! Codex keeps both in its session files under `~/.codex/sessions`: each
//! `token_count` event carries the call's tokens and the quota.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use serde_json::Value;
use universe_core::{Agent, Quota, QuotaWindow, SubscriptionCall};

/// Seconds since 1970.
pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Claude Code's quota, from the `rate_limit_info` of its stream's
/// `rate_limit_event`: each window's utilization, from 0 to 1.
pub(crate) fn claude_quota(info: &Value, at: u64) -> Option<Quota> {
    let windows = info["unifiedWindows"].as_object()?;
    let mut out: Vec<QuotaWindow> = windows
        .iter()
        .filter_map(|(name, window)| {
            let minutes = match name.as_str() {
                "five_hour" => 300,
                n if n.starts_with("seven_day") => 7 * 24 * 60,
                _ => return None,
            };
            Some(QuotaWindow {
                name: name.clone(),
                minutes,
                used: window["utilization"].as_f64()? * 100.0,
                resets_at: window["resetsAt"].as_u64()?,
            })
        })
        .collect();
    out.sort_by(|a, b| a.minutes.cmp(&b.minutes).then_with(|| a.name.cmp(&b.name)));
    (!out.is_empty()).then_some(Quota {
        agent: Agent::ClaudeCode,
        plan: None,
        windows: out,
        at,
    })
}

/// Codex's quota, from the `rate_limits` of a `token_count` event: its
/// primary and secondary windows, each a share used in percent.
pub(crate) fn codex_quota(limits: &Value, at: u64) -> Option<Quota> {
    let windows: Vec<QuotaWindow> = ["primary", "secondary"]
        .into_iter()
        .filter_map(|name| {
            let window = &limits[name];
            Some(QuotaWindow {
                name: name.to_owned(),
                minutes: window["window_minutes"].as_u64()?,
                used: window["used_percent"].as_f64()?,
                resets_at: window["resets_at"].as_u64()?,
            })
        })
        .collect();
    (!windows.is_empty()).then_some(Quota {
        agent: Agent::Codex,
        plan: limits["plan_type"].as_str().map(str::to_owned),
        windows,
        at,
    })
}

/// `2026-10-17T19:24:29.123Z` as seconds since 1970; only UTC times, as
/// both agents write them.
pub(crate) fn epoch(iso: &str) -> Option<u64> {
    let (date, time) = iso.split_once('T')?;
    let mut date = date.split('-').map(|p| p.parse::<i64>().ok());
    let (year, month, day) = (date.next()??, date.next()??, date.next()??);
    let time = time.trim_end_matches('Z');
    let time = time.split(['.', '+']).next()?;
    let mut time = time.split(':').map(|p| p.parse::<i64>().ok());
    let (hour, minute, second) = (time.next()??, time.next()??, time.next()??);
    // Days from the civil date, after Howard Hinnant's algorithm.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + hour * 3_600 + minute * 60 + second).ok()
}

/// One session file, read as far as it was, with what its next lines need
/// to know of the earlier ones.
#[derive(Debug, Default)]
struct Read {
    offset: u64,
    calls: Vec<SubscriptionCall>,
    session: String,
    folder: String,
    by_universe: bool,
    model: String,
    request: String,
    /// Claude Code writes a message once per block of its content.
    messages: HashSet<String>,
}

/// The agents' session files as read so far, for the whole program: each
/// is read again only from where it was left.
pub(crate) static LOG: std::sync::LazyLock<CallLog> = std::sync::LazyLock::new(CallLog::default);

/// The agents' session files, each read again only from where it was left.
#[derive(Debug, Default)]
pub(crate) struct CallLog {
    files: Mutex<HashMap<PathBuf, Read>>,
}

impl CallLog {
    /// Every call of both agents since `since`, oldest first.
    pub(crate) fn since(&self, since: u64) -> Vec<SubscriptionCall> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let claude = std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|h| h.join(".claude")))
            .map(|d| d.join("projects"));
        let codex = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|h| h.join(".codex")))
            .map(|d| d.join("sessions"));
        let mut paths: Vec<(Agent, PathBuf)> = Vec::new();
        if let Some(dir) = claude {
            jsonl_since(&dir, since, &mut |p| paths.push((Agent::ClaudeCode, p)));
        }
        if let Some(dir) = codex {
            jsonl_since(&dir, since, &mut |p| paths.push((Agent::Codex, p)));
        }
        let Ok(mut files) = self.files.lock() else {
            return Vec::new();
        };
        let mut calls = Vec::new();
        for (agent, path) in paths {
            let read = files.entry(path.clone()).or_default();
            read_on(agent, &path, read);
            calls.extend(read.calls.iter().filter(|c| c.at >= since).cloned());
        }
        calls.sort_by_key(|c| c.at);
        calls
    }
}

/// Calls `found` with each `.jsonl` file under `dir` changed since `since`.
fn jsonl_since(dir: &Path, since: u64, found: &mut dyn FnMut(PathBuf)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_dir() {
            jsonl_since(&path, since, found);
        } else if path.extension().is_some_and(|e| e == "jsonl") {
            let changed = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            if changed >= since {
                found(path);
            }
        }
    }
}

/// Reads `path` on from where `read` left it, a line at a time, whole
/// lines only: the last may still be being written.
fn read_on(agent: Agent, path: &Path, read: &mut Read) {
    use std::io::{BufRead, Seek};
    let Ok(mut file) = std::fs::File::open(path) else {
        return;
    };
    if file.seek(std::io::SeekFrom::Start(read.offset)).is_err() {
        return;
    }
    let mut lines = std::io::BufReader::new(file);
    let mut line = Vec::new();
    loop {
        line.clear();
        match lines.read_until(b'\n', &mut line) {
            Ok(n) if n > 0 && line.ends_with(b"\n") => {
                read.offset += n as u64;
                let text = String::from_utf8_lossy(&line);
                match agent {
                    Agent::ClaudeCode => claude_line(&text, read),
                    Agent::Codex => codex_line(&text, read),
                }
            }
            _ => return,
        }
    }
}

/// One line of a Claude Code session file: a message of the model with its
/// tokens, or what the person asked, which starts a request.
fn claude_line(line: &str, read: &mut Read) {
    // Most lines are neither: they are passed over before being parsed.
    if !line.contains("\"usage\"") && !line.contains("\"type\":\"user\"") {
        return;
    }
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return;
    };
    if read.session.is_empty() {
        read.session = value["sessionId"].as_str().unwrap_or_default().to_owned();
        read.folder = value["cwd"].as_str().unwrap_or_default().to_owned();
        // Run by `claude -p`, as universe runs it, rather than in its own
        // interface.
        read.by_universe = value["entrypoint"].as_str() == Some("sdk-cli");
    }
    match value["type"].as_str() {
        Some("user") => {
            let asked = match &value["message"]["content"] {
                Value::String(_) => true,
                Value::Array(blocks) => blocks.iter().any(|b| b["type"] == "text"),
                _ => false,
            };
            if asked && value["isMeta"] != true {
                read.request = value["uuid"].as_str().unwrap_or_default().to_owned();
            }
        }
        Some("assistant") => {
            let message = &value["message"];
            let id = message["id"].as_str().unwrap_or_default().to_owned();
            if id.is_empty() || !read.messages.insert(id) {
                return;
            }
            let usage = &message["usage"];
            let count = |key: &str| usage[key].as_u64().unwrap_or(0);
            let Some(at) = value["timestamp"].as_str().and_then(epoch) else {
                return;
            };
            read.calls.push(SubscriptionCall {
                agent: Agent::ClaudeCode,
                at,
                model: message["model"].as_str().unwrap_or_default().to_owned(),
                session: read.session.clone(),
                request: read.request.clone(),
                folder: read.folder.clone(),
                by_universe: read.by_universe,
                input: count("input_tokens"),
                cache_read: count("cache_read_input_tokens"),
                cache_written: count("cache_creation_input_tokens"),
                output: count("output_tokens"),
                quota: None,
            });
        }
        _ => {}
    }
}

/// One line of a Codex session file: who started the session and where, a
/// request starting, the model it uses, or a call's tokens and the quota.
fn codex_line(line: &str, read: &mut Read) {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return;
    };
    let payload = &value["payload"];
    match (value["type"].as_str(), payload["type"].as_str()) {
        (Some("session_meta"), _) => {
            read.session = payload["id"].as_str().unwrap_or_default().to_owned();
            read.folder = payload["cwd"].as_str().unwrap_or_default().to_owned();
            read.by_universe = payload["originator"].as_str() == Some("codex_exec");
        }
        (Some("turn_context"), _) => {
            if let Some(model) = payload["model"].as_str() {
                read.model = model.to_owned();
            }
        }
        (Some("event_msg"), Some("task_started")) => {
            read.request = payload["turn_id"].as_str().unwrap_or_default().to_owned();
        }
        (Some("event_msg"), Some("token_count")) => {
            let Some(at) = value["timestamp"].as_str().and_then(epoch) else {
                return;
            };
            let usage = &payload["info"]["last_token_usage"];
            let count = |key: &str| usage[key].as_u64().unwrap_or(0);
            let input = count("input_tokens");
            let cached = count("cached_input_tokens");
            let output = count("output_tokens");
            if input + output == 0 {
                return;
            }
            read.calls.push(SubscriptionCall {
                agent: Agent::Codex,
                at,
                model: read.model.clone(),
                session: read.session.clone(),
                request: read.request.clone(),
                folder: read.folder.clone(),
                by_universe: read.by_universe,
                // Cached input is part of the input, as OpenAI counts it.
                input: input.saturating_sub(cached),
                cache_read: cached,
                cache_written: count("cache_write_input_tokens"),
                output,
                quota: codex_quota(&payload["rate_limits"], at),
            });
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn times_are_read_as_both_agents_write_them() {
        assert_eq!(epoch("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch("2000-03-01T00:00:00.000Z"), Some(951_868_800));
        assert_eq!(epoch("2026-10-17T19:24:29.123Z"), Some(1_792_265_069));
        assert_eq!(epoch("yesterday"), None);
    }

    #[test]
    fn each_agent_says_its_quota_its_own_way() {
        let claude = claude_quota(
            &json!({"status": "allowed", "unifiedWindows": {
                "seven_day": {"utilization": 0.01, "resetsAt": 1_792_674_000},
                "five_hour": {"utilization": 0.11, "resetsAt": 1_792_282_200},
                "unknown_kind": {"utilization": 0.5, "resetsAt": 1}
            }}),
            100,
        )
        .unwrap();
        let shown: Vec<(&str, u64, u64)> = claude
            .windows
            .iter()
            .map(|w| (w.name.as_str(), w.minutes, w.used.round() as u64))
            .collect();
        assert_eq!(shown, [("five_hour", 300, 11), ("seven_day", 10_080, 1)]);
        assert_eq!(claude.windows[0].resets_at, 1_792_282_200);

        let codex = codex_quota(
            &json!({"primary": {"used_percent": 7.0, "window_minutes": 300, "resets_at": 5},
                    "secondary": {"used_percent": 2.0, "window_minutes": 10080, "resets_at": 9},
                    "plan_type": "plus"}),
            100,
        )
        .unwrap();
        assert_eq!(codex.plan.as_deref(), Some("plus"));
        assert_eq!(codex.windows[1].used, 2.0);
        assert!(codex_quota(&json!({}), 100).is_none());
    }

    #[test]
    fn a_claude_session_file_gives_each_message_once_under_its_request() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        let lines = [
            json!({"type": "user", "uuid": "u-1", "sessionId": "s-1", "cwd": "/p", "entrypoint": "sdk-cli",
                   "message": {"content": "do it"}}),
            json!({"type": "assistant", "timestamp": "1970-01-01T00:01:40Z",
                   "message": {"id": "m-1", "model": "claude-x",
                               "usage": {"input_tokens": 2, "cache_read_input_tokens": 900,
                                         "cache_creation_input_tokens": 100, "output_tokens": 10}}}),
            // The same message again, for its next block of content.
            json!({"type": "assistant", "timestamp": "1970-01-01T00:01:41Z",
                   "message": {"id": "m-1", "model": "claude-x",
                               "usage": {"input_tokens": 2, "output_tokens": 10}}}),
            // A tool's result is no new request.
            json!({"type": "user", "uuid": "u-2", "message": {"content": [{"type": "tool_result"}]}}),
            json!({"type": "assistant", "timestamp": "1970-01-01T00:01:50Z",
                   "message": {"id": "m-2", "model": "claude-x", "usage": {"input_tokens": 1, "output_tokens": 3}}}),
        ];
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(&path, text).unwrap();
        let mut read = Read::default();
        read_on(Agent::ClaudeCode, &path, &mut read);
        let calls: Vec<(&str, &str, u64, u64, bool)> = read
            .calls
            .iter()
            .map(|c| {
                (
                    c.request.as_str(),
                    c.session.as_str(),
                    c.at,
                    c.cache_read,
                    c.by_universe,
                )
            })
            .collect();
        assert_eq!(
            calls,
            [("u-1", "s-1", 100, 900, true), ("u-1", "s-1", 110, 0, true)]
        );
        // Read again, only what was added is read.
        let more = json!({"type": "assistant", "timestamp": "1970-01-01T00:02:00Z",
                          "message": {"id": "m-3", "model": "claude-x", "usage": {"output_tokens": 1}}});
        std::fs::write(
            &path,
            format!("{}{more}\n", std::fs::read_to_string(&path).unwrap()),
        )
        .unwrap();
        read_on(Agent::ClaudeCode, &path, &mut read);
        assert_eq!(read.calls.len(), 3);
    }

    #[test]
    fn a_codex_session_file_gives_each_call_with_its_model_request_and_quota() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout.jsonl");
        let lines = [
            json!({"type": "session_meta", "payload": {"id": "s-9", "cwd": "/p", "originator": "codex-tui"}}),
            json!({"type": "event_msg", "payload": {"type": "task_started", "turn_id": "t-1"}}),
            json!({"type": "turn_context", "payload": {"model": "gpt-x"}}),
            json!({"type": "event_msg", "timestamp": "1970-01-01T00:01:40.5Z", "payload": {
                "type": "token_count",
                "info": {"last_token_usage": {"input_tokens": 1000, "cached_input_tokens": 800,
                                              "output_tokens": 50}},
                "rate_limits": {"primary": {"used_percent": 3.0, "window_minutes": 300, "resets_at": 5000},
                                "plan_type": "pro"}}}),
        ];
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(&path, text).unwrap();
        let mut read = Read::default();
        read_on(Agent::Codex, &path, &mut read);
        let call = &read.calls[0];
        assert_eq!(
            (
                call.model.as_str(),
                call.request.as_str(),
                call.input,
                call.cache_read,
                call.at
            ),
            ("gpt-x", "t-1", 200, 800, 100)
        );
        assert!(!call.by_universe);
        let quota = call.quota.as_ref().unwrap();
        assert_eq!(
            (quota.plan.as_deref(), quota.windows[0].used),
            (Some("pro"), 3.0)
        );
    }
}
