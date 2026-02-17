// Content-based deduplication using blake3 hashing

use std::path::Path;
use thiserror::Error;

/// Errors during content hashing
#[derive(Debug, Error)]
pub enum HashError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("file size {size} bytes exceeds maximum {max_size} bytes")]
    FileTooLarge { size: u64, max_size: u64 },

    #[error("file not found: {0}")]
    NotFound(std::path::PathBuf),
}

/// Content-based deduplication using blake3 hashing.
///
/// Computes blake3 hashes of file contents to detect duplicate content
/// across the filesystem. Skips hashing files larger than max_size_bytes.
#[derive(Debug, Clone, Copy)]
pub struct ContentDedup {
    /// Maximum file size to hash in bytes
    max_size_bytes: u64,
}

impl ContentDedup {
    /// Create a new ContentDedup with the given maximum file size.
    ///
    /// Files larger than max_size_bytes will return HashError::FileTooLarge
    /// instead of being hashed.
    pub fn new(max_size_bytes: u64) -> Self {
        Self { max_size_bytes }
    }

    /// Get the maximum file size in bytes
    pub fn max_size_bytes(&self) -> u64 {
        self.max_size_bytes
    }

    /// Compute the blake3 hash of a file's contents.
    ///
    /// Returns a hex-encoded hash string. Fails if:
    /// - File doesn't exist
    /// - File is larger than max_size_bytes
    /// - IO error occurs
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let dedup = ContentDedup::new(1024);
    /// let hash = dedup.hash_file(Path::new("test.txt"))?;
    /// assert_eq!(hash.len(), 64); // blake3 hex = 64 chars
    /// ```
    pub fn hash_file(&self, path: &Path) -> Result<String, HashError> {
        // Check file exists
        if !path.exists() {
            return Err(HashError::NotFound(path.to_path_buf()));
        }

        // Check file size
        let metadata = std::fs::metadata(path)?;
        let file_size = metadata.len();
        if file_size > self.max_size_bytes {
            return Err(HashError::FileTooLarge {
                size: file_size,
                max_size: self.max_size_bytes,
            });
        }

        // Read and hash
        let content = std::fs::read(path)?;
        let hash = blake3::hash(&content);
        Ok(hash.to_hex().to_string())
    }

    /// Check if a file's hash matches an expected value.
    ///
    /// Returns true if the file's blake3 hash equals the expected hash.
    /// Returns false if hashes differ. Returns an error if:
    /// - File doesn't exist
    /// - File is too large
    /// - IO error occurs
    pub fn hash_matches(&self, path: &Path, expected: &str) -> Result<bool, HashError> {
        let actual = self.hash_file(path)?;
        Ok(actual == expected)
    }
}

impl Default for ContentDedup {
    fn default() -> Self {
        Self::new(10 * 1024 * 1024) // 10MB default
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn hash_empty_file() {
        let dedup = ContentDedup::new(1024);
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "").unwrap();

        let hash = dedup.hash_file(file.path()).unwrap();
        assert_eq!(hash.len(), 64); // blake3 hex is always 64 chars
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn hash_simple_content() {
        let dedup = ContentDedup::new(1024);
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "hello world").unwrap();

        let hash1 = dedup.hash_file(file.path()).unwrap();
        assert_eq!(hash1.len(), 64);

        // Same content should produce same hash
        let hash2 = dedup.hash_file(file.path()).unwrap();
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn hash_different_content_different_hash() {
        let dedup = ContentDedup::new(1024);

        let mut file1 = NamedTempFile::new().unwrap();
        writeln!(file1, "content A").unwrap();

        let mut file2 = NamedTempFile::new().unwrap();
        writeln!(file2, "content B").unwrap();

        let hash1 = dedup.hash_file(file1.path()).unwrap();
        let hash2 = dedup.hash_file(file2.path()).unwrap();

        assert_ne!(hash1, hash2);
    }

    #[test]
    fn hash_matches_returns_true_for_same_content() {
        let dedup = ContentDedup::new(1024);
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "test content").unwrap();

        let hash = dedup.hash_file(file.path()).unwrap();
        let matches = dedup.hash_matches(file.path(), &hash).unwrap();

        assert!(matches);
    }

