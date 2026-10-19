//! The behaviour a model writes for a simulated back end, beyond what the
//! spec's paths say: a JavaScript program that answers some operations as
//! the person asked (a checkout confirmed makes an order) or bends their
//! default answers, and changes what the back end holds. It is kept in a
//! file of universe's folder, read again whenever it changes, so that it
//! outlives a crash and can be changed by hand.
//!
//! It runs in an engine of its own, QuickJS, which has no access to files,
//! the network or processes, for a short time and a little memory each
//! call: what a model wrote cannot reach the machine.
//!
//! ```js
//! on("POST /v1/checkouts/{id}/confirm", ({ params, answer }) => {
//!   const checkout = store.get(params.id);
//!   if (!checkout) return; // the default answer
//!   store.create("/v1/orders/", { product_id: checkout.product_id });
//!   store.change(params.id, { status: "confirmed" });
//!   return { ...answer, status: "confirmed" };
//! });
//! ```

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rquickjs::context::EvalOptions;
use rquickjs::{CatchResultExt, Context, Function, Runtime};
use serde_json::Value;

/// How long a handler may run, its store's work included.
const LONGEST: Duration = Duration::from_secs(2);

/// How much memory the program may hold.
const MEMORY: usize = 64 * 1024 * 1024;

/// The name the program's errors give it, with their line.
const NAME: &str = "back-end.js";

/// How deep its calls may go.
const STACK: usize = 1024 * 1024;

/// What the program is given beyond the language: `on`, `reply`, `store`
/// and `console.log`. What it calls the store with, and what the store
/// answers, cross as JSON.
const PRELUDE: &str = r#""use strict";
const __handlers = {};
const __reply = Symbol("reply");
function on(operation, handler) {
  if (typeof operation !== "string" || typeof handler !== "function") {
    throw new TypeError('on(operation, handler): an operation such as "POST /v1/orders/", and a function');
  }
  __handlers[operation.trim().replace(/\s+/, " ")] = handler;
}
function reply(status, body) {
  return { [__reply]: true, status, body };
}
function __ask(operation, ...args) {
  const answer = JSON.parse(__store(operation, JSON.stringify(args)));
  if ("error" in answer) throw new Error(answer.error);
  return answer.value;
}
const store = Object.freeze({
  get: (id) => __ask("get", id),
  list: (path) => __ask("list", path),
  create: (path, fields) => __ask("create", path, fields ?? {}),
  change: (id, fields) => __ask("change", id, fields ?? {}),
  delete: (id) => __ask("delete", id),
});
globalThis.console = Object.freeze({
  log: (...parts) => __say(parts.map((p) => (typeof p === "string" ? p : JSON.stringify(p))).join(" ")),
});
function __run(operation, context) {
  const handler = __handlers[operation];
  if (handler === undefined) return JSON.stringify({ pass: true });
  const out = handler(JSON.parse(context));
  if (out instanceof Promise) throw new Error("A handler answers at once: it may not be async");
  if (out === undefined) return JSON.stringify({ pass: true });
  if (out !== null && typeof out === "object" && out[__reply]) {
    return JSON.stringify({ status: out.status, body: out.body === undefined ? null : out.body });
  }
  return JSON.stringify({ body: out });
}
"#;

/// What the program is told it can do, for the model that writes it.
pub const API: &str = r#"The program is JavaScript (ES2023), run as a script in a sandbox: no import, no require, no fetch, no timers, no files. Each call may run 2 seconds at most. It declares handlers:

on("POST /v1/checkouts/{id}/confirm", (call) => { ... })

