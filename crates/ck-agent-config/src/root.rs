use std::path::PathBuf;

/// The folder that stands for `/`.
#[derive(Debug, Clone)]
pub struct Root(PathBuf);

impl Root {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }

    /// The real location of an absolute path such as `/etc/ck/config.json`.
    pub(crate) fn path(&self, absolute: &str) -> PathBuf {
        self.0.join(absolute.trim_start_matches('/'))
    }
}
