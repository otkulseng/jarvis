//! Links to other kernels: making them, which kernels can be reached through
//! them, and the questions and subscriptions that go over them.

use super::*;

impl Kernel {
    pub(super) fn link(&mut self, spec: &str, remove: bool) -> Res<Value> {
        if remove {
            if self.db.execute("DELETE FROM links WHERE spec = ?1", [spec])? == 0 {
                return Err("this kernel has no such link. Give it exactly as `jarvis link` lists it".into());
            }
            if let Some(id) = self.dials.remove(spec).and_then(|d| d.link)
                && let Some(link) = self.links.get(&id) {
                    (link.close)();
                }
            self.publish_net();
            return Ok(said(vec!["the link is forgotten".into()], vec![]));
        }
        if let Some(kernel) = serde_json::from_str::<Value>(spec)?["kernel"].as_str()
            && kernel == self.me {
                return Err("a kernel cannot be linked to itself".into());
            }
        self.db.execute("INSERT OR IGNORE INTO links (spec) VALUES (?1)", [spec])?;
        self.dials.entry(spec.to_string()).or_insert_with(Dial::new);
        self.publish_net();
        Ok(said(vec!["the link is being made; `jarvis link` shows how it is going".into()], vec![]))
    }

    /// Send one line over a link. False if the link is not there.
    pub(super) fn say(&self, link: u64, line: Value) -> bool {
        self.links.get(&link).is_some_and(|l| l.tx.send(line.to_string()).is_ok())
    }

    /// Send a line towards a kernel, over whichever link leads there.
    pub(super) fn route_to(&self, kernel: &str, line: Value) -> bool {
        self.table.get(kernel).is_some_and(|k| self.say(k.link, line))
    }

    pub(super) fn link_open(&mut self, id: u64, tx: Sender<String>, close: net::Close) {
        self.links.insert(id, Link { tx, close, peer: None, heard: Instant::now(), refused: false });
        self.say(id, json!({"t": "hello", "name": self.me, "identity": self.cfg.identity, "version": cli::VERSION}));
    }

    /// Say no to a link, and end it.
    pub(super) fn refuse(&mut self, id: u64, why: &str) {
        self.say(id, json!({"t": "refuse", "why": why}));
        self.link_problem(id, why);
        // Give the line a moment to leave before the link is closed.
        thread::sleep(Duration::from_millis(100));
        if let Some(link) = self.links.get(&id) {
            (link.close)();
        }
    }

    pub(super) fn link_problem(&mut self, id: u64, why: &str) {
        if let Some(link) = self.links.get_mut(&id) {
            link.refused = true;
        }
        if let Some(dial) = self.dials.values_mut().find(|d| d.link == Some(id)) {
            dial.problem = Some(why.to_string());
        }
        let me = self.addr(KERNEL);
        self.note(&me, "link_refused", json!({"why": why}));
    }

    /// Remember a kernel that can be reached. One that was taken to be gone
    /// and can be reached is there again.
    pub(super) fn know(&self, name: &str, identity: &str, version: &str) {
        let _ = self.db.execute(
            "INSERT INTO kernels (name, identity, version, last_reached) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(name) DO UPDATE SET identity = excluded.identity, version = excluded.version,
                 last_reached = excluded.last_reached, closed = 0, closed_by = NULL",
            params![name, identity, version, index::now_ms()],
        );
    }

