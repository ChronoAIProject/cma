//! Explicit operator commands; ordinary startup never runs DDL or audits.
pub(crate) async fn command(arguments: &[String]) -> Result<(), String> {
    let valid = match arguments {
        [mode] => mode == "--migrate-agent-log-segments",
        [mode, _, _] => matches!(
            mode.as_str(),
            "--audit-agent-log-segments"
                | "--cancel-agent-log-audit"
                | "--install-agent-log-projection"
        ),
        _ => false,
    };
    if !valid {
        return Err("usage: cma --migrate-agent-log-segments | --audit-agent-log-segments AGENT AUDIT_ID | --cancel-agent-log-audit AGENT AUDIT_ID | --install-agent-log-projection AGENT AUDIT_ID".into());
    }
    let uri = std::env::var("CHRONO_MONGODB_URI").map_err(|_| "CHRONO_MONGODB_URI is required")?;
    let name = std::env::var("CHRONO_MONGODB_DATABASE")
        .map_err(|_| "CHRONO_MONGODB_DATABASE is required")?;
    let client = mongodb::Client::with_uri_str(uri)
        .await
        .map_err(|_| "cannot connect to migration database")?;
    let database = client.database(&name);
    if arguments.len() == 1 {
        super::migrate(&database).await.map_err(|e| e.to_string())?;
        println!("Agent log segment additive schema verified; no Agent activated");
    } else {
        super::verify(&database).await.map_err(|e| e.to_string())?;
        let status = if arguments[0] == "--install-agent-log-projection" {
            super::audit::install(&client, &database, &arguments[1], &arguments[2]).await
        } else {
            super::audit_unit(
                &client,
                &database,
                &arguments[1],
                &arguments[2],
                arguments[0] == "--cancel-agent-log-audit",
            )
            .await
        }
        .map_err(|e| e.to_string())?;
        println!("Agent log audit unit: {status}");
    }
    Ok(())
}
