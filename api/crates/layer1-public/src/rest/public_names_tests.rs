use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

use layer2_core::{
    DirectoryQuery, Page, PublicIdentity, RegisteredUser, StoreError, SubjectId, UserDirectory,
    UserDirectoryStore,
};

#[derive(Default)]
pub(crate) struct Names {
    pub calls: AtomicUsize,
    identities: Mutex<BTreeMap<String, PublicIdentity>>,
}

impl Names {
    pub fn seeded() -> Arc<Self> {
        let identities = [
            ("owner-a", "Alex Chen"),
            ("owner-b", "Sam Lee"),
            ("operator-a", "Morgan Park"),
            ("owner", "Research Creator"),
        ]
        .into_iter()
        .map(|(subject, name)| {
            (
                subject.into(),
                PublicIdentity {
                    subject: SubjectId(subject.into()),
                    display_name: Some(name.into()),
                    avatar_url: None,
                },
            )
        })
        .collect();
        Arc::new(Self {
            identities: Mutex::new(identities),
            ..Self::default()
        })
    }
}

#[async_trait::async_trait]
impl UserDirectoryStore for Names {
    async fn observe(&self, identity: &PublicIdentity) -> Result<(), StoreError> {
        self.identities
            .lock()
            .unwrap()
            .insert(identity.subject.0.clone(), identity.clone());
        Ok(())
    }

    async fn user(&self, subject: &SubjectId) -> Result<Option<RegisteredUser>, StoreError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .identities
            .lock()
            .unwrap()
            .get(&subject.0)
            .cloned()
            .map(|identity| RegisteredUser {
                identity,
                first_seen_at: "2026-09-09T00:00:00Z".into(),
                last_seen_at: "2026-09-09T00:00:00Z".into(),
            }))
    }

    async fn list(&self, _: &DirectoryQuery) -> Result<Page<RegisteredUser>, StoreError> {
        panic!("resource names must not enumerate the user directory")
    }
}

#[tokio::test]
async fn names_are_bounded_and_missing_or_identifier_names_are_not_invented() {
    let store = Names::seeded();
    let directory = UserDirectory::new(store.clone());
    assert!(directory
        .display_names(&vec![SubjectId("owner-a".into()); 201])
        .await
        .is_err());
    assert_eq!(store.calls.load(Ordering::SeqCst), 0);
    for name in [None, Some(" "), Some("owner-a")] {
        directory
            .observe(&PublicIdentity {
                subject: SubjectId("owner-a".into()),
                display_name: name.map(str::to_owned),
                avatar_url: None,
            })
            .await
            .unwrap();
        let names = directory
            .display_names(&[
                SubjectId("owner-a".into()),
                SubjectId("owner-b".into()),
                SubjectId("unknown".into()),
            ])
            .await
            .unwrap();
        assert_eq!(
            names,
            BTreeMap::from([("owner-b".into(), "Sam Lee".into())])
        );
    }
}
