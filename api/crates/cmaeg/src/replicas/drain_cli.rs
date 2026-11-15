use super::{config::Config, drain};
use std::{net::IpAddr, time::Duration};

/// Uses the serving process's existing key authority; disabled mode performs no IO.
pub async fn run() -> Result<(), &'static str> {
    if !Config::from_env()?.enabled {
        return Ok(());
    }
    let config = crate::Config::from_env().map_err(|_| "replica drain configuration invalid")?;
    let ip: IpAddr = if config.listen.is_ipv6() {
        "::1"
    } else {
        "127.0.0.1"
    }
    .parse()
    .map_err(|_| "replica drain address invalid")?;
    let address = std::net::SocketAddr::new(ip, config.listen.port());
    let ticket = drain::ticket(
        &config.management.master_keys,
        config.listen.port(),
        &config.management.public_url,
        time::OffsetDateTime::now_utc().unix_timestamp(),
    )?;
    let response = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(25))
        .build()
        .map_err(|_| "replica drain client unavailable")?
        .post(format!("http://{address}{}", drain::PATH))
        .bearer_auth(ticket)
        .send()
        .await
        .map_err(|_| "replica drain request failed")?;
    if response.status() != reqwest::StatusCode::NO_CONTENT {
        return Err("replica drain request refused");
    }
    Ok(())
}
