//! The gateway's Tokio runtime.

/// The gateway runtime. Built explicitly so later process modes can run before any runtime exists.
pub fn build() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
}

#[cfg(test)]
mod tests {
    use tokio::runtime::{Handle, RuntimeFlavor};

    #[test]
    fn builds_the_multi_thread_runtime_with_io_and_time() {
        let runtime = super::build().unwrap();
        let flavor = runtime.block_on(async {
            // Both drivers are enabled: a timer and a socket need them.
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            Handle::current().runtime_flavor()
        });
        assert_eq!(flavor, RuntimeFlavor::MultiThread);
    }
}
