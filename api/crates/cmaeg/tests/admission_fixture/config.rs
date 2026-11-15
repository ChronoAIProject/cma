use serde::{Deserialize, Serialize};
use std::{
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub listen: SocketAddr,
    pub advertised_origin: String,
    pub database: String,
    pub admin_mongodb_uri: String,
    pub oracle_mongodb_uri: String,
    pub gateway_mongodb_uri: String,
    pub private_output: PathBuf,
    pub control_bearer: String,
    pub replicas_enabled: bool,
}
impl Config {
    pub fn read(path: &Path) -> Result<Self, &'static str> {
        let metadata = std::fs::symlink_metadata(path).map_err(|_| "fixture config missing")?;
        if !metadata.is_file() || metadata.len() > 32 * 1024 {
            return Err("invalid fixture config file");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err("fixture config must be private");
            }
        }
        let bytes = std::fs::read(path).map_err(|_| "fixture config unreadable")?;
        let config: Self = serde_json::from_slice(&bytes).map_err(|_| "invalid fixture config")?;
        config.validate()?;
        Ok(config)
    }
    fn validate(&self) -> Result<(), &'static str> {
        let suffix = self
            .database
            .strip_prefix("eg_diag_")
            .ok_or("fixture database prefix")?;
        let id = uuid::Uuid::parse_str(suffix).map_err(|_| "fixture database identity")?;
        if id.get_version_num() != 4 || id.simple().to_string() != suffix {
            return Err("fixture database identity");
        }
        if !self.listen.ip().is_loopback() && !self.listen.ip().is_unspecified() {
            return Err("fixture listen address");
        }
        if self.listen.port() == 0 {
            return Err("fixture listen port must be prebound");
        }
        let origin = local_origin(&self.advertised_origin, false)?;
        if !origin
            .host_str()
            .is_some_and(|host| host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback()))
        {
            return Err("advertised HTTP fixture must be loopback in each gateway");
        }
        if self.control_bearer.len() < 32
            || self.control_bearer.len() > 128
            || !self
                .control_bearer
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err("fixture control credential shape");
        }
        for value in [
            &self.admin_mongodb_uri,
            &self.oracle_mongodb_uri,
            &self.gateway_mongodb_uri,
        ] {
            if !value.starts_with("mongodb://") || value.len() > 8192 {
                return Err("fixture Mongo URI shape");
            }
        }
        if !self.private_output.is_absolute() || self.private_output.exists() {
            return Err("fixture output must be a new absolute directory");
        }
        Ok(())
    }
}

pub async fn local_mongo(uri: &str) -> Result<mongodb::Client, &'static str> {
    use mongodb::options::{ClientOptions, ServerAddress};
    if !uri.starts_with("mongodb://") {
        return Err("fixture Mongo scheme");
    }
    let mut options = ClientOptions::parse(uri)
        .await
        .map_err(|_| "fixture Mongo options")?;
    for address in &options.hosts {
        match address {
            ServerAddress::Tcp {
                host,
                port: Some(port),
            } if *port != 0 => {
                let allowed = host == "localhost"
                    || host.ends_with(".cma-scale.svc.cluster.local")
                    || host.parse::<IpAddr>().is_ok_and(|ip| match ip {
                        IpAddr::V4(ip) => ip.is_private() || ip.is_loopback(),
                        IpAddr::V6(ip) => ip.is_unique_local() || ip.is_loopback(),
                    });
                if !allowed {
                    return Err("fixture Mongo host outside local boundary");
                }
            }
            _ => return Err("fixture Mongo address must be explicit local TCP"),
        }
    }
    options.server_selection_timeout = Some(std::time::Duration::from_secs(3));
    options.connect_timeout = Some(std::time::Duration::from_secs(3));
    mongodb::Client::with_options(options).map_err(|_| "fixture Mongo client")
}

pub fn private_dir(path: &Path) -> Result<(), &'static str> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .map_err(|_| "fixture private output directory")
}

