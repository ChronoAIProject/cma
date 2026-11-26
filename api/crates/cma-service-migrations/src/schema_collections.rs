//! Collection validator inspection and fenced replacement.
use super::*;

/// Collection creation and validator replacement share the exact same definition.
pub async fn collection(
    db: &Database,
    name: &str,
    validator: Document,
    create: bool,
) -> mongodb::error::Result<()> {
    collection_with_predecessors(db, name, validator, create, Vec::new()).await
}

pub async fn collection_with_predecessors(
    db: &Database,
    name: &str,
    validator: Document,
    create: bool,
    predecessors: Vec<Document>,
) -> mongodb::error::Result<()> {
    collection_with_guarded_replacement(
        db,
        name,
        validator,
        create,
        predecessors,
        None,
        |validator| async move {
            command(db, doc! {"collMod":name,"validator":validator,"validationLevel":"strict","validationAction":"error"}).await?;
            Ok(())
        },
    )
    .await
}

/// An independently installed stronger validator is already compatible. Its
/// writer fence must also guard any replacement of a retained validator.
pub async fn collection_with_guarded_replacement<F>(
    db: &Database,
    name: &str,
    validator: Document,
    create: bool,
    predecessors: Vec<Document>,
    stronger: Option<&Document>,
    replace: impl FnOnce(Document) -> F,
) -> mongodb::error::Result<()>
where
    F: std::future::Future<Output = mongodb::error::Result<()>>,
{
    if describing() {
        if let Some(stronger) = stronger {
            record(doc! {"guarded_validator":name,"stronger":stronger})?;
        }
        return record(
            doc! {"schema_collection":name,"validator":validator,"create":create,"predecessors":predecessors,"validationLevel":"strict","validationAction":"error"},
        );
    }
    if inspecting() || effects::active() {
        match collection_state(db, name, &validator, &predecessors, stronger).await? {
            CollectionState::Current => return Ok(()),
            CollectionState::Absent if !create && !inspecting() => {
                return Err(invalid("required migration collection is absent"))
            }
            CollectionState::Absent | CollectionState::Predecessor if inspecting() => {
                missing();
                return Ok(());
            }
            _ => {}
        }
    }
    if create {
        let result = command(db, doc! {"create":name,"validator":validator.clone(),"validationLevel":"strict","validationAction":"error"}).await;
        match result {
            Ok(_) => return verify_collection(db, name, &validator, stronger).await,
            Err(error) if matches!(error.kind.as_ref(), mongodb::error::ErrorKind::Command(command) if command.code == 48) =>
                {}
            Err(error) => return Err(error),
        }
    }
    if effects::active() {
        match collection_state(db, name, &validator, &predecessors, stronger).await? {
            CollectionState::Current => return Ok(()),
            CollectionState::Predecessor => {}
            CollectionState::Absent => {
                return Err(invalid("required migration collection is absent"))
            }
        }
    }
    replace(validator.clone()).await?;
    verify_collection(db, name, &validator, stronger).await
}

async fn verify_collection(
    db: &Database,
    name: &str,
    validator: &Document,
    stronger: Option<&Document>,
) -> mongodb::error::Result<()> {
    if effects::active() {
        if collection_state(db, name, validator, &[], stronger).await? != CollectionState::Current {
            return Err(invalid("migration collection postcondition unavailable"));
        }
        effects::check_fence().await?;
    }
    Ok(())
}
#[derive(PartialEq)]
enum CollectionState {
    Absent,
    Current,
    Predecessor,
}
async fn collection_state(
    db: &Database,
    name: &str,
    validator: &Document,
    predecessors: &[Document],
    stronger: Option<&Document>,
) -> mongodb::error::Result<CollectionState> {
    let mut cursor = tokio::time::timeout(IO, async {
        db.list_collections().filter(doc! {"name":name}).await
    })
    .await
    .map_err(|_| invalid("migration collection catalog unavailable"))??;
    let Some(info) = cursor.try_next().await? else {
        return Ok(CollectionState::Absent);
    };
    if !matches!(
        info.collection_type,
        mongodb::results::CollectionType::Collection
    ) || info.info.read_only
    {
        return Err(invalid("unsupported migration collection type"));
    }
    let options = mongodb::bson::serialize_to_document(&info.options)
        .map_err(mongodb::error::Error::custom)?;
    if options.get_str("validationLevel").unwrap_or("strict") != "strict"
        || options.get_str("validationAction").unwrap_or("error") != "error"
    {
        return Err(invalid("migration validation mode drift"));
    }
    match options.get_document("validator").ok() {
        Some(actual) if actual == validator || stronger == Some(actual) => {
            Ok(CollectionState::Current)
        }
        Some(actual) if predecessors.contains(actual) => Ok(CollectionState::Predecessor),
        _ => Err(invalid("unrecognized migration validator")),
    }
}