- The operation is a method and a path exactly as the spec writes them, parameters in braces.
- call.method, call.path (as asked), call.params ({ id: "..." }), call.query ({ limit: "10" }), call.body (the request's JSON, or null), call.status (the default status), call.answer (the default answer: made from the schema, with what the back end holds laid over it).
- Return nothing to give the default answer; return a value to answer it with the default status; return reply(status, body) for another status. An answer must hold to the operation's schema for its status, or the default is given instead.
- A handler answers at once: no async, no Promise.

The back end holds every item by its id, built up as the page goes: those lists showed, those created, changed and deleted. What a handler does there shows in every later answer, wherever the item appears:
- store.get(id): the item as it reads now, or null when it was never seen.
- store.list(path): the items GET path answers now, query included ("/v1/orders/?customer_id=...").
- store.create(path, fields): creates an item in the list of path ("/v1/orders/"), as a POST there would, with these fields and an id of its own; returns it.
- store.change(id, fields): changes fields of an item, merged into it; returns it.
- store.delete(id): deletes an item: it leaves every list.
- console.log(...): a line in the App view's log.

A handler must not throw when an item is unknown: return nothing, and the default answer is given. Keep what earlier handlers did unless asked otherwise."#;

/// What the behaviour written for the back end did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Written {
    /// It was read: the operations it handles, and those it names that the
    /// spec has not.
    Loaded {
        /// The operations it handles, as it writes them.
        handled: Vec<String>,
        /// Those the spec has not.
        unknown: Vec<String>,
    },
    /// It does not run, and why.
    Broken(String),
    /// A handler failed, or answered what its schema refuses: the default
    /// answer was given, and why.
    Refused {
        /// The operation, as the program writes it.
        operation: String,
        /// Why.
        why: String,
    },
    /// A line it logged.
    Said(String),
}

/// What the store of the back end does when the program asks: its
/// operation (`get`, `list`, `create`, `change`, `delete`) and its
/// arguments, and what it answers or why it cannot.
pub(crate) type Store = Arc<dyn Fn(&str, Vec<Value>) -> Result<Value, String> + Send + Sync>;

/// Where what the program logs goes.
pub(crate) type Says = Arc<dyn Fn(String) + Send + Sync>;

/// What a handler answered.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Outcome {
    /// Nothing: the default answer.
    Pass,
    /// This body, with this status or the default one.
    Answer {
        /// The status it chose.
        status: Option<u16>,
        /// The body.
        body: Value,
    },
}

enum Job {
    Load(String, Sender<Result<Vec<String>, String>>),
    Run {
        operation: String,
        call: Value,
        reply: Sender<Result<Outcome, String>>,
    },
}

/// The engine a program runs in, on a thread of its own, one call at a
/// time; stopped when dropped.
pub(crate) struct Engine {
    jobs: Option<Sender<Job>>,
    thread: Option<JoinHandle<()>>,
}

impl Engine {
    /// Starts an engine with no program, its store `store`, its log `says`.
    pub(crate) fn start(store: Store, says: Says) -> Self {
        let (jobs, taken) = mpsc::channel();
        let thread = std::thread::spawn(move || serve(&taken, &store, &says));
        Self {
            jobs: Some(jobs),
            thread: Some(thread),
        }
    }

    /// Runs `program` in place of the one before: the operations it
    /// declares handlers for, as it writes them.
    ///
    /// # Errors
    ///
    /// Why it does not run: the exception it throws, where.
    pub(crate) fn load(&self, program: &str) -> Result<Vec<String>, String> {
        let (reply, answer) = mpsc::channel();
        self.send(Job::Load(program.to_owned(), reply))?;
        answer.recv().map_err(|_| "The engine stopped".to_owned())?
    }

    /// What the handler of `operation` answers `call`.
    ///
    /// # Errors
    ///
    /// Why it answered nothing: what it threw, or that it ran too long.
    pub(crate) fn run(&self, operation: &str, call: Value) -> Result<Outcome, String> {
        let (reply, answer) = mpsc::channel();
        self.send(Job::Run {
            operation: operation.to_owned(),
            call,
            reply,
        })?;
        answer
            .recv_timeout(LONGEST * 3)
            .map_err(|_| "The engine did not answer".to_owned())?
    }

