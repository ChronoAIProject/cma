//! Verified raw evidence is reusable; redaction and release authority are not.
use super::*;
use crate::sandbox::{
    agent_observation::output::OutputFlight,
    agent_output_index::{AgentOutputChange, AgentToolOutputState},
    store::{
        indexed::{AgentOutputMetadata, AgentOutputReader},
        AgentResponseLogStore,
    },
};
use std::collections::BTreeMap;

#[derive(Clone, serde::Serialize)]
struct Memo {
    metadata: AgentOutputMetadata,
    changes: BTreeMap<String, Vec<AgentOutputChange>>,
    tools: BTreeMap<String, AgentToolOutputState>,
    points: BTreeMap<u64, AgentLogEntry>,
}
impl Memo {
    fn bytes(&self) -> usize {
        serde_json::to_vec(self).map_or(usize::MAX, |bytes| bytes.len())
    }
    fn bound(&mut self) {
        if self.bytes() > crate::sandbox::agent_observation::MAX_PAGE_BYTES {
            self.changes.clear();
            self.tools.clear();
            self.points.clear();
        }
    }
}

pub(super) struct MemoReader<'a> {
    store: &'a dyn AgentResponseLogStore,
    agent: &'a Agent,
    turn: Option<u64>,
    reader: Option<Box<dyn AgentOutputReader + 'a>>,
    memo: Arc<std::sync::Mutex<Memo>>,
    metadata: AgentOutputMetadata,
}

impl<'a> MemoReader<'a> {
    pub(super) async fn open(
        store: &'a dyn AgentResponseLogStore,
        agent: &'a Agent,
        turn: Option<u64>,
        flight: &OutputFlight<'_>,
    ) -> Result<Option<Self>, AgentReadError> {
        if let Some(memo) = flight.get::<Memo>(b"indexed") {
            return Ok(Some(Self {
                store,
                agent,
                turn,
                reader: None,
                metadata: memo.metadata.clone(),
                memo: Arc::new(std::sync::Mutex::new((*memo).clone())),
            }));
        }
        let Some(reader) = store
            .agent_output_reader(agent, turn)
            .await
            .map_err(capture_open_error)?
        else {
            return Ok(None);
        };
        let metadata = reader.metadata().clone();
        Ok(Some(Self {
            store,
            agent,
            turn,
            reader: Some(reader),
            metadata: metadata.clone(),
            memo: Arc::new(std::sync::Mutex::new(Memo {
                metadata,
                changes: BTreeMap::new(),
                tools: BTreeMap::new(),
                points: BTreeMap::new(),
            })),
        }))
    }
    pub(super) fn commit(&self) -> impl FnOnce(&OutputFlight<'_>) + use<> {
        let memo = self.memo.clone();
        move |flight| {
            let memo = memo.lock().unwrap().clone();
            if memo.bytes() <= crate::sandbox::agent_observation::MAX_PAGE_BYTES {
                flight.insert(b"indexed", memo);
            }
        }
    }
    async fn reader(&mut self) -> Result<&mut Box<dyn AgentOutputReader + 'a>, SandboxStoreError> {
        if self.reader.is_none() {
            self.reader = Some(
                self.store
                    .agent_output_reader(self.agent, self.turn)
                    .await?
                    .ok_or_else(|| {
                        SandboxStoreError::Corrupt("indexed source disappeared".into())
                    })?,
            );
            if serde_json::to_vec(self.reader.as_ref().unwrap().metadata()).ok()
                != serde_json::to_vec(&self.metadata).ok()
            {
                return Err(SandboxStoreError::SequenceConflict);
            }
        }
        Ok(self.reader.as_mut().unwrap())
    }
}
#[async_trait::async_trait]
impl AgentOutputReader for MemoReader<'_> {
    fn metadata(&self) -> &AgentOutputMetadata {
        &self.metadata
    }
    async fn changes(
        &mut self,
        from: u64,
        limit: u16,
    ) -> Result<Vec<AgentOutputChange>, SandboxStoreError> {
        if let Some(value) = self
            .memo
            .lock()
            .unwrap()
            .changes
            .get(&format!("{from}:{limit}"))
        {
            return Ok(value.clone());
        }
        let value = self.reader().await?.changes(from, limit).await?;
        self.memo
            .lock()
            .unwrap()
            .changes
            .insert(format!("{from}:{limit}"), value.clone());
        self.memo.lock().unwrap().bound();
        Ok(value)
    }
    async fn tool(&mut self, call: &str) -> Result<AgentToolOutputState, SandboxStoreError> {
        if let Some(value) = self.memo.lock().unwrap().tools.get(call) {
            return Ok(value.clone());
        }
        let value = self.reader().await?.tool(call).await?;
        self.memo
            .lock()
            .unwrap()
            .tools
            .insert(call.to_owned(), value.clone());
        self.memo.lock().unwrap().bound();
        Ok(value)
    }
    async fn points(&mut self, sequences: &[u64]) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        let cached: Option<Vec<_>> = {
            let memo = self.memo.lock().unwrap();
            sequences
                .iter()
                .map(|seq| memo.points.get(seq).cloned())
                .collect()
        };
        if let Some(rows) = cached {
            return Ok(rows);
        }
        let rows = self.reader().await?.points(sequences).await?;
        self.memo
            .lock()
            .unwrap()
            .points
            .extend(rows.iter().map(|row| (row.seq, row.clone())));
        self.memo.lock().unwrap().bound();
        Ok(rows)
    }
    async fn source_page(
        &mut self,
        after: u64,
        limit: u16,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        self.reader().await?.source_page(after, limit).await
    }
    async fn open_message(&mut self) -> Result<String, SandboxStoreError> {
        self.reader().await?.open_message().await
    }
}
