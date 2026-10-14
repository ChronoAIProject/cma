use std::collections::BTreeMap;

use layer2_core::trigger_domain::{ServiceConnectionMetadata, ServiceConnectionMode};

use super::{
    decode, malformed, selected_service, Deserialize, NyxidServiceConnectionBroker,
    ServiceConnectionError, ServiceId, SubjectId,
};

const CATALOG_PATH: &str = "/api/v1/services";
const MAX_CATALOG_ROWS: usize = 1_000;

#[derive(Deserialize)]
struct Catalog {
    services: Vec<CatalogService>,
}

#[derive(Deserialize)]
struct CatalogService {
    slug: String,
    name: String,
    #[serde(default)]
    service_type: Option<String>,
    #[serde(default)]
    is_active: Option<bool>,
}

impl NyxidServiceConnectionBroker {
    pub(super) async fn catalog_metadata(
        &self,
        owner: &SubjectId,
        services: &[ServiceId],
    ) -> Result<Vec<ServiceConnectionMetadata>, ServiceConnectionError> {
        if services.is_empty() {
            return Ok(Vec::new());
        }
        let credential = self.credential(owner).await?;
        let response = self
            .send(
                self.http
                    .get(self.endpoint(CATALOG_PATH))
                    .bearer_auth(credential.expose()),
                false,
            )
            .await?;
        let catalog: Catalog = decode(response).await?;
        let connections = self.user_services(&credential).await?;
        if catalog.services.len() > MAX_CATALOG_ROWS {
            return Err(malformed());
        }
        let mut by_slug = BTreeMap::new();
        for row in &catalog.services {
            if row.slug.trim().is_empty()
                || row.slug.len() > 128
                || row.slug.trim() != row.slug
                || row.slug.chars().any(char::is_control)
                || row.name.trim().is_empty()
                || row.name.len() > 256
                || row.name.chars().any(char::is_control)
                || by_slug.insert(row.slug.as_str(), row).is_some()
            {
                return Err(malformed());
            }
        }
        let mut setup_url = self.browser_origin.clone();
        setup_url.set_path("/keys");
        Ok(services
            .iter()
            .map(|service| {
                let existing = selected_service(service, &connections);
                let slug = existing.map_or(service.as_str(), |connection| connection.slug.as_str());
                let row = by_slug.get(slug);
                if let Some(connection) = existing {
                    // Repair the exact binding used by this launch. Creating another
                    // hosted connection could leave the broken binding in place.
                    let mut repair = setup_url.clone();
                    repair
                        .path_segments_mut()
                        .expect("HTTPS browser origin")
                        .push(&connection.id);
                    return ServiceConnectionMetadata {
                        service: service.clone(),
                        display_name: row
                            .map(|row| row.name.clone())
                            .or_else(|| Some(slug.to_owned())),
                        connection_mode: ServiceConnectionMode::Manual,
                        setup_url: Some(repair.to_string()),
                    };
                }
                let connection_mode = match row
                    .filter(|row| row.is_active == Some(true))
                    .and_then(|row| row.service_type.as_deref())
                {
                    Some("http") => ServiceConnectionMode::Hosted,
                    Some("ssh") => ServiceConnectionMode::Manual,
                    _ => ServiceConnectionMode::Unknown,
                };
                ServiceConnectionMetadata {
                    service: service.clone(),
                    display_name: row.map(|row| row.name.clone()),
                    connection_mode,
                    setup_url: (connection_mode == ServiceConnectionMode::Manual)
                        .then(|| setup_url.to_string()),
                }
            })
            .collect())
    }
}
