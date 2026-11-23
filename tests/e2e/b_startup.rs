//! Four clients start at the same moment (the first one to get there starts the server), over and
//! over. Every round waits on real markers, not sleeps: `imux ls` must list all four sessions. A
//! client that never starts shows up as a failed round with the time it took, so a startup stall is
//! told apart from a slow test. (The Rust version of tests/b_startup.sh.)
use crate::support::*;
use std::time::{Duration, Instant};

#[test]
fn b_startup() {
    let rounds: u32 = std::env::var("ROUNDS").ok().and_then(|v| v.parse().ok()).unwrap_or(10);
    let limit = Duration::from_secs(std::env::var("LIMIT").ok().and_then(|v| v.parse().ok()).unwrap_or(20));
    let e = Env::new("bst");
    let mut t = Check::new("b_startup");
    let mut slowest = Duration::ZERO;
    for round in 1..=rounds {
        let t0 = Instant::now();
        // four clients, started from four threads at once
        let clients: Vec<Client> = std::thread::scope(|s| {
            let handles: Vec<_> = (1..=4)
                .map(|n| {
                    let e = &e;
                    s.spawn(move || e.client(100, 30, &["new", "-s", &format!("s{n}")]))
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let mut seen = 0;
        while t0.elapsed() <= limit {
            seen = e.out(&["ls"]).lines().filter(|l| l.len() > 2 && l.starts_with('s') && l[1..].starts_with(|c: char| ('1'..='4').contains(&c)) && l[2..].starts_with(':')).count();
            if seen == 4 {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let elapsed = t0.elapsed();
        slowest = slowest.max(elapsed);
        if seen == 4 {
            t.ok(&format!("round {round}: four clients up in {:.1}s", elapsed.as_secs_f64()), true);
        } else {
            t.ok(&format!("round {round}: only {seen} of 4 sessions after {:.1}s", elapsed.as_secs_f64()), false);
            for (n, c) in clients.iter().enumerate() {
                println!("       client {}: {} bytes written", n + 1, c.bytes().len());
            }
            break;
        }
        e.kill_server();
        drop(clients);
        std::thread::sleep(Duration::from_millis(200));
    }
    println!("slowest round: {:.1}s", slowest.as_secs_f64());
    t.finish();
}
