macro_rules! create_record {
    ($state:expr, $collection:ident, $id:expr, $record:expr) => {{
        if $record.revision != 1 {
            return Err(SandboxStoreError::Conflict);
        }
        if $state.$collection.contains_key(&$id) {
            return Err(SandboxStoreError::Duplicate);
        }
        $state.$collection.insert($id, $record);
        Ok(())
    }};
}

macro_rules! save_record {
    ($state:expr, $collection:ident, $id:expr, $expected:expr, $record:expr) => {{
        let current = $state
            .$collection
            .get(&$id)
            .ok_or(SandboxStoreError::NotFound)?;
        let next_revision = $expected
            .checked_add(1)
            .ok_or(SandboxStoreError::Conflict)?;
        if current.revision != $expected || $record.revision != next_revision {
            return Err(SandboxStoreError::Conflict);
        }
        $state.$collection.insert($id, $record);
        Ok(())
    }};
}
