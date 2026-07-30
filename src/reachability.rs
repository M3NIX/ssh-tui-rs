use std::{
    collections::VecDeque,
    net::{TcpStream, ToSocketAddrs},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

const MAX_WORKERS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostReachability {
    Unchecked,
    Checking,
    Reachable,
    Unreachable,
}

#[derive(Debug)]
pub(crate) struct CheckTarget {
    pub host_index: usize,
    pub host: String,
    pub port: u16,
}

#[derive(Debug)]
pub(crate) struct CheckResult {
    pub host_index: usize,
    pub reachability: HostReachability,
}

pub(crate) fn spawn_checks(
    targets: Vec<CheckTarget>,
    timeout: Duration,
) -> mpsc::Receiver<CheckResult> {
    let (results_tx, results_rx) = mpsc::channel();
    let worker_count = targets.len().min(MAX_WORKERS);
    let queue = Arc::new(Mutex::new(VecDeque::from(targets)));

    for _ in 0..worker_count {
        let queue = Arc::clone(&queue);
        let results_tx = results_tx.clone();
        std::thread::spawn(move || {
            loop {
                let target = queue.lock().ok().and_then(|mut queue| queue.pop_front());
                let Some(target) = target else {
                    break;
                };
                let reachability = if tcp_connects(&target.host, target.port, timeout) {
                    HostReachability::Reachable
                } else {
                    HostReachability::Unreachable
                };
                if results_tx
                    .send(CheckResult {
                        host_index: target.host_index,
                        reachability,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
    }

    results_rx
}

fn tcp_connects(host: &str, port: u16, timeout: Duration) -> bool {
    let started = Instant::now();
    let Ok(addresses) = (host, port).to_socket_addrs() else {
        return false;
    };
    connects_to_any(addresses, started, timeout, |address, remaining| {
        TcpStream::connect_timeout(address, remaining).is_ok()
    })
}

fn connects_to_any(
    addresses: impl IntoIterator<Item = std::net::SocketAddr>,
    started: Instant,
    timeout: Duration,
    mut connect: impl FnMut(&std::net::SocketAddr, Duration) -> bool,
) -> bool {
    for address in addresses {
        let Some(remaining) = timeout.checked_sub(started.elapsed()) else {
            return false;
        };
        if connect(&address, remaining) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tries_resolved_addresses_until_one_connects() {
        let addresses = [
            "127.0.0.1:21".parse().unwrap(),
            "127.0.0.1:22".parse().unwrap(),
        ];
        let mut attempted_ports = Vec::new();

        let reachable = connects_to_any(
            addresses,
            Instant::now(),
            Duration::from_secs(5),
            |address, remaining| {
                attempted_ports.push(address.port());
                assert!(remaining <= Duration::from_secs(5));
                address.port() == 22
            },
        );

        assert!(reachable);
        assert_eq!(attempted_ports, [21, 22]);
    }
}
