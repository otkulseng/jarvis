//! Everything that travels: messages, requests to other kernels, and what
//! kernels tell each other about them. Each is kept until the next kernel on
//! its way has confirmed it, and is taken once where it is for.

use super::*;

impl Kernel {
    /// Whether this kernel has ever heard of a kernel of that name.
    pub(super) fn knows(&self, kernel: &str) -> bool {
        self.table.contains_key(kernel) || self.db.query_row("SELECT 1 FROM kernels WHERE name = ?1", [kernel], |_| Ok(())).is_ok()
    }

    /// Why nothing can be asked of a kernel right now, and what to do.
    pub(super) fn out_of_reach(&self, kernel: &str) -> String {
        if self.knows(kernel) {
            format!("the kernel `{kernel}` cannot be reached from `{}` right now (`jarvis kernels` shows which can, `jarvis link` why a link is down)", self.me)
        } else {
            format!("no kernel called `{kernel}` is known to `{}` (`jarvis kernels` lists those that are; `jarvis link` connects to one)", self.me)
        }
    }

    /// Why nothing can be sent to a kernel, if so. Mail to a kernel that is
    /// only out of reach for now is accepted: it waits.
    pub(super) fn refuses(&self, kernel: &str) -> Option<String> {
        if kernel == self.me || self.table.contains_key(kernel) {
            return None;
        }
        if index::kernel_closed(&self.db, kernel) {
            return Some(format!(
                "the kernel `{kernel}` is taken to be gone (`jarvis kernels` shows who said so), so nothing is sent there. Should it be back, a link to it lifts that"
            ));
        }
        (!self.knows(kernel)).then(|| self.out_of_reach(kernel))
    }