    fn send(&self, job: Job) -> Result<(), String> {
        self.jobs
            .as_ref()
            .and_then(|jobs| jobs.send(job).ok())
            .ok_or_else(|| "The engine stopped".to_owned())
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // Its thread stops once it has no one to take jobs from.
        self.jobs = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// A program running: its context before its runtime, which outlives it.
struct Loaded {
    context: Context,
    _runtime: Runtime,
}

fn serve(jobs: &Receiver<Job>, store: &Store, says: &Says) {
    let deadline: Rc<Cell<Option<Instant>>> = Rc::default();
    let mut loaded: Option<Loaded> = None;
    while let Ok(job) = jobs.recv() {
        match job {
            Job::Load(program, reply) => {
                let made = fresh(&program, store, says, &deadline);
                let handled = made.map(|(made, handled)| {
                    loaded = Some(made);
                    handled
                });
                let _ = reply.send(handled);
            }
            Job::Run {
                operation,
                call,
                reply,
            } => {
                let Some(program) = &loaded else {
                    let _ = reply.send(Ok(Outcome::Pass));
                    continue;
                };
                deadline.set(Some(Instant::now() + LONGEST));
                let outcome = program.context.with(|ctx| {
                    let run: Function = ctx.globals().get("__run").map_err(|e| e.to_string())?;
                    let out: String = run
                        .call((operation, call.to_string()))
                        .catch(&ctx)
                        .map_err(|e| said(&e.to_string(), &deadline))?;
                    outcome(&out)
                });
                deadline.set(None);
                let _ = reply.send(outcome);
            }
        }
    }
}

/// A program run afresh, in a runtime of its own: it and the operations
/// it declares handlers for.
fn fresh(
    program: &str,
    store: &Store,
    says: &Says,
    deadline: &Rc<Cell<Option<Instant>>>,
) -> Result<(Loaded, Vec<String>), String> {
    let runtime = Runtime::new().map_err(|e| e.to_string())?;
    runtime.set_memory_limit(MEMORY);
    runtime.set_max_stack_size(STACK);
    let watched = Rc::clone(deadline);
    runtime.set_interrupt_handler(Some(Box::new(move || {
        watched.get().is_some_and(|d| Instant::now() > d)
    })));
    let context = Context::full(&runtime).map_err(|e| e.to_string())?;
    deadline.set(Some(Instant::now() + LONGEST));
    let handled = context.with(|ctx| -> Result<Vec<String>, String> {
        let globals = ctx.globals();
        let asked = Arc::clone(store);
        let ask = Function::new(ctx.clone(), move |operation: String, arguments: String| {
            let arguments = serde_json::from_str::<Vec<Value>>(&arguments).unwrap_or_default();
            let answer = match asked(&operation, arguments) {
                Ok(value) => serde_json::json!({ "value": value }),
                Err(why) => serde_json::json!({ "error": why }),
            };
            answer.to_string()
        })
        .map_err(|e| e.to_string())?;
        globals.set("__store", ask).map_err(|e| e.to_string())?;
        let logged = Arc::clone(says);
        let say = Function::new(ctx.clone(), move |text: String| logged(text))
            .map_err(|e| e.to_string())?;
        globals.set("__say", say).map_err(|e| e.to_string())?;
        ctx.eval::<(), _>(PRELUDE)
            .catch(&ctx)
            .map_err(|e| e.to_string())?;
        let mut named = EvalOptions::default();
        named.filename = Some(NAME.to_owned());
        ctx.eval_with_options::<(), _>(program, named)
            .catch(&ctx)
            .map_err(|e| said(&e.to_string(), deadline))?;
        let keys: String = ctx
            .eval("JSON.stringify(Object.keys(__handlers))")
            .catch(&ctx)
            .map_err(|e| e.to_string())?;
        serde_json::from_str(&keys).map_err(|e| e.to_string())
    });
    deadline.set(None);
    handled.map(|handled| {
        (
            Loaded {
                context,
                _runtime: runtime,
            },
            handled,
        )
    })
}

/// What a handler answered, as the prelude writes it.
fn outcome(out: &str) -> Result<Outcome, String> {
    let out: Value = serde_json::from_str(out).map_err(|e| e.to_string())?;
    if out["pass"] == true {
        return Ok(Outcome::Pass);
    }
    let status = out["status"].as_u64().and_then(|s| u16::try_from(s).ok());
    if out.get("status").is_some_and(|s| !s.is_null()) && status.is_none() {
        return Err(format!(
            "reply() needs a status such as 404, not {}",
            out["status"]
        ));
    }
    Ok(Outcome::Answer {
        status,
        body: out["body"].clone(),
    })
}

/// An error as the person reads it: one that ran too long said so.
fn said(error: &str, deadline: &Rc<Cell<Option<Instant>>>) -> String {
    if deadline.get().is_some_and(|d| Instant::now() > d) {
        return format!("It ran longer than {} s and was stopped", LONGEST.as_secs());
    }
    error.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    fn engine() -> (Engine, Arc<Mutex<Vec<String>>>) {
        let said: Arc<Mutex<Vec<String>>> = Arc::default();
        let kept = Arc::clone(&said);
        let store: Store = Arc::new(|operation, arguments| match operation {
            "get" if arguments[0] == "known" => Ok(json!({"id": "known", "name": "Kept"})),
            "get" => Ok(Value::Null),
            "create" => Ok(json!({"id": "new", "fields": arguments[1]})),
            _ => Err(format!("no {operation}")),
        });
        let says: Says = Arc::new(move |line| kept.lock().unwrap().push(line));
        (Engine::start(store, says), said)
    }

    #[test]
    fn a_program_answers_what_it_handles_and_nothing_else() {
        let (engine, said) = engine();
        let handled = engine
            .load(
                r#"
                on("GET /v1/things/{id}", ({ params, answer }) => {
                  const thing = store.get(params.id);
                  if (!thing) return;
                  console.log("found", thing);
                  return { ...answer, name: thing.name };
                });
                on("POST  /v1/things/", ({ body }) => reply(201, store.create("/v1/things/", body)));
                on("DELETE /v1/things/{id}", () => store.delete("x"));
                "#,
            )
            .unwrap();
        assert_eq!(
            handled,
            [
                "GET /v1/things/{id}",
                "POST /v1/things/",
                "DELETE /v1/things/{id}"
            ]
        );
        let call = |params: Value| json!({"params": params, "answer": {"id": "a", "name": "Made"}, "body": {"name": "Mine"}});
        assert_eq!(
            engine
                .run("GET /v1/things/{id}", call(json!({"id": "known"})))
                .unwrap(),
            Outcome::Answer {
                status: None,
                body: json!({"id": "a", "name": "Kept"})
            }
        );
        assert_eq!(
            said.lock().unwrap().as_slice(),
            [r#"found {"id":"known","name":"Kept"}"#]
        );
        assert_eq!(
            engine
                .run("GET /v1/things/{id}", call(json!({"id": "other"})))
                .unwrap(),
            Outcome::Pass
        );
        assert_eq!(
            engine.run("POST /v1/things/", call(json!({}))).unwrap(),
            Outcome::Answer {
                status: Some(201),
                body: json!({"id": "new", "fields": {"name": "Mine"}})
            }
        );
        // What the store refuses is thrown, and said.
        let refused = engine
            .run("DELETE /v1/things/{id}", call(json!({})))
            .unwrap_err();
        assert!(refused.contains("no delete"), "{refused}");
        assert_eq!(
            engine.run("GET /v1/elsewhere", call(json!({}))).unwrap(),
            Outcome::Pass
        );
    }

    #[test]
    fn a_program_that_fails_says_why_and_one_that_loops_is_stopped() {
        let (engine, _) = engine();
        let broken = engine.load("\non('GET /x', () => {").unwrap_err();
        assert!(broken.contains("back-end.js:2"), "{broken}");
        engine
            .load(
                r#"
                on("GET /loop", () => { for (;;) {} });
                on("GET /throw", ({ answer }) => answer.missing.field);
                on("GET /async", async () => 1);
                on("GET /bad", () => reply("oops", {}));
                "#,
            )
            .unwrap();
        let stopped = engine.run("GET /loop", json!({})).unwrap_err();
        assert!(stopped.contains("longer than"), "{stopped}");
        // It runs on after being stopped.
        let thrown = engine.run("GET /throw", json!({"answer": {}})).unwrap_err();
        assert!(thrown.contains("undefined"), "{thrown}");
        assert!(
            engine
                .run("GET /async", json!({}))
                .unwrap_err()
                .contains("async")
        );
        assert!(
            engine
                .run("GET /bad", json!({}))
                .unwrap_err()
                .contains("status")
        );
        // Nothing of the machine is reachable.
        let reached = engine.load("require('fs')").unwrap_err();
        assert!(reached.contains("require"), "{reached}");
    }
}
