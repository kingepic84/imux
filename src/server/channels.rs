//! `wait-for`: named channels that scripts use to wait for each other. `wait-for name` blocks
//! until `wait-for -S name` is run; `-L` and `-U` lock and unlock a channel, a second `-L` waiting
//! until the first holder unlocks. The state is plain data here; the server holds the connections
//! of the clients that wait and answers them when the channel says so.

use std::collections::{HashMap, VecDeque};

/// Different channels at once, and clients waiting on all of them together.
pub const MAX_CHANNELS: usize = 64;
pub const MAX_WAITING: usize = 64;
const MAX_NAME: usize = 64;

#[derive(Default)]
struct Channel {
    /// Clients blocked in `wait-for name`.
    waiting: Vec<usize>,
    /// A signal came when nobody waited: the next wait returns at once.
    signalled: bool,
    locked: bool,
    /// Clients blocked in `wait-for -L name`, first come first served.
    lockers: VecDeque<usize>,
}

impl Channel {
    fn idle(&self) -> bool {
        self.waiting.is_empty() && !self.signalled && !self.locked && self.lockers.is_empty()
    }
}

#[derive(Default)]
pub struct Channels {
    map: HashMap<String, Channel>,
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_NAME && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

impl Channels {
    fn channel(&mut self, name: &str) -> Result<&mut Channel, String> {
        if !valid_name(name) {
            return Err(format!("wait-for: bad channel name {name:?} (1 to 64 letters, digits, . _ -)"));
        }
        if !self.map.contains_key(name) && self.map.len() >= MAX_CHANNELS {
            return Err(format!("wait-for: too many channels ({MAX_CHANNELS})"));
        }
        Ok(self.map.entry(name.to_string()).or_default())
    }

    fn blocked(&self) -> usize {
        self.map.values().map(|c| c.waiting.len() + c.lockers.len()).sum()
    }

    fn tidy(&mut self, name: &str) {
        if self.map.get(name).is_some_and(Channel::idle) {
            self.map.remove(name);
        }
    }

    /// `wait-for name`. `Ok(true)` answers at once (a signal was waiting), `Ok(false)` leaves the
    /// client blocked until `signal`.
    pub fn wait(&mut self, name: &str, client: usize) -> Result<bool, String> {
        if self.blocked() >= MAX_WAITING {
            return Err(format!("wait-for: too many clients waiting ({MAX_WAITING})"));
        }
        let c = self.channel(name)?;
        if c.signalled {
            c.signalled = false;
            self.tidy(name);
            return Ok(true);
        }
        c.waiting.push(client);
        Ok(false)
    }

    /// `wait-for -S name`: every client waiting is let go; with none waiting the signal is kept
    /// for the next wait.
    pub fn signal(&mut self, name: &str) -> Result<Vec<usize>, String> {
        let c = self.channel(name)?;
        let woken = std::mem::take(&mut c.waiting);
        if woken.is_empty() {
            c.signalled = true;
        }
        Ok(woken)
    }

    /// `wait-for -L name`. `Ok(true)` when the lock is the caller's now, `Ok(false)` when it waits.
    pub fn lock(&mut self, name: &str, client: usize) -> Result<bool, String> {
        if self.blocked() >= MAX_WAITING {
            return Err(format!("wait-for: too many clients waiting ({MAX_WAITING})"));
        }
        let c = self.channel(name)?;
        if c.locked {
            c.lockers.push_back(client);
            return Ok(false);
        }
        c.locked = true;
        Ok(true)
    }

    /// `wait-for -U name`: the lock goes to the first client waiting for it, whose number is
    /// returned, else it is free.
    pub fn unlock(&mut self, name: &str) -> Result<Option<usize>, String> {
        if !valid_name(name) {
            return Err(format!("wait-for: bad channel name {name:?} (1 to 64 letters, digits, . _ -)"));
        }
        let Some(c) = self.map.get_mut(name).filter(|c| c.locked) else {
            return Err(format!("wait-for: channel {name} is not locked"));
        };
        let next = c.lockers.pop_front();
        c.locked = next.is_some();
        self.tidy(name);
        Ok(next)
    }

