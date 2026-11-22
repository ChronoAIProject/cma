use layer2_core::sandbox::{Sandbox, SandboxOrigin};
use mongodb::{ClientSession, Database};

use super::AdmissionError;
use crate::store::profile_pin_admission;

pub(super) async fn admit(
    database: &Database,
    sandbox: &Sandbox,
    session: &mut ClientSession,
) -> Result<(), AdmissionError> {
    let profile = &sandbox.profile;
    if matches!(&sandbox.origin, SandboxOrigin::Trigger { .. })
        || !profile_pin_admission::sandbox(
            database,
            profile.id.as_str(),
            profile.revision,
            Some(&profile.digest),
            session,
        )
        .await
        .map_err(AdmissionError::Driver)?
    {
        profile_pin_admission::frozen::sandbox(database, sandbox, session)
            .await
            .map_err(AdmissionError::Driver)?
            .require()
            .map_err(AdmissionError::Domain)?;
    }
    Ok(())
}
