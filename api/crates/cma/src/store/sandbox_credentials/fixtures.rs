use super::*;

impl MongoSandboxCredentialStore {
    pub(in crate::store) async fn seed_retained_for_test(&self, material: &CredentialMaterial) {
        let document = self.document(material).unwrap();
        self.collection.insert_one(document).await.unwrap();
    }
}
