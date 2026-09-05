#[allow(
    dead_code,
    reason = "composed when the Trigger Launch routes and worker are introduced"
)]
mod connections_nyxid;

#[cfg(test)]
mod connections_nyxid_tests;

#[allow(
    dead_code,
    reason = "composed with the public Launch routes introduced after the local worker contract"
)]
mod scheduler;
#[allow(
    dead_code,
    reason = "composed with the public Launch routes introduced after the local worker contract"
)]
mod worker;

#[cfg(test)]
mod worker_tests;
