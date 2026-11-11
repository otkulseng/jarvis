//! What the page knows about jarvis, and what each change means.
//!
//! `Model::apply` is the one place where a change from jarvis is turned into
//! what browsers are told. To make the page react to a new kind of change,
//! add an arm there (and a handler of the same name in
//! `web/src/lib/live/store.svelte.ts`). Nothing here does input or output, so
//! all of it can be tested: see the tests at the end.
//!
//! Browsers are told things in *frames*, JSON objects with a `type`:
//!
//!   process  {"process": {...}}        a process, as `jarvis ps --json` shows it
//!   gone     {"name": NAME}            a process was removed
//!   edge     {"edge": {a, b, count, talk, waiting, last_at}}
//!                                      how much two have written to each other.
//!                                      `talk` leaves out what jarvis wrote on
//!                                      someone's behalf
//!   message  {"message": {...}}        a new message, or one whose status changed
//!   fact     {"fact": {...}}           something the kernel recorded, steps included
//!   pulse    {"from": A, "to": B}      a message just went from A to B (for the map)
//!   thread   {"topic", "messages", "facts", "more"}
//!                                      the history of a topic that was just opened
//!                                      (made in `threads.rs`)
//!
//! `message` and `fact` frames also carry `topics`: the threads they belong in.
//!
//! Every frame goes to *topics*, and a browser gets the frames of the topics
//! it asked for:
//!
//!   map              every process and every edge
//!   inbox            messages to the human
//!   process:NAME     the messages to and from NAME, and the facts about it
//!   edge:A|B         the messages between A and B (A before B in the alphabet)

use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

pub const HUMAN: &str = "human";

/// Something to tell browsers.
#[derive(Debug, Clone, PartialEq)]
pub struct Out {
    pub topics: Vec<String>,
    /// Frames with the same key replace each other while they wait to be
    /// sent: only the newest state of a process or an edge is worth sending.
    pub key: Option<String>,
    pub frame: Value,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Edge {
    pub count: i64,
    /// Written by jarvis on someone's behalf ("you have just been created").
    pub by_jarvis: i64,
    pub waiting: i64,
    pub last_at: i64,
    /// Messages up to this one were counted when the page started.
    pub counted_to: i64,
}

#[derive(Default)]
pub struct Model {
    pub processes: BTreeMap<String, Value>,
    pub edges: BTreeMap<(String, String), Edge>,
    /// The messages not dealt with yet, and the edge each is on.
    open: HashMap<i64, (String, String)>,
}

pub fn pair(a: &str, b: &str) -> (String, String) {
    if a <= b { (a.to_string(), b.to_string()) } else { (b.to_string(), a.to_string()) }
}

pub fn process_topic(name: &str) -> String {
    format!("process:{name}")
}

pub fn edge_topic(a: &str, b: &str) -> String {
    let (a, b) = pair(a, b);
    format!("edge:{a}|{b}")
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or("")
}

impl Model {
    /// Start the edges from what `jarvis messages --pairs --json` and
    /// `jarvis messages --waiting --json` printed.
    pub fn load(&mut self, pairs: &[Value], waiting: &[Value]) {
        self.edges.clear();
        self.open.clear();
        for p in pairs {
            let n = |key: &str| p[key].as_i64().unwrap_or(0);
            self.edges.insert(
                pair(text(p, "a"), text(p, "b")),
                Edge { count: n("count"), by_jarvis: n("by_jarvis"), waiting: 0, last_at: n("last_at"), counted_to: n("last_id") },
            );
        }
        for m in waiting {
            self.set_open(m);
        }
    }

    /// Note whether a message is waiting. Safe to do twice.
    fn set_open(&mut self, message: &Value) {
        let Some(id) = message["id"].as_i64() else { return };
        let key = pair(text(message, "from"), text(message, "to"));
        let waiting = text(message, "status") != "handled";
        match (waiting, self.open.contains_key(&id)) {
            (true, false) => {
                self.edges.entry(key.clone()).or_default().waiting += 1;
                self.open.insert(id, key);
            }
            (false, true) => {
                if let Some(edge) = self.open.remove(&id).and_then(|key| self.edges.get_mut(&key)) {
                    edge.waiting = (edge.waiting - 1).max(0);
                }
            }
            _ => {}
        }
    }

    fn exists(&self, who: &str) -> bool {
        who == HUMAN || self.processes.contains_key(who)
    }

