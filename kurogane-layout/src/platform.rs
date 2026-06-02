use std::path::PathBuf;

pub fn data_local_dir() -> PathBuf {
    dirs::data_local_dir().unwrap_or_else(std::env::temp_dir)
}

pub fn cache_dir() -> PathBuf {
    dirs::cache_dir().unwrap_or_else(std::env::temp_dir)
}