    /// A client went away: it stops waiting and its place in a queue is gone.
    pub fn drop_client(&mut self, client: usize) {
        for c in self.map.values_mut() {
            c.waiting.retain(|w| *w != client);
            c.lockers.retain(|w| *w != client);
        }
        self.map.retain(|_, c| !c.idle());
    }
}

impl super::Server {
    /// `wait-for [-L|-S|-U] channel`. A command that has to wait leaves its connection open
    /// (`deferred`) and is answered when the channel lets it go.
    pub(super) fn cmd_wait_for(&mut self, client: usize, args: &[String]) -> Result<String, String> {
        let f = super::remote::flags("wait-for", args, "", "LSU")?;
        let [name] = f.words.as_slice() else { return Err("usage: wait-for [-L|-S|-U] channel".to_string()) };
        if ['L', 'S', 'U'].iter().filter(|c| f.has(**c)).count() > 1 {
            return Err("wait-for: use only one of -L, -S and -U".to_string());
        }
        let blocked = if f.has('S') {
            for woken in self.channels.signal(name)? {
                self.answer_wait(woken);
            }
            false
        } else if f.has('U') {
            if let Some(next) = self.channels.unlock(name)? {
                self.answer_wait(next);
            }
            false
        } else if f.has('L') {
            !self.channels.lock(name, client)?
        } else {
            !self.channels.wait(name, client)?
        };
        self.deferred = blocked;
        Ok(String::new())
    }

    fn answer_wait(&mut self, client: usize) {
        if self.clients.contains_key(&client) {
            self.finish_request(client, Ok(String::new()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wait_is_answered_by_the_next_signal_for_everyone_waiting() {
        let mut ch = Channels::default();
        assert_eq!(ch.wait("done", 1), Ok(false));
        assert_eq!(ch.wait("done", 2), Ok(false));
        assert_eq!(ch.wait("other", 3), Ok(false));
        assert_eq!(ch.signal("done"), Ok(vec![1, 2]));
        assert_eq!(ch.signal("other"), Ok(vec![3]));
    }

    #[test]
    fn a_signal_nobody_heard_is_kept_for_the_next_wait_once() {
        let mut ch = Channels::default();
        assert_eq!(ch.signal("early"), Ok(vec![]));
        assert_eq!(ch.wait("early", 1), Ok(true));
        assert_eq!(ch.wait("early", 2), Ok(false), "the signal was used up");
    }

    #[test]
    fn the_lock_is_handed_on_in_order_and_unlocking_a_free_channel_is_an_error() {
        let mut ch = Channels::default();
        assert_eq!(ch.lock("m", 1), Ok(true));
        assert_eq!(ch.lock("m", 2), Ok(false));
        assert_eq!(ch.lock("m", 3), Ok(false));
        assert_eq!(ch.unlock("m"), Ok(Some(2)));
        assert_eq!(ch.unlock("m"), Ok(Some(3)));
        assert_eq!(ch.unlock("m"), Ok(None));
        assert!(ch.unlock("m").is_err());
        assert!(ch.unlock("never-used").is_err());
    }

    #[test]
    fn a_client_that_leaves_loses_its_place() {
        let mut ch = Channels::default();
        ch.wait("w", 1).unwrap();
        ch.wait("w", 2).unwrap();
        ch.lock("m", 3).unwrap();
        ch.lock("m", 4).unwrap();
        ch.drop_client(1);
        ch.drop_client(4);
        assert_eq!(ch.signal("w"), Ok(vec![2]));
        assert_eq!(ch.unlock("m"), Ok(None));
    }

    #[test]
    fn names_and_counts_are_limited() {
        let mut ch = Channels::default();
        for bad in ["", "a b", "x/y", &"n".repeat(65)] {
            assert!(ch.wait(bad, 1).is_err(), "{bad:?}");
        }
        for i in 0..MAX_WAITING {
            assert_eq!(ch.wait(&format!("c{}", i % 8), i), Ok(false));
        }
        assert!(ch.wait("one-more", 999).is_err());
        let mut many = Channels::default();
        for i in 0..MAX_CHANNELS {
            many.signal(&format!("s{i}")).unwrap();
        }
        assert!(many.signal("one-too-many").is_err());
    }

    #[test]
    fn an_idle_channel_is_forgotten() {
        let mut ch = Channels::default();
        ch.signal("a").unwrap();
        ch.wait("a", 1).unwrap();
        assert!(ch.map.is_empty());
        ch.lock("b", 1).unwrap();
        ch.unlock("b").unwrap();
        assert!(ch.map.is_empty());
    }
}
