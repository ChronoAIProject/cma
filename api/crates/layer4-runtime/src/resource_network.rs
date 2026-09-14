use std::path::Path;

pub(crate) fn read(net: &Path, proc_net_dev: &Path) -> (Option<u64>, Option<u64>) {
    let sysfs = read_sysfs(net);
    if sysfs.0.is_some() {
        sysfs
    } else {
        read_proc(proc_net_dev)
    }
}

fn read_sysfs(net: &Path) -> (Option<u64>, Option<u64>) {
    let Ok(entries) = std::fs::read_dir(net) else {
        return (None, None);
    };
    let mut received = 0_u128;
    let mut sent = 0_u128;
    let mut readable = false;
    for entry in entries.filter_map(std::result::Result::ok) {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if name == "lo" || !trusted(&name) {
            continue;
        }
        let statistics = net.join(name).join("statistics");
        let (Some(rx), Some(tx)) = (
            read_u64(&statistics.join("rx_bytes")),
            read_u64(&statistics.join("tx_bytes")),
        ) else {
            continue;
        };
        received = received.saturating_add(u128::from(rx));
        sent = sent.saturating_add(u128::from(tx));
        readable = true;
    }
    totals(readable, received, sent)
}

fn read_proc(path: &Path) -> (Option<u64>, Option<u64>) {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return (None, None);
    };
    let mut received = 0_u128;
    let mut sent = 0_u128;
    let mut readable = false;
    for line in raw.lines() {
        let Some((name, values)) = line.rsplit_once(':') else {
            continue;
        };
        let name = name.trim();
        if name == "lo" || !trusted(name) {
            continue;
        }
        let values = values
            .split_whitespace()
            .map(str::parse::<u64>)
            .collect::<std::result::Result<Vec<_>, _>>();
        let Ok(values) = values else {
            continue;
        };
        if values.len() != 16 {
            continue;
        }
        received = received.saturating_add(u128::from(values[0]));
        sent = sent.saturating_add(u128::from(values[8]));
        readable = true;
    }
    totals(readable, received, sent)
}

fn totals(readable: bool, received: u128, sent: u128) -> (Option<u64>, Option<u64>) {
    if !readable {
        return (None, None);
    }
    (
        Some(u64::try_from(received).unwrap_or(u64::MAX)),
        Some(u64::try_from(sent).unwrap_or(u64::MAX)),
    )
}

fn read_u64(path: &Path) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn trusted(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 15
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'@' | b':')
        })
}