    /// Someone takes another kernel to be gone: what was waiting here for it
    /// is given up, and whoever sent it is told.
    pub(super) fn close_kernel(&mut self, name: &str, by: &str) -> Res<Value> {
        if name == self.me {
            return Err("that is this kernel, and it is running. Ctrl-C where it runs stops it".into());
        }
        if self.table.contains_key(name) {
            return Err(format!("the kernel `{name}` can be reached right now, so it is not gone (`jarvis kernels`)").into());
        }
        if !self.knows(name) {
            return Err(self.out_of_reach(name).into());
        }
        let me = self.addr(KERNEL);
        self.rec(&me, "closed", json!({"kernel": name, "by": by}))?;
        let held: Vec<(String, String)> = {
            let mut stmt = self.db.prepare("SELECT id, payload FROM outbox WHERE dest = ?1")?;
            stmt.query_map([name], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?
        };
        let why = format!("the kernel `{name}` is taken to be gone (said {by})");
        let mut given_up = 0;
        for (id, payload) in held {
            self.db.execute("DELETE FROM outbox WHERE id = ?1", [&id])?;
            self.awaiting.remove(&id);
            self.paused.remove(&id);
            let item: Value = serde_json::from_str(&payload).unwrap_or(Value::Null);
            if item["kind"] != "receipt" {
                self.undeliverable(&id, &why)?;
                given_up += 1;
            }
        }
        Ok(said(vec![format!("{name} is taken to be gone; {given_up} message(s) that waited here for it were given up")], vec![]))
    }

    /// Check where a message is to go, or say why it cannot be sent there.
    pub(super) fn check_to(&self, to: &str) -> Res<()> {
        let (name, kernel) = index::address(to, "recipient")?;
        if kernel == self.me {
            if !index::is_person(to) && self.find(name)?.is_none() {
                return Err(format!("there is no process `{to}` (`jarvis ps @{}` lists the processes of this kernel)", self.me).into());
            }
            return Ok(());
        }
        match self.refuses(kernel) {
            Some(why) => Err(why.into()),
            None => Ok(()),
        }
    }

    pub(super) fn send(&mut self, from: &str, to: &str, body: &str) -> Res<Value> {
        self.check_to(to)?;
        let id = self.post(from, to, body)?;
        let mut notes = Vec::new();
        // If it will not be read soon, say why.
        if kernel_of(to) == self.me {
            if let Some(proc) = self.find(name_of(to))? {
                if proc.status != "alive" {
                    notes.push(format!("{to} is stopped; the message waits until `jarvis start {to}`"));
                } else if proc.failures > 0 {
                    notes.push(format!("the last {} turn(s) of {to} failed; the message waits while the kernel keeps trying (`jarvis ps {to}` shows why)", proc.failures));
                }
            }
        } else if !self.table.contains_key(kernel_of(to)) {
            notes.push(format!(
                "the kernel `{}` cannot be reached right now; the message waits here and goes on when it can (`jarvis link` shows why a link is down)",
                kernel_of(to)
            ));
        }
        Ok(json!({"o": [format!("sent message {id} to {to}")], "e": notes, "id": id, "to": to}))
    }

    /// Have another kernel carry out a command. It travels as a message to
    /// that kernel: kept on the way, carried out once.
    pub(super) fn request(&mut self, kernel: &str, by: &str, argv: &[String], extra: Value, outcome: Sender<Value>) -> Res<Value> {
        if let Some(why) = self.refuses(kernel) {
            return Err(why.into());
        }
        let mut request = json!({"argv": argv});
        for (key, value) in extra.as_object().into_iter().flatten() {
            request[key] = value.clone();
        }
        let id = self.post_item(by, json!({"from": by, "to": at(KERNEL, kernel), "body": argv.join(" "), "request": request}))?;
        self.waiters.insert(id.clone(), outcome);
        Ok(json!({"o": [id], "e": [], "reachable": self.table.contains_key(kernel)}))
    }

    /// Tell someone something on the kernel's own behalf. A process without
    /// an agent is not told: nobody there reads it.
    pub(super) fn tell(&mut self, to: &str, text: &str) -> Res<()> {
        if name_of(to) == KERNEL {
            return Ok(());
        }
        if kernel_of(to) == self.me && !index::is_person(to) {
            match self.find(name_of(to))? {
                Some(proc) if proc.harness != NO_AGENT => {}
                _ => return Ok(()),
            }
        }
        let me = self.addr(KERNEL);
        self.post(&me, to, text)?;
        Ok(())
    }

    /// Send a message: write it down, and deliver it or start it on its way.
    /// `to` is an address. Returns the message's name.
    pub(super) fn post(&mut self, from: &str, to: &str, body: &str) -> Res<String> {
        let mut message = json!({"from": from, "to": to, "body": body});
        // What its sender had been given when it wrote this: the turn it is in.
        if kernel_of(from) == self.me
            && let Some(turn) = self.live.get(name_of(from)).and_then(|l| l.turn) {
                message["turn"] = json!(turn);
            }
        self.post_item(from, message)
    }

    /// Write down something that is sent (a message, or a request to a
    /// kernel), give it its name, and deliver it or start it on its way.
    pub(super) fn post_item(&mut self, from: &str, mut item: Value) -> Res<String> {
        let id = format!("{}:{}", self.me, self.next_n);
        item["id"] = json!(id);
        let to = item["to"].as_str().unwrap_or("").to_string();
        self.rec(from, "send", item.clone())?;
        let dest = kernel_of(&to);
        if dest == self.me {
            if !index::is_person(&to) {
                self.tend(name_of(&to));
            }
        } else {
            item["at"] = json!(index::now_ms());
            self.db.execute("INSERT OR IGNORE INTO outbox (id, dest, payload) VALUES (?1, ?2, ?3)", params![id, dest, item.to_string()])?;
            self.pump_outbox();
        }
        Ok(id)
    }

    /// Tell the kernel a message came from how far it got here. It travels
    /// back like a message: kept on the way, taken once.
    pub(super) fn receipt(&mut self, message: &str, status: &str, extra: Value) {
        let Some((origin, _)) = message.split_once(':') else { return };
        if origin == self.me {
            return;
        }
        let id = format!("{message}#{status}");
        let mut item = json!({"kind": "receipt", "id": id, "to": at(KERNEL, origin), "from": self.addr(KERNEL), "message": message, "status": status});
        for (key, value) in extra.as_object().into_iter().flatten() {
            item[key] = value.clone();
        }
        match self.db.execute("INSERT OR IGNORE INTO outbox (id, dest, payload) VALUES (?1, ?2, ?3)", params![id, origin, item.to_string()]) {
            Ok(_) => self.pump_outbox(),
            Err(e) => self.problem(&format!("could not keep the word `{status}` about the message {message} for the kernel `{origin}`: {e}")),
        }
    }

    /// A message can never be delivered: write that down and let whoever
    /// sent it know, here or through the kernel it came from.
    pub(super) fn undeliverable(&mut self, id: &str, why: &str) -> Res<()> {
        let to: Option<String> = self.db.query_row("SELECT recipient FROM messages WHERE id = ?1", [id], |r| r.get(0)).ok();
        let about = to.unwrap_or_else(|| self.addr(KERNEL));
        self.rec(&about, "undeliverable", json!({"message": id, "why": why}))?;
        if id.split(':').next() == Some(self.me.as_str()) {
            self.came_back(id, why)
        } else {
            self.receipt(id, "undeliverable", json!({"result": why}));
            Ok(())
        }
    }

    /// A message sent from here could not be delivered: whoever sent it hears of it.
    pub(super) fn came_back(&mut self, id: &str, why: &str) -> Res<()> {
        if let Some(waiter) = self.waiters.remove(id)
            && waiter.send(json!({"code": 1, "o": [], "e": [why]})).is_ok() {
                return Ok(());
            }
        let Ok((sender, to)) = self.db.query_row("SELECT sender, recipient FROM messages WHERE id = ?1", [id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))) else {
            return Ok(());
        };
        self.tell(&sender, &format!("[jarvis] Your message {id} to {to} could not be delivered: {why}."))
    }

