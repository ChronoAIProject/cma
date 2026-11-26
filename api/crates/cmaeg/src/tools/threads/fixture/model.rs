//! State owned by the native thread transport fixture.
use super::*;

pub struct Memory {
    pub calls: Vec<(String, String, Value)>,
    pub channels: BTreeMap<String, Value>,
    pub active: Value,
    pub archived: Value,
    pub message: Value,
    pub delay: Option<(String, String, Duration)>,
    pub status: StatusCode,
    pub create_accepted: bool,
    pub patch_response: Option<Value>,
    pub hook: Option<Box<dyn FnOnce() + Send>>,
    pub hook_path: String,
    pub hook_method: String,
}
pub struct App {
    pub backend: Arc<fake_cma::FakeCma>,
    pub(super) platform_base: String,
    pub memory: Arc<Mutex<Memory>>,
    pub state: ToolsState,
    pub definition: ChannelDefinition,
    pub event: EventRefPayload,
    pub activity: Activity,
    pub key: String,
    pub base: String,
    pub source_body: Value,
    pub(super) servers: Vec<JoinHandle<()>>,
}
impl Drop for App {
    fn drop(&mut self) {
        for server in &self.servers {
            server.abort();
        }
    }
}