    #[test]
    fn hash_matches_returns_false_for_different_hash() {
        let dedup = ContentDedup::new(1024);
        let file = NamedTempFile::new().unwrap();

        let matches = dedup.hash_matches(file.path(), "deadbeef").unwrap();
        assert!(!matches);
    }

    #[test]
    fn hash_file_that_doesnt_exist() {
        let dedup = ContentDedup::new(1024);
        let result = dedup.hash_file(Path::new("/nonexistent/file.txt"));

        assert!(matches!(result, Err(HashError::NotFound(_))));
    }

    #[test]
    fn hash_matches_file_that_doesnt_exist() {
        let dedup = ContentDedup::new(1024);
        let result = dedup.hash_matches(Path::new("/nonexistent/file.txt"), "hash");

        assert!(matches!(result, Err(HashError::NotFound(_))));
    }

    #[test]
    fn file_too_large_returns_error() {
        let dedup = ContentDedup::new(100); // max 100 bytes
        let mut file = NamedTempFile::new().unwrap();

        // Write 200 bytes
        file.write_all(&vec![b'a'; 200]).unwrap();

        let result = dedup.hash_file(file.path());
        assert!(matches!(result, Err(HashError::FileTooLarge { size, max_size }) if {
            size == 200 && max_size == 100
        }));
    }

    #[test]
    fn hash_matches_file_too_large() {
        let dedup = ContentDedup::new(100);
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(&vec![b'a'; 200]).unwrap();

        let result = dedup.hash_matches(file.path(), "hash");
        assert!(matches!(result, Err(HashError::FileTooLarge { .. })));
    }

    #[test]
    fn file_exactly_at_max_size_hashes() {
        let dedup = ContentDedup::new(100);
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(&vec![b'a'; 100]).unwrap();

        let hash = dedup.hash_file(file.path()).unwrap();
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn one_byte_under_max_size_hashes() {
        let dedup = ContentDedup::new(100);
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(&vec![b'a'; 99]).unwrap();

        let hash = dedup.hash_file(file.path()).unwrap();
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn max_size_bytes_returns_configured_value() {
        let dedup = ContentDedup::new(4200);
        assert_eq!(dedup.max_size_bytes(), 4200);
    }

    #[test]
    fn default_max_size_is_10mb() {
        let dedup = ContentDedup::default();
        assert_eq!(dedup.max_size_bytes(), 10 * 1024 * 1024);
    }

    #[test]
    fn hash_is_deterministic() {
        let dedup = ContentDedup::new(1024);

        let mut file1 = NamedTempFile::new().unwrap();
        writeln!(file1, "same content").unwrap();

        let mut file2 = NamedTempFile::new().unwrap();
        writeln!(file2, "same content").unwrap();

        let hash1 = dedup.hash_file(file1.path()).unwrap();
        let hash2 = dedup.hash_file(file2.path()).unwrap();

        assert_eq!(hash1, hash2);
    }

    #[test]
    fn hash_binary_content() {
        let dedup = ContentDedup::new(1024);
        let mut file = NamedTempFile::new().unwrap();

        // Write some raw bytes
        file.write_all(&[0x00, 0xFF, 0x42, 0x00]).unwrap();

        let hash = dedup.hash_file(file.path()).unwrap();
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn hash_unicode_content() {
        let dedup = ContentDedup::new(1024);
        let mut file = NamedTempFile::new().unwrap();

        // Mix of scripts
        writeln!(file, "Hello 世界 🌍 مرحبا").unwrap();

        let hash = dedup.hash_file(file.path()).unwrap();
        assert_eq!(hash.len(), 64);
    }
}