    /// The kernel a message went to says how far it got there.
    pub(super) fn take_receipt(&mut self, item: &Value) -> Res<()> {
        let (message, status) = (item["message"].as_str().unwrap_or(""), item["status"].as_str().unwrap_or(""));
        let Ok((sender, to, body)) = self.db.query_row("SELECT sender, recipient, body FROM messages WHERE id = ?1", [message], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        }) else {
            return Ok(());
        };
        self.rec(&sender, "receipt", json!({"message": message, "status": status, "result": item["result"], "by": item["from"]}))?;
        if status == "undeliverable" {
            return self.came_back(message, item["result"].as_str().unwrap_or("it could not be delivered"));
        }
        // What came of a request. A command that still waits for it prints
        // it; otherwise whoever asked is told.
        if item.get("code").is_some() {
            if let Some(waiter) = self.waiters.remove(message)
                && waiter.send(json!({"code": item["code"], "o": item["o"], "e": item["e"]})).is_ok() {
                    return Ok(());
                }
            let outcome = if status == "handled" { "was carried out" } else { "could not be carried out" };
            let result = item["result"].as_str().unwrap_or("").trim();
            self.tell(&sender, &format!("[jarvis] Your request {message} to the kernel `{}` (`jarvis {body}`) {outcome}{}", kernel_of(&to), if result.is_empty() { ".".to_string() } else { format!(":\n{result}") }))?;
        }
        Ok(())
    }

    /// A request arrived for this kernel: carry it out, on a thread of its
    /// own like any command.
    pub(super) fn carry_out(&mut self, item: &Value) {
        let (Some(id), Some(from)) = (item["id"].as_str(), item["from"].as_str()) else { return };
        let mut request = item["request"].clone();
        request["by"] = json!(from);
        let (cfg, tx, shared, id) = (self.cfg.clone(), self.tx.clone(), self.shared.clone(), id.to_string());
        thread::spawn(move || {
            let body = serve::answer(cfg, tx.clone(), shared, request);
            let _ = tx.send(Event::RequestDone { id, body });
        });
    }

    /// A request was carried out: write down what came of it and tell the
    /// kernel it came from.
    pub(super) fn request_done(&mut self, id: &str, body: &Value) {
        let code = body["code"].as_i64().unwrap_or(1);
        let lines = |key: &str| -> Vec<String> { body[key].as_array().map(|a| a.iter().filter_map(|l| l.as_str().map(str::to_string)).collect()).unwrap_or_default() };
        let result = [lines("o"), lines("e")].concat().join("\n");
        let me = self.addr(KERNEL);
        self.note(&me, "request_done", json!({"message": id, "code": code, "result": result}));
        self.receipt(id, if code == 0 { "handled" } else { "failed" }, json!({"code": code, "o": body["o"], "e": body["e"], "result": result}));
    }

    /// Hand what is held for other kernels to the links that lead there.
    pub(super) fn pump_outbox(&mut self) {
        let held: Vec<(String, String, String)> = {
            let Ok(mut stmt) = self.db.prepare_cached("SELECT id, dest, payload FROM outbox") else { return };
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map(|rows| rows.flatten().collect()).unwrap_or_default()
        };
        for (id, dest, payload) in held {
            if self.awaiting.contains_key(&id) || self.paused.contains(&id) {
                continue;
            }
            let Some(link) = self.table.get(&dest).map(|k| k.link) else { continue };
            let Ok(mut item) = serde_json::from_str::<Value>(&payload) else { continue };
            item["hops"] = json!(item["hops"].as_i64().unwrap_or(0) + 1);
            if self.say(link, json!({"t": "msg", "m": item})) {
                self.awaiting.insert(id, link);
            }
        }
    }

    /// Something arrived over a link: a message or a request for someone
    /// here, word about a message sent from here, or something to pass on.
    pub(super) fn receive(&mut self, link: u64, item: &Value) {
        let (Some(id), Some(to)) = (item["id"].as_str(), item["to"].as_str()) else { return };
        let is_receipt = item["kind"] == "receipt";
        let peer = self.links.get(&link).and_then(|l| l.peer.clone()).unwrap_or_default();
        let result = (|| -> Res<()> {
            let dest = kernel_of(to);
            if dest != self.me {
                if !self.table.contains_key(dest) && index::kernel_closed(&self.db, dest) {
                    if !is_receipt {
                        self.receipt(id, "undeliverable", json!({"result": format!("the kernel `{dest}` is taken to be gone, as seen from `{}`", self.me)}));
                    }
                    return Ok(());
                }
                // Kept until the next kernel on the way has confirmed it.
                let added = self.db.execute("INSERT OR IGNORE INTO outbox (id, dest, payload) VALUES (?1, ?2, ?3)", params![id, dest, item.to_string()])?;
                if added > 0 && item["hops"].as_i64().unwrap_or(0) > MOST_HOPS {
                    // It is going in circles: the kernels disagree, for now, about the way.
                    self.paused.insert(id.to_string());
                    let mut fresh = item.clone();
                    fresh["hops"] = json!(0);
                    self.db.execute("UPDATE outbox SET payload = ?2 WHERE id = ?1", params![id, fresh.to_string()])?;
                }
                return Ok(());
            }
            // What arrives a second time (its confirmation was lost) arrives once.
            if self.db.execute("INSERT OR IGNORE INTO seen (id) VALUES (?1)", [id])? == 0 {
                return Ok(());
            }
            if is_receipt {
                return self.take_receipt(item);
            }
            let mut arrived = item.clone();
            arrived["via"] = json!(peer);
            self.rec(to, "arrive", arrived)?;
            if item["request"].is_object() && name_of(to) == KERNEL {
                self.receipt(id, "pending", json!({}));
                self.carry_out(item);
            } else if index::is_person(to) {
                self.receipt(id, "handled", json!({}));
            } else if self.find(name_of(to))?.is_some() {
                self.receipt(id, "pending", json!({}));
                self.tend(name_of(to));
            } else {
                self.undeliverable(id, &format!("there is no process `{to}`"))?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.say(link, json!({"t": "ack", "id": id}));
                self.pump_outbox();
            }
            // Not confirmed: the other kernel keeps it and offers it again.
            Err(e) => self.problem(&format!("could not take in `{id}` from the kernel `{peer}`: {e}. It was not confirmed, so `{peer}` keeps it and offers it again")),
        }
    }

    /// The next kernel has it: this one need not keep it any longer.
    pub(super) fn confirmed(&mut self, link: u64, id: &str) {
        if self.awaiting.get(id) != Some(&link) {
            return;
        }
        self.awaiting.remove(id);
        let payload: Option<String> = self.db.query_row("SELECT payload FROM outbox WHERE id = ?1", [id], |r| r.get(0)).ok();
        let _ = self.db.execute("DELETE FROM outbox WHERE id = ?1", [id]);
        let next = self.links.get(&link).and_then(|l| l.peer.clone()).unwrap_or_default();
        let item: Value = payload.and_then(|p| serde_json::from_str(&p).ok()).unwrap_or(Value::Null);
        // That a message went on is worth writing down; word about one is not.
        if item["kind"] != "receipt" {
            // About its sender if that is a process here; else it is this
            // kernel's own doing, passing on what came from elsewhere.
            let from = item["from"].as_str().unwrap_or("");
            let about = if kernel_of(from) == self.me { from.to_string() } else { self.addr(KERNEL) };
            self.note(&about, "forward", json!({"message": id, "next": next}));
        }
    }

    /// Write down a problem that is the kernel's own.
    pub(super) fn problem(&mut self, error: &str) {
        let me = self.addr(KERNEL);
        self.note(&me, "kernel_error", json!({"error": error}));
    }
}