    pub fn edge_frame(&self, key: &(String, String)) -> Option<Out> {
        let edge = self.edges.get(key)?;
        // A line is drawn only between two that are there.
        if !self.exists(&key.0) || !self.exists(&key.1) {
            return None;
        }
        Some(Out {
            topics: vec!["map".into()],
            key: Some(format!("edge:{}|{}", key.0, key.1)),
            frame: json!({"type": "edge", "edge": {
                "a": key.0, "b": key.1, "count": edge.count, "talk": edge.count - edge.by_jarvis,
                "waiting": edge.waiting, "last_at": edge.last_at,
            }}),
        })
    }

    pub fn process_frame(process: &Value) -> Out {
        Out {
            topics: vec!["map".into()],
            key: Some(format!("process:{}", text(process, "id"))),
            frame: json!({"type": "process", "process": process}),
        }
    }

    /// Everything on the map, for a browser that has just connected.
    pub fn map(&self) -> Vec<Value> {
        let processes = self.processes.values().map(|p| Self::process_frame(p).frame);
        let edges = self.edges.keys().filter_map(|key| self.edge_frame(key)).map(|out| out.frame);
        processes.chain(edges).collect()
    }

    /// Who is told about a message: both ends, the line between them, and
    /// the inbox if it is for the human.
    fn message_out(message: &Value) -> Out {
        let (from, to) = (text(message, "from"), text(message, "to"));
        let mut topics = vec![process_topic(from), process_topic(to), edge_topic(from, to)];
        if to == HUMAN {
            topics.push("inbox".into());
        }
        let frame = json!({"type": "message", "topics": topics, "message": message});
        Out { topics, key: None, frame }
    }

