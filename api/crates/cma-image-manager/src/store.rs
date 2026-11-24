use crate::{model::*, schema};
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, Document},
    options::{Acknowledgment, ReadConcern, ReturnDocument, WriteConcern},
    Collection, Database,
};

#[derive(Clone)]
pub struct Store {
    db: Database,
    images: Collection<Image>,
    builds: Collection<Build>,
}

impl Store {
    pub async fn open(db: Database) -> Result<Self> {
        Self::open_with_authority(
            db,
            cma_service_auth::mongodb_authority::MongoAuthorityMode::Isolated,
        )
        .await
    }
    pub async fn open_with_authority(
        db: Database,
        authority: cma_service_auth::mongodb_authority::MongoAuthorityMode,
    ) -> Result<Self> {
        schema::verify_runtime_with_authority(&db, authority).await?;
        let options = mongodb::options::CollectionOptions::builder()
            .read_concern(ReadConcern::majority())
            .write_concern(WriteConcern::builder().w(Acknowledgment::Majority).build())
            .build();
        Ok(Self {
            images: db.collection_with_options(schema::IMAGES, options.clone()),
            builds: db.collection_with_options(schema::BUILDS, options),
            db,
        })
    }
    pub async fn ping(&self) -> Result<()> {
        self.db.run_command(doc! {"ping":1}).await?;
        Ok(())
    }
    pub async fn create_image(&self, owner: &str, key: &str, spec: CreateImage) -> Result<Image> {
        spec.validate()?;
        validate_actor_key(owner, key)?;
        let image = Image {
            id: id("img"),
            schema_version: DOCUMENT_VERSION,
            owner: owner.into(),
            request_key_hash: hash(&key)?,
            request_sha256: hash(&spec)?,
            spec,
            shares: vec![],
            visibility: ImageVisibility::Private,
            retired: false,
            revision: 1,
            created_at_ms: now_ms(),
        };
        let existing = self
            .images
            .find_one(doc! {"owner":owner,"request_key_hash":&image.request_key_hash})
            .await?;
        if let Some(existing) = existing {
            return same_image(existing, &image);
        }
        match self.images.insert_one(&image).await {
            Ok(_) => Ok(image),
            Err(error) => match self
                .images
                .find_one(doc! {"owner":owner,"request_key_hash":&image.request_key_hash})
                .await?
            {
                Some(existing) => same_image(existing, &image),
                None => Err(error.into()),
            },
        }
    }
    pub async fn image(&self, actor: &str, id: &str, owned: bool) -> Result<Image> {
        self.images
            .find_one(access_filter(actor, id, owned))
            .await?
            .ok_or(Error::NotFound)
    }
    pub async fn list_images(
        &self,
        actor: &str,
        after: &str,
        scope: ImageListScope,
    ) -> Result<Vec<Image>> {
        let mut filter = match scope {
            ImageListScope::Private => doc! {"owner": actor, "visibility":{"$ne":"public"}},
            ImageListScope::Owned => doc! {"owner": actor},
            ImageListScope::Shared => doc! {"shares": actor, "owner":{"$ne":actor}},
            ImageListScope::Public => doc! {"visibility":"public", "retired":false},
            ImageListScope::All => access_filter(actor, "", false),
        };
        filter.remove("_id");
        filter.insert("_id", doc! {"$gt":after});
        Ok(self
            .images
            .find(filter)
            .sort(doc! {"_id":1})
            .limit(101)
            .await?
            .try_collect()
            .await?)
    }
    pub async fn update_image(
        &self,
        owner: &str,
        id: &str,
        revision: i64,
        spec: CreateImage,
    ) -> Result<Image> {
        spec.validate()?;
        self.image_change(owner,id,revision,doc!{"$set":{"spec":mongodb::bson::serialize_to_document(&spec).map_err(|_|Error::Invalid("invalid image"))?}}).await
    }
    pub async fn retire(&self, owner: &str, id: &str, revision: i64) -> Result<Image> {
        // Retirement never deletes registry blobs still used by live Agents.
        self.image_change(owner, id, revision, doc! {"$set":{"retired":true}})
            .await
    }
    pub async fn set_visibility(
        &self,
        owner: &str,
        id: &str,
        revision: i64,
        visibility: ImageVisibility,
    ) -> Result<Image> {
        if revision <= 0 {
            return Err(Error::Invalid("positive revision required"));
        }
        self.image(owner, id, true).await?;
        let mut filter = doc! {"_id":id,"owner":owner,"revision":revision};
        if visibility == ImageVisibility::Public {
            filter.insert("retired", false);
        }
        let value =
            mongodb::bson::serialize_to_bson(&visibility).map_err(|_| Error::Unavailable)?;
        self.images
            .find_one_and_update(
                filter,
                doc! {"$set":{"visibility":value},"$inc":{"revision":1_i64}},
            )
            .return_document(ReturnDocument::After)
            .await?
            .ok_or(Error::Conflict)
    }
    pub async fn share(
        &self,
        owner: &str,
        id: &str,
        revision: i64,
        subject: &str,
        grant: bool,
    ) -> Result<Image> {
        if !bounded_text(subject, 512) {
            return Err(Error::Invalid("invalid share subject"));
        }
        let image = self.image(owner, id, true).await?;
        if grant && image.shares.len() >= 1000 && !image.shares.iter().any(|s| s == subject) {
            return Err(Error::Invalid("image share limit reached"));
        }
        let update = if grant {
            doc! {"$addToSet":{"shares":subject}}
        } else {
            doc! {"$pull":{"shares":subject}}
        };
        self.image_change(owner, id, revision, update).await
    }
    async fn image_change(
        &self,
        owner: &str,
        id: &str,
        revision: i64,
        mut update: Document,
    ) -> Result<Image> {
        if revision <= 0 {
            return Err(Error::Invalid("positive revision required"));
        }
        update.insert("$inc", doc! {"revision":1_i64});
        self.images
            .find_one_and_update(doc! {"_id":id,"owner":owner,"revision":revision}, update)
            .return_document(ReturnDocument::After)
            .await?
            .ok_or(Error::Conflict)
    }
    pub async fn create_build(
        &self,
        owner: &str,
        image_id: &str,
        key: &str,
        source: BuildInput,
    ) -> Result<Build> {
        validate_actor_key(owner, key)?;
        source.validate()?;
        let build = Build {
            id: id("bld"),
            schema_version: DOCUMENT_VERSION,
            image_id: image_id.into(),
            owner: owner.into(),
            request_key_hash: hash(&key)?,
            request_sha256: hash(&(image_id, &source))?,
            source,
            phase: BuildPhase::Queued,
            revision: 1,
            cloud_id: None,
            digest: None,
            error: None,
            created_at_ms: now_ms(),
            updated_at_ms: now_ms(),
        };
        let query = doc! {"owner":owner,"request_key_hash":&build.request_key_hash};
        if let Some(existing) = self.builds.find_one(query.clone()).await? {
            return same_build(existing, &build);
        }
        // Fence retirement and insert the build in one transaction. The admitted
        // request survives a lost commit response and is recovered by its key.
        let mut session = self.db.client().start_session().await?;
        session
            .start_transaction()
            .read_concern(ReadConcern::snapshot())
            .write_concern(WriteConcern::builder().w(Acknowledgment::Majority).build())
            .await?;
        let admitted = self
            .images
            .update_one(
                doc! {"_id":image_id,"owner":owner,"retired":false},
                doc! {"$inc":{"revision":1_i64}},
            )
            .session(&mut session)
            .await?;
        if admitted.matched_count != 1 {
            session.abort_transaction().await?;
            return Err(Error::Conflict);
        }
        self.builds.insert_one(&build).session(&mut session).await?;
        match session.commit_transaction().await {
            Ok(()) => Ok(build),
            Err(error) => match self.builds.find_one(query).await? {
                Some(existing) => same_build(existing, &build),
                None => Err(error.into()),
            },
        }
    }

