//! Exercise the default executable before any service composition.
#![cfg(not(feature = "qualification-hooks"))]

use std::{ffi::OsStr, io::ErrorKind, time::Duration};
use tokio::{process::Command, time::timeout};

#[tokio::test]
async fn ordinary_cma_rejects_reserved_qualification_options() {
    let prefix = format!("cma-q-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let plan = std::env::temp_dir().join(format!("{prefix}.json"));
    let socket = std::env::temp_dir().join(format!("{prefix}.sock"));
    let plan_args = [OsStr::new("--qualification-plan"), plan.as_os_str()];
    let socket_args = [
        OsStr::new("--qualification-control-socket"),
        socket.as_os_str(),
    ];
    let combined = [plan_args, socket_args].concat();

    for arguments in [&plan_args[..], &socket_args[..], &combined] {
        let output = timeout(
            Duration::from_secs(10),
            Command::new(env!("CARGO_BIN_EXE_cma"))
                .args(arguments)
                .env_clear()
                .kill_on_drop(true)
                .output(),
        )
        .await
        .expect("qualification option did not exit within the bound")
        .expect("run the actual CMA executable");
        match std::fs::symlink_metadata(&socket) {
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => panic!("could not verify socket absence: {error}"),
            Ok(entry) => {
                if entry.is_dir() {
                    std::fs::remove_dir(&socket).expect("remove unexpected fixture directory");
                } else {
                    std::fs::remove_file(&socket).expect("remove unexpected fixture file");
                }
                panic!("ordinary CMA created the requested qualification socket path");
            }
        }
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let diagnostic = String::from_utf8(output.stderr).expect("valid startup diagnostic");
        assert!(diagnostic.starts_with("usage: cma "));
        assert_eq!(diagnostic.lines().count(), 1);
        assert!(!diagnostic.contains(&prefix));
    }
}