    /// One change from `jarvis watch`, turned into what browsers are told.
    pub fn apply(&mut self, change: &Value) -> Vec<Out> {
        let mut out = Vec::new();
        match text(change, "kind") {
            "process" => {
                let process = &change["process"];
                let name = text(process, "id").to_string();
                let is_new = self.processes.insert(name.clone(), process.clone()).is_none();
                out.push(Self::process_frame(process));
                // Lines to it can be drawn now that it is there.
                if is_new {
                    let keys: Vec<_> = self.edges.keys().filter(|k| k.0 == name || k.1 == name).cloned().collect();
                    out.extend(keys.iter().filter_map(|key| self.edge_frame(key)));
                }
            }
            "gone" => {
                let name = text(change, "name").to_string();
                self.processes.remove(&name);
                self.edges.retain(|key, _| key.0 != name && key.1 != name);
                self.open.retain(|_, key| key.0 != name && key.1 != name);
                out.push(Out { topics: vec!["map".into()], key: None, frame: json!({"type": "gone", "name": name}) });
            }
            "fact" => {
                let fact = &change["fact"];
                let message = &change["message"];
                if text(fact, "kind") == "send" && message.is_object() {
                    let key = pair(text(message, "from"), text(message, "to"));
                    let id = message["id"].as_i64().unwrap_or(0);
                    let edge = self.edges.entry(key.clone()).or_default();
                    // A message counted when the page started is not counted again.
                    if id > edge.counted_to {
                        edge.count += 1;
                        edge.last_at = edge.last_at.max(message["at"].as_i64().unwrap_or(0));
                        if text(message, "body").starts_with("[jarvis]") {
                            edge.by_jarvis += 1;
                        }
                    }
                    self.set_open(message);
                    out.push(Self::message_out(message));
                    out.extend(self.edge_frame(&key));
                    // For the map: something just went from one to the other.
                    let (from, to) = (text(message, "from"), text(message, "to"));
                    out.push(Out {
                        topics: vec!["map".into()],
                        key: Some(format!("pulse:{from}>{to}")),
                        frame: json!({"type": "pulse", "from": from, "to": to}),
                    });
                }
                let topics = vec![process_topic(text(fact, "proc"))];
                let frame = json!({"type": "fact", "topics": topics, "fact": fact});
                out.push(Out { topics, key: None, frame });
            }
            "message" => {
                let message = &change["message"];
                self.set_open(message);
                out.push(Self::message_out(message));
                out.extend(self.edge_frame(&pair(text(message, "from"), text(message, "to"))));
            }
            // `ready`, `restart` and kinds this page does not know yet.
            _ => {}
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(name: &str) -> Value {
        json!({"kind": "process", "process": {"id": name, "state": "idle"}})
    }

    fn send(id: i64, from: &str, to: &str, status: &str, body: &str) -> Value {
        json!({"kind": "fact",
               "fact": {"id": id * 10, "proc": from, "kind": "send", "data": {"message": id, "to": to}},
               "message": {"id": id, "at": id, "from": from, "to": to, "body": body, "status": status}})
    }

    fn edge(model: &Model, a: &str, b: &str) -> Edge {
        model.edges.get(&pair(a, b)).cloned().unwrap_or_default()
    }

    #[test]
    fn a_message_is_counted_and_told_to_both_ends_and_the_map() {
        let mut model = Model::default();
        model.apply(&process("a"));
        model.apply(&process("b"));
        let out = model.apply(&send(1, "b", "a", "pending", "hi"));
        assert_eq!(edge(&model, "a", "b"), Edge { count: 1, by_jarvis: 0, waiting: 1, last_at: 1, counted_to: 0 });
        assert_eq!(out[0].topics, vec!["process:b", "process:a", "edge:a|b"]);
        assert_eq!(out[1].frame["edge"]["waiting"], 1);
        assert_eq!(out[2].frame["type"], "pulse");
        assert_eq!(out[3].topics, vec!["process:b"]);
    }

    #[test]
    fn a_status_change_moves_the_waiting_count_once() {
        let mut model = Model::default();
        model.apply(&process("a"));
        model.apply(&send(1, HUMAN, "a", "pending", "hi"));
        let taken = json!({"kind": "message", "message": {"id": 1, "from": HUMAN, "to": "a", "status": "taken"}});
        let handled = json!({"kind": "message", "message": {"id": 1, "from": HUMAN, "to": "a", "status": "handled"}});
        model.apply(&taken);
        assert_eq!(edge(&model, "a", HUMAN).waiting, 1);
        model.apply(&handled);
        model.apply(&handled);
        assert_eq!(edge(&model, "a", HUMAN).waiting, 0);
    }

    #[test]
    fn what_was_counted_at_the_start_is_not_counted_again() {
        let mut model = Model::default();
        model.apply(&process("a"));
        model.load(
            &[json!({"a": "a", "b": HUMAN, "count": 5, "by_jarvis": 1, "waiting": 1, "last_at": 50, "last_id": 7})],
            &[json!({"id": 7, "from": HUMAN, "to": "a", "status": "pending"})],
        );
        model.apply(&send(7, HUMAN, "a", "pending", "seen before"));
        assert_eq!(edge(&model, "a", HUMAN).count, 5);
        assert_eq!(edge(&model, "a", HUMAN).waiting, 1);
        model.apply(&send(8, HUMAN, "a", "pending", "[jarvis] new"));
        let after = edge(&model, "a", HUMAN);
        assert_eq!((after.count, after.by_jarvis, after.waiting), (6, 2, 2));
    }

    #[test]
    fn mail_to_the_human_reaches_the_inbox() {
        let mut model = Model::default();
        model.apply(&process("a"));
        let out = model.apply(&send(1, "a", HUMAN, "handled", "done"));
        assert!(out[0].topics.contains(&"inbox".to_string()));
        assert_eq!(edge(&model, "a", HUMAN).waiting, 0);
    }

    #[test]
    fn a_removed_process_takes_its_lines_with_it() {
        let mut model = Model::default();
        model.apply(&process("a"));
        model.apply(&process("b"));
        model.apply(&send(1, "a", "b", "pending", "hi"));
        let out = model.apply(&json!({"kind": "gone", "name": "b"}));
        assert_eq!(out[0].frame["type"], "gone");
        assert!(model.edges.is_empty());
        assert!(model.map().iter().all(|frame| frame["type"] == "process"));
    }

    /// Many events must stay cheap: each one is a few map lookups, whatever came before.
    #[test]
    fn two_hundred_thousand_messages_are_applied_in_well_under_a_second() {
        let mut model = Model::default();
        let names: Vec<String> = (0..200).map(|i| format!("p{i}")).collect();
        for name in &names {
            model.apply(&process(name));
        }
        let changes: Vec<Value> =
            (0..200_000).map(|i| send(i as i64 + 1, &names[i % 200], &names[(i * 7 + 1) % 200], "handled", "x")).collect();
        let began = std::time::Instant::now();
        let mut frames = 0;
        for change in &changes {
            frames += model.apply(change).len();
        }
        let took = began.elapsed();
        assert_eq!(frames, 800_000);
        assert_eq!(model.edges.values().map(|e| e.count).sum::<i64>(), 200_000);
        assert!(took.as_millis() < 3000, "took {took:?}");
    }

    #[test]
    fn an_unknown_kind_of_change_is_ignored() {
        let mut model = Model::default();
        assert!(model.apply(&json!({"kind": "something new"})).is_empty());
    }
}