    pub async fn build(&self, actor: &str, id: &str) -> Result<Build> {
        self.builds
            .find_one(doc! {"_id":id,"owner":actor})
            .await?
            .ok_or(Error::NotFound)
    }
    pub async fn image_builds(
        &self,
        actor: &str,
        image_id: &str,
        versions: bool,
        after: &str,
    ) -> Result<Vec<Build>> {
        self.image(actor, image_id, !versions).await?;
        let mut filter = doc! {"image_id":image_id,"_id":{"$gt":after}};
        if versions {
            filter.insert("phase", "succeeded");
        }
        Ok(self
            .builds
            .find(filter)
            .sort(doc! {"_id":1})
            .limit(101)
            .await?
            .try_collect()
            .await?)
    }
    pub async fn version(&self, actor: &str, image_id: &str, version: &str) -> Result<Build> {
        self.image(actor, image_id, false).await?;
        self.builds
            .find_one(doc! {"_id":version,"image_id":image_id,"phase":"succeeded"})
            .await?
            .ok_or(Error::NotFound)
    }
    pub async fn resolve(&self, actor: &str, image_id: &str, version: &str) -> Result<Build> {
        let image = self.image(actor, image_id, false).await?;
        if image.retired {
            return Err(Error::Conflict);
        }
        self.builds
            .find_one(doc! {"_id":version,"image_id":image_id,"phase":"succeeded"})
            .await?
            .ok_or(Error::NotFound)
    }
    pub async fn claim_build(&self) -> Result<Option<Build>> {
        self.claim_build_excluding(&[]).await
    }
    pub(crate) async fn claim_build_excluding(&self, excluded: &[String]) -> Result<Option<Build>> {
        Ok(self.builds.find_one_and_update(doc!{"phase":"queued","_id":{"$nin":excluded}},
            doc!{"$set":{"phase":"dispatching","error":"submission_outcome_unconfirmed","updated_at_ms":now_ms()},"$inc":{"revision":1_i64}})
            .sort(doc!{"created_at_ms":1}).return_document(ReturnDocument::After).await?)
    }
    pub async fn pending(&self) -> Result<Vec<Build>> {
        self.pending_excluding(&[]).await
    }
    pub(crate) async fn pending_excluding(&self, excluded: &[String]) -> Result<Vec<Build>> {
        Ok(self
            .builds
            .find(doc! {"phase":{"$in":["dispatching","running"]},"_id":{"$nin":excluded}})
            .sort(doc! {"updated_at_ms":1})
            .limit(100)
            .await?
            .try_collect()
            .await?)
    }
    pub async fn reject_dispatch(&self, current: &Build) -> Result<bool> {
        if current.phase != BuildPhase::Dispatching || current.cloud_id.is_some() {
            return Err(Error::Conflict);
        }
        let source = &current.source;
        let dockerfile_content = source.dockerfile_content.as_ref().map_or_else(
            || mongodb::bson::Bson::Document(doc! {"$exists": false}),
            |content| mongodb::bson::Bson::String(content.clone()),
        );
        let result = self.builds.update_one(
            doc! {"_id":&current.id,"revision":current.revision,"owner":&current.owner,
                "image_id":&current.image_id,"request_key_hash":&current.request_key_hash,
                "request_sha256":&current.request_sha256,"schema_version":current.schema_version,
                "source.repository":&source.repository,"source.commit":&source.commit,
                "source.dockerfile":&source.dockerfile,"source.context":&source.context,
                "source.dockerfile_content":dockerfile_content,"created_at_ms":current.created_at_ms,
                "phase":"dispatching","cloud_id":mongodb::bson::Bson::Null},
            doc! {"$set":{"phase":"failed","error":"submission_rejected","updated_at_ms":now_ms()},"$inc":{"revision":1_i64}},
        ).await?;
        Ok(result.modified_count == 1)
    }
    pub async fn defer_observation(&self, current: &Build) -> Result<()> {
        // Rotate pending observations so unknown dispatches cannot starve newer builds.
        self.builds.update_one(doc! {"_id":&current.id,"revision":current.revision,"phase":{"$in":["dispatching","running"]}},
            doc! {"$set":{"updated_at_ms":now_ms()},"$inc":{"revision":1_i64}}).await?;
        Ok(())
    }
    pub async fn observe_build(
        &self,
        current: &Build,
        cloud_id: &str,
        phase: BuildPhase,
        digest: Option<&str>,
    ) -> Result<bool> {
        if !bounded_text(cloud_id, 256)
            || current.cloud_id.as_ref().is_some_and(|s| s != cloud_id)
            || !matches!(current.phase, BuildPhase::Dispatching | BuildPhase::Running)
            || !matches!(
                phase,
                BuildPhase::Running
                    | BuildPhase::Succeeded
                    | BuildPhase::Failed
                    | BuildPhase::Cancelled
            )
            || (phase == BuildPhase::Succeeded && digest.is_none_or(|d| !valid_digest(d)))
            || (phase != BuildPhase::Succeeded && digest.is_some())
        {
            return Err(Error::Conflict);
        }
        let phase = mongodb::bson::serialize_to_bson(&phase).map_err(|_| Error::Unavailable)?;
        let result = self.builds.update_one(doc!{"_id":&current.id,"revision":current.revision,"owner":&current.owner,"image_id":&current.image_id,"request_sha256":&current.request_sha256,"cloud_id":&current.cloud_id,"phase":{"$in":["dispatching","running"]}},
            doc!{"$set":{"cloud_id":cloud_id,"phase":phase,"digest":digest,"error":mongodb::bson::Bson::Null,"updated_at_ms":now_ms()},"$inc":{"revision":1_i64}}).await?;
        Ok(result.modified_count == 1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageListScope {
    Private,
    Owned,
    Shared,
    Public,
    All,
}

fn access_filter(actor: &str, id: &str, owned: bool) -> Document {
    if owned {
        doc! {"_id":id,"owner":actor}
    } else {
        doc! {"_id":id,"$or":[{"owner":actor},{"shares":actor},{"visibility":"public","retired":false}]}
    }
}
fn validate_actor_key(owner: &str, key: &str) -> Result<()> {
    if !bounded_text(owner, 512) || !bounded_text(key, 256) {
        return Err(Error::Invalid("owner and idempotency key are required"));
    }
    Ok(())
}
fn same_image(existing: Image, desired: &Image) -> Result<Image> {
    if existing.request_sha256 == desired.request_sha256 {
        Ok(existing)
    } else {
        Err(Error::Conflict)
    }
}
fn same_build(existing: Build, desired: &Build) -> Result<Build> {
    if existing.request_sha256 == desired.request_sha256 {
        Ok(existing)
    } else {
        Err(Error::Conflict)
    }
}