    pub(super) fn link_line(&mut self, id: u64, v: Value) {
        let Some(link) = self.links.get_mut(&id) else { return };
        link.heard = Instant::now();
        let has_peer = link.peer.is_some();
        let text = |key: &str| v[key].as_str().unwrap_or("").to_string();
        match v["t"].as_str().unwrap_or("") {
            "hello" => {
                let (name, identity) = (text("name"), text("identity"));
                if name.is_empty() || has_peer {
                    return;
                }
                if name == self.me {
                    return self.refuse(id, &format!("both kernels are called `{name}`. Two kernels of one name cannot be linked: one of them must be started under a new name"));
                }
                if self.table.get(&name).is_some_and(|k| k.identity != identity) {
                    return self.refuse(id, &format!("another kernel called `{name}` can already be reached from `{}`. Two kernels of one name cannot be linked: one of them must be started under a new name", self.me));
                }
                if let Some(link) = self.links.get_mut(&id) {
                    link.peer = Some(name.clone());
                }
                if let Some(dial) = self.dials.values_mut().find(|d| d.link == Some(id)) {
                    dial.problem = None;
                    dial.tries = 0;
                    dial.wait = Duration::from_secs(2);
                }
                let me = self.addr(KERNEL);
                self.note(&me, "link_up", json!({"kernel": name, "version": text("version")}));
                // The kernel at the other end can be reached from now on.
                self.offers.entry(id).or_default().insert(name.clone(), Known { identity, hops: 1, link: id, version: text("version") });
                self.choose_routes();
                self.advertise();
            }
            "refuse" => {
                let why = v["why"].as_str().unwrap_or("the other kernel refused the link").to_string();
                self.link_problem(id, &why);
                if let Some(link) = self.links.get(&id) {
                    (link.close)();
                }
            }
            "table" if has_peer => self.learn(id, &v["kernels"]),
            "msg" if has_peer => self.receive(id, &v["m"]),
            "ack" => self.confirmed(id, v["id"].as_str().unwrap_or("")),
            // A question for a kernel: answered there, on a thread of its own.
            "req" => {
                let (rid, src, dst) = (text("rid"), text("src"), text("dst"));
                if dst == self.me {
                    let (cfg, tx, shared, body) = (self.cfg.clone(), self.tx.clone(), self.shared.clone(), v["body"].clone());
                    thread::spawn(move || {
                        let body = serve::answer(cfg, tx.clone(), shared, body);
                        let _ = tx.send(Event::Answered { rid, to: src, body });
                    });
                } else if !self.route_to(&dst, v.clone()) {
                    let body = json!({"o": [], "e": [format!("jarvis: {}", self.out_of_reach(&dst))], "code": 1});
                    self.route_to(&src, json!({"t": "res", "rid": rid, "dst": src, "body": body}));
                }
            }
            "res" => {
                let dst = text("dst");
                if dst == self.me {
                    if let Some((reply, _)) = self.asked.remove(&text("rid")) {
                        let _ = reply.send(Ok(v["body"].clone()));
                    }
                } else {
                    self.route_to(&dst, v.clone());
                }
            }
            // A kernel wants to follow another: what a command prints there
            // is sent back line by line, until either side ends it.
            "sub" => {
                let (sid, src, dst) = (text("sid"), text("src"), text("dst"));
                if dst == self.me {
                    let alive = Arc::new(AtomicBool::new(true));
                    self.pubs.insert(sid.clone(), (src.clone(), alive.clone()));
                    let (cfg, tx, shared, body) = (self.cfg.clone(), self.tx.clone(), self.shared.clone(), v["body"].clone());
                    thread::spawn(move || serve::publish(cfg, tx, shared, body, sid, src, alive));
                } else if !self.route_to(&dst, v.clone()) {
                    self.route_to(&src, json!({"t": "pub", "sid": sid, "src": dst, "dst": src, "end": 1, "why": self.out_of_reach(&dst)}));
                }
            }
            "pub" => {
                let (sid, src, dst) = (text("sid"), text("src"), text("dst"));
                let ended = v.get("end").is_some();
                if dst != self.me {
                    // Nobody to pass it to: the kernel that sends it can stop.
                    if !self.route_to(&dst, v.clone()) && !ended {
                        self.route_to(&src, json!({"t": "unsub", "sid": sid, "dst": src}));
                    }
                    return;
                }
                let delivered = self.subs.get(&sid).is_some_and(|(_, to)| to.send(v.clone()).is_ok());
                if !delivered || ended {
                    self.subs.remove(&sid);
                    if !ended {
                        self.route_to(&src, json!({"t": "unsub", "sid": sid, "dst": src}));
                    }
                }
            }
            "unsub" => {
                let dst = text("dst");
                if dst == self.me {
                    self.end_pub(&text("sid"));
                } else {
                    self.route_to(&dst, v.clone());
                }
            }
            _ => {}
        }
    }