pub fn local_origin(value: &str, allow_fixture_dns: bool) -> Result<reqwest::Url, &'static str> {
    let url = reqwest::Url::parse(value).map_err(|_| "invalid local origin")?;
    let host = url.host_str().ok_or("local origin host")?;
    let private = host.parse::<IpAddr>().is_ok_and(|ip| match ip {
        IpAddr::V4(ip) => ip.is_loopback() || ip.is_private(),
        IpAddr::V6(ip) => ip.is_loopback() || ip.is_unique_local(),
    });
    if url.scheme() != "http"
        || url.port().is_none()
        || url.port() == Some(0)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || !(private || allow_fixture_dns && host == "host.docker.internal")
    {
        return Err("origin outside local fixture boundary");
    }
    Ok(url)
}

pub fn private_json(path: &Path, value: &impl Serialize) -> Result<(), &'static str> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|_| "private output creation failed")?;
    let bytes = serde_json::to_vec_pretty(value).map_err(|_| "private output encoding failed")?;
    file.write_all(&bytes)
        .map_err(|_| "private output write failed")?;
    file.sync_all().map_err(|_| "private output sync failed")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn endpoints_are_explicit_local_origins_without_credentials_or_paths() {
        for valid in ["http://127.0.0.1:19100", "http://10.2.3.4:8080"] {
            assert!(local_origin(valid, false).is_ok());
        }
        assert!(local_origin("http://host.docker.internal:19100", true).is_ok());
        for invalid in [
            "http://host.docker.internal:19100",
            "https://127.0.0.1:19100",
            "http://public.example:8080",
            "http://8.8.8.8:8080",
            "http://127.0.0.1:8080/path",
            "http://user@127.0.0.1:8080",
            "http://127.0.0.1:8080/?token=x",
        ] {
            assert!(local_origin(invalid, false).is_err());
        }
    }
}

#[cfg(test)]
mod private_config_tests {
    use super::*;
    fn fixture() -> Config {
        Config {
            listen: "127.0.0.1:19100".parse().unwrap(),
            advertised_origin: "http://127.0.0.1:19100".into(),
            database: format!("eg_diag_{}", uuid::Uuid::new_v4().simple()),
            admin_mongodb_uri: "mongodb://127.0.0.1:27017".into(),
            oracle_mongodb_uri: "mongodb://127.0.0.1:27017".into(),
            gateway_mongodb_uri: "mongodb://127.0.0.1:27017".into(),
            private_output: std::env::temp_dir()
                .join(format!("eg-config-{}", uuid::Uuid::new_v4())),
            control_bearer: uuid::Uuid::new_v4().to_string(),
            replicas_enabled: true,
        }
    }
    #[test]
    fn scope_is_fresh_dedicated_database_and_loopback_advertised_origin() {
        let mut config = fixture();
        assert!(config.validate().is_ok());
        config.database = "cma_scale".into();
        assert!(config.validate().is_err());
        config = fixture();
        config.advertised_origin = "http://10.2.3.4:19100".into();
        assert!(config.validate().is_err());
        config = fixture();
        config.control_bearer = "small".into();
        assert!(config.validate().is_err());
        config = fixture();
        config.listen = "8.8.8.8:19100".parse().unwrap();
        assert!(config.validate().is_err());
        config = fixture();
        config.listen = "127.0.0.1:0".parse().unwrap();
        assert!(config.validate().is_err());
        config = fixture();
        config.admin_mongodb_uri = "mongodb+srv://external.invalid".into();
        assert!(config.validate().is_err());
    }
    #[test]
    fn private_receipts_are_create_once_and_retain_exact_bytes() {
        let path = std::env::temp_dir().join(format!("eg-private-{}", uuid::Uuid::new_v4()));
        private_dir(&path).unwrap();
        assert!(private_dir(&path).is_err());
        let value = serde_json::json!({"status":"retained"});
        let receipt = path.join("receipt.json");
        private_json(&receipt, &value).unwrap();
        assert!(private_json(&receipt, &value).is_err());
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(&receipt).unwrap()).unwrap(),
            value
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(&receipt).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::remove_file(receipt).unwrap();
        std::fs::remove_dir(path).unwrap();
    }
    #[tokio::test]
    async fn public_mongo_and_non_tcp_routes_are_refused_before_connecting() {
        for uri in [
            "mongodb://8.8.8.8:27017",
            "mongodb://production.invalid:27017",
            "mongodb+srv://production.invalid",
        ] {
            assert!(local_mongo(uri).await.is_err());
        }
    }
}
