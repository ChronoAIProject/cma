use std::path::PathBuf;

pub(super) struct TemporaryUpload(Option<PathBuf>);

impl TemporaryUpload {
    pub(super) fn new(path: PathBuf) -> Self {
        Self(Some(path))
    }

    pub(super) fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for TemporaryUpload {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}