    /// Nobody follows this kernel under that name any more: the command that
    /// fed it ends.
    pub(super) fn end_pub(&mut self, sid: &str) {
        if let Some((_, alive)) = self.pubs.remove(sid) {
            alive.store(false, Ordering::SeqCst);
            self.wake_followers();
        }
    }

    pub(super) fn link_down(&mut self, id: u64, why: &str) {
        let was = self.links.remove(&id);
        if let Some(link) = &was {
            (link.close)();
        }
        let came_up = was.as_ref().is_some_and(|l| l.peer.is_some());
        let refused = was.as_ref().is_some_and(|l| l.refused);
        let mut failing: Option<(String, String)> = None;
        if let Some((spec, dial)) = self.dials.iter_mut().find(|(_, d)| d.link == Some(id)) {
            dial.link = None;
            dial.next_try = Instant::now() + dial.wait;
            dial.wait = (dial.wait * 2).min(Duration::from_secs(60));
            dial.tries = if came_up { 0 } else { dial.tries + 1 };
            // Why a link that was refused went down is known already.
            if !refused {
                // A link that keeps failing for the same reason is written down once.
                if !came_up && dial.problem.as_deref() != Some(why) {
                    failing = Some((link_words(spec), why.to_string()));
                }
                dial.problem = Some(why.to_string());
            }
        }
        // What it was asked to carry and has not confirmed is offered again.
        self.awaiting.retain(|_, link| *link != id);
        let me = self.addr(KERNEL);
        if let Some(peer) = was.and_then(|l| l.peer) {
            self.note(&me, "link_down", json!({"kernel": peer, "why": why}));
        }
        if let Some((link, why)) = failing {
            self.note(&me, "link_failing", json!({"link": link, "why": why}));
        }
        self.offers.remove(&id);
        self.choose_routes();
        self.publish_net();
    }

    /// A neighbour said which kernels it can reach. Take over what is new or
    /// nearer, drop what it no longer offers.
    pub(super) fn learn(&mut self, id: u64, kernels: &Value) {
        let Some(entries) = kernels.as_array() else { return };
        let mut offered: HashMap<String, Known> = HashMap::new();
        for e in entries {
            let (Some(name), Some(identity)) = (e["name"].as_str(), e["identity"].as_str()) else { continue };
            let hops = e["hops"].as_u64().unwrap_or(FARTHEST) + 1;
            if name == self.me {
                if identity != self.cfg.identity {
                    return self.refuse(id, &format!("another kernel called `{name}` can be reached through this link. Two kernels of one name cannot be linked: one of them must be started under a new name"));
                }
                continue;
            }
            if self.table.get(name).is_some_and(|k| k.identity != identity) {
                return self.refuse(id, &format!("two different kernels called `{name}` would be reachable. Two kernels of one name cannot be linked: one of them must be started under a new name"));
            }
            if hops > FARTHEST {
                continue;
            }
            offered.insert(name.to_string(), Known { identity: identity.to_string(), hops, link: id, version: e["version"].as_str().unwrap_or("").to_string() });
        }
        self.offers.insert(id, offered);
        self.choose_routes();
    }

    /// For every kernel a neighbour offers, take the nearest way. Every
    /// offer is kept, so that when a link goes, a longer way that was there
    /// all along is used at once.
    pub(super) fn choose_routes(&mut self) {
        let mut best: HashMap<String, Known> = HashMap::new();
        let mut links: Vec<&u64> = self.offers.keys().collect();
        links.sort();
        for link in links {
            for (name, offer) in &self.offers[link] {
                if best.get(name).is_none_or(|b| offer.hops < b.hops) {
                    best.insert(name.clone(), Known { identity: offer.identity.clone(), hops: offer.hops, link: *link, version: offer.version.clone() });
                }
            }
        }
        let same = best.len() == self.table.len()
            && best.iter().all(|(name, new)| self.table.get(name).is_some_and(|old| old.link == new.link && old.hops == new.hops && old.version == new.version));
        if !same {
            self.table = best;
            self.routes_changed();
        }
    }

    /// The kernels that can be reached changed: remember them, tell the
    /// neighbours, try again what was waiting for a way, and end what
    /// depended on a kernel that is out of reach now.
    pub(super) fn routes_changed(&mut self) {
        for (name, known) in &self.table {
            self.know(name, &known.identity, &known.version);
        }
        self.paused.clear();
        // Dropping where a subscription's lines go tells whoever reads them.
        self.subs.retain(|_, (kernel, _)| self.table.contains_key(kernel.as_str()));
        let gone: Vec<String> = self.pubs.iter().filter(|(_, (kernel, _))| !self.table.contains_key(kernel)).map(|(sid, _)| sid.clone()).collect();
        for sid in gone {
            self.end_pub(&sid);
        }
        self.advertise();
        self.pump_outbox();
        self.wake_followers();
    }

    /// Tell every neighbour which kernels can be reached through this one.
    /// (Not those reached through the neighbour itself: it knows a shorter
    /// way to them.)
    pub(super) fn advertise(&mut self) {
        for (id, link) in &self.links {
            if link.peer.is_none() {
                continue;
            }
            let mut kernels = vec![json!({"name": self.me, "identity": self.cfg.identity, "hops": 0, "version": cli::VERSION})];
            for (name, k) in &self.table {
                if k.link != *id {
                    kernels.push(json!({"name": name, "identity": k.identity, "hops": k.hops, "version": k.version}));
                }
            }
            let _ = link.tx.send(json!({"t": "table", "kernels": kernels}).to_string());
        }
        self.publish_net();
    }

    /// Put what is known about kernels and links where commands can read it.
    pub(super) fn publish_net(&self) {
        let peer_of = |id: u64| self.links.get(&id).and_then(|l| l.peer.clone());
        let mut reachable: Vec<Reachable> = self
            .table
            .iter()
            .map(|(name, k)| Reachable { name: name.clone(), hops: k.hops, via: peer_of(k.link).unwrap_or_default(), version: k.version.clone() })
            .collect();
        reachable.sort_by(|a, b| (a.hops, &a.name).cmp(&(b.hops, &b.name)));
        let now = Instant::now();
        let mut links: Vec<LinkView> = self
            .dials
            .iter()
            .map(|(spec, d)| {
                let peer = d.link.and_then(peer_of);
                let waiting = peer.is_none() && d.link.is_none();
                LinkView { spec: Some(spec.clone()), peer, problem: d.problem.clone(), tries: d.tries, next_try_s: waiting.then(|| d.next_try.saturating_duration_since(now).as_secs()) }
            })
            .collect();
        let dialled: HashSet<u64> = self.dials.values().filter_map(|d| d.link).collect();
        links.extend(self.links.iter().filter(|(id, _)| !dialled.contains(id)).map(|(_, l)| LinkView { spec: None, peer: l.peer.clone(), problem: None, tries: 0, next_try_s: None }));
        *self.shared.net.lock().unwrap() = NetView { reachable, links };
    }
}

/// A link as it was made, in words: `to laptop on this machine`, `through ssh ...`.
pub fn link_words(spec: &str) -> String {
    let spec: Value = serde_json::from_str(spec).unwrap_or(Value::Null);
    match (spec["kernel"].as_str(), spec["command"].as_array()) {
        (Some(kernel), _) => format!("to `{kernel}` on this machine"),
        (None, Some(words)) => format!("through `{}`", words.iter().filter_map(|w| w.as_str()).collect::<Vec<_>>().join(" ")),
        _ => "made from the other end".to_string(),
    }
}
