//! Structured logging initialization and the queryable log file behind `jarvis logs`.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use jarvis_core::LogLevel;
use thiserror::Error;
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

use crate::redact::Redactor;

/// Filename of the current daemon log inside the log directory.
pub const LOG_FILE_NAME: &str = "jarvisd.jsonl";

/// Maximum number of lines `jarvis logs` returns by default.
pub const DEFAULT_TAIL_LINES: usize = 200;

/// Maximum number of lines `jarvis logs` will ever return.
pub const MAX_TAIL_LINES: usize = 5_000;

/// Explains why logging could not be configured or read.
#[derive(Debug, Error)]
pub enum LoggingError {
    /// The log directory could not be created.
    #[error("failed to create the log directory")]
    Directory {
        /// The underlying operating-system error.
        #[source]
        source: io::Error,
    },
    /// The log file could not be opened or written.
    #[error("failed to access the log file")]
    File {
        /// The underlying operating-system error.
        #[source]
        source: io::Error,
    },
    /// A global tracing subscriber was already installed.
    #[error("a global tracing subscriber is already installed")]
    AlreadyInstalled,
    /// The configured log level was not a valid filter directive.
    #[error("the configured log level is not a valid filter")]
    InvalidLevel,
}

/// Shared append-only sink that redacts every buffer before it reaches the file.
///
/// `tracing` calls the writer once per formatted event, so applying redaction
/// here means a call site cannot bypass it by forgetting a rule.
#[derive(Clone, Debug)]
struct RedactingSink {
    file: Arc<Mutex<File>>,
    redactor: Redactor,
}

impl Write for RedactingSink {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let text = String::from_utf8_lossy(buffer);
        let redacted = self.redactor.redact_line(&text);
        let mut guard = self
            .file
            .lock()
            .map_err(|_| io::Error::other("log sink lock poisoned"))?;
        guard.write_all(redacted.as_bytes())?;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut guard = self
            .file
            .lock()
            .map_err(|_| io::Error::other("log sink lock poisoned"))?;
        guard.flush()
    }
}

impl<'writer> fmt::MakeWriter<'writer> for RedactingSink {
    type Writer = Self;

    fn make_writer(&'writer self) -> Self::Writer {
        self.clone()
    }
}

/// Owns the log file handle and the redactor applied to every recorded line.
#[derive(Debug)]
pub struct Logging {
    path: PathBuf,
    sink: RedactingSink,
}

impl Logging {
    /// Opens the log file and installs the global subscriber.
    ///
    /// Human-readable output goes to stderr and structured JSON goes to the log
    /// file, both through the same redactor, so the two renderings cannot drift.
    ///
    /// # Errors
    ///
    /// Returns [`LoggingError`] when the directory or file cannot be created, the
    /// level is not a valid filter, or a subscriber is already installed.
    pub fn init(
        log_directory: &Path,
        level: LogLevel,
        redactor: Redactor,
    ) -> Result<Self, LoggingError> {
        fs::create_dir_all(log_directory).map_err(|source| LoggingError::Directory { source })?;
        let path = log_directory.join(LOG_FILE_NAME);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|source| LoggingError::File { source })?;

        let sink = RedactingSink {
            file: Arc::new(Mutex::new(file)),
            redactor,
        };
        let filter = EnvFilter::try_new(level.as_str()).map_err(|_| LoggingError::InvalidLevel)?;

        let file_layer = fmt::layer()
            .json()
            .with_writer(sink.clone())
            .with_ansi(false);
        let stderr_layer = fmt::layer().with_writer(io::stderr).with_ansi(true);

        tracing_subscriber::registry()
            .with(filter)
            .with(file_layer)
            .with(stderr_layer)
            .try_init()
            .map_err(|_| LoggingError::AlreadyInstalled)?;

        Ok(Self { path, sink })
    }

    /// Returns the log file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reports how many known secrets the sink masks.
    #[must_use]
    pub fn secret_count(&self) -> usize {
        self.sink.redactor.secret_count()
    }

    /// Redacts a line exactly as the sink does, for diagnostics and tests.
    #[must_use]
    pub fn redact(&self, line: &str) -> String {
        self.sink.redactor.redact_line(line)
    }

    /// Flushes buffered log records to disk.
    ///
    /// # Errors
    ///
    /// Returns [`LoggingError::File`] when the file cannot be flushed.
    pub fn flush(&self) -> Result<(), LoggingError> {
        let mut guard = self.sink.file.lock().map_err(|_| LoggingError::File {
            source: io::Error::other("log sink lock poisoned"),
        })?;
        guard
            .flush()
            .map_err(|source| LoggingError::File { source })
    }
}

/// One line of the structured log, already redacted on disk.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogLine {
    /// One-based position of the line within the file.
    pub number: u64,
    /// Raw JSON text of the line without its trailing newline.
    pub text: String,
}

/// Reads the tail of the structured log without loading the whole file.
///
/// A missing log returns an empty result rather than an error, because a daemon
/// that has never run has simply produced nothing yet.
///
/// # Errors
///
/// Returns [`LoggingError::File`] when the log exists but cannot be read.
pub fn read_tail(path: &Path, lines: usize) -> Result<Vec<LogLine>, LoggingError> {
    let bounded = lines.clamp(1, MAX_TAIL_LINES);
    let file = match File::open(path) {
        Ok(file) => file,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(LoggingError::File { source }),
    };

    let mut reader = BufReader::new(file);
    let total_bytes = reader
        .get_ref()
        .metadata()
        .map_err(|source| LoggingError::File { source })?
        .len();
    let window: u64 = 64 * 1024;
    let mut reversed: Vec<String> = Vec::new();
    let mut tail_fragment = String::new();
    let mut end = total_bytes;

    while end > 0 && reversed.len() < bounded {
        let start = end.saturating_sub(window);
        let length = usize::try_from(end - start).unwrap_or(usize::MAX);
        reader
            .seek(SeekFrom::Start(start))
            .map_err(|source| LoggingError::File { source })?;
        let mut chunk = vec![0_u8; length];
        io::Read::read_exact(&mut reader, &mut chunk)
            .map_err(|source| LoggingError::File { source })?;

        // A window boundary can split a line, so the incomplete head of this
        // window is completed with the fragment carried from the later window.
        let mut combined = String::from_utf8_lossy(&chunk).into_owned();
        if !tail_fragment.is_empty() {
            combined.push_str(&tail_fragment);
            tail_fragment.clear();
        }
        if start > 0 {
            if let Some(last_newline) = combined.rfind('\n') {
                tail_fragment = combined[last_newline + 1..].to_owned();
                combined.truncate(last_newline + 1);
            } else {
                tail_fragment = combined;
                combined = String::new();
            }
        }
        for line in combined.lines().rev() {
            if reversed.len() >= bounded {
                break;
            }
            if !line.trim().is_empty() {
                reversed.push(line.to_owned());
            }
        }
        end = start;
    }

    let total_lines = count_lines(path)?;
    let kept = u64::try_from(reversed.len()).unwrap_or(u64::MAX);
    let first_number = total_lines.saturating_sub(kept) + 1;

    Ok(reversed
        .into_iter()
        .rev()
        .enumerate()
        .map(|(index, text)| LogLine {
            number: first_number + u64::try_from(index).unwrap_or(0),
            text,
        })
        .collect())
}

/// Counts newline-delimited records.
///
/// # Errors
///
/// Returns [`LoggingError::File`] when the log exists but cannot be read.
pub fn count_lines(path: &Path) -> Result<u64, LoggingError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(source) => return Err(LoggingError::File { source }),
    };
    let mut count = 0_u64;
    for line in BufReader::new(file).lines() {
        line.map_err(|source| LoggingError::File { source })?;
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;

    struct TempDirectory(PathBuf);

    impl TempDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "jarvis-observability-logging-{}",
                jarvis_core::scratch_tag()
            ));
            fs::create_dir_all(&path).unwrap_or_else(|error| panic!("create temp dir: {error}"));
            Self(path)
        }

        fn file(&self) -> PathBuf {
            self.0.join(LOG_FILE_NAME)
        }
    }

    impl Drop for TempDirectory {
        fn drop(&mut self) {
            jarvis_core::remove_scratch_dir(&self.0);
        }
    }

    #[test]
    fn a_missing_log_reads_as_empty_rather_than_erroring() {
        let directory = TempDirectory::new();
        let tail = read_tail(&directory.file(), DEFAULT_TAIL_LINES)
            .unwrap_or_else(|error| panic!("read: {error}"));
        assert!(tail.is_empty());
        assert_eq!(
            count_lines(&directory.file()).unwrap_or_else(|error| panic!("count: {error}")),
            0
        );
    }

    #[test]
    fn the_tail_returns_the_last_lines_with_true_positions() {
        let directory = TempDirectory::new();
        let mut content = String::new();
        for index in 1..=50 {
            writeln!(content, "{{\"n\":{index}}}").unwrap_or_else(|_| unreachable!());
        }
        fs::write(directory.file(), content)
            .unwrap_or_else(|error| panic!("write fixture: {error}"));

        let tail = read_tail(&directory.file(), 3).unwrap_or_else(|error| panic!("read: {error}"));
        assert_eq!(tail.len(), 3);
        assert_eq!(tail[0].text, r#"{"n":48}"#);
        assert_eq!(tail[0].number, 48);
        assert_eq!(tail[2].text, r#"{"n":50}"#);
        assert_eq!(tail[2].number, 50);
    }

    #[test]
    fn reading_a_tail_across_a_window_boundary_keeps_whole_lines() {
        let directory = TempDirectory::new();
        let filler = "x".repeat(1024);
        let mut content = String::new();
        for index in 1..=200 {
            writeln!(content, "{{\"n\":{index},\"pad\":\"{filler}\"}}")
                .unwrap_or_else(|_| unreachable!());
        }
        fs::write(directory.file(), content)
            .unwrap_or_else(|error| panic!("write fixture: {error}"));

        // The file is far larger than the 64 KiB read window, so boundaries are crossed.
        let tail = read_tail(&directory.file(), 2).unwrap_or_else(|error| panic!("read: {error}"));
        assert_eq!(tail.len(), 2);
        assert_eq!(tail[1].number, 200);
        for line in &tail {
            assert!(
                line.text.starts_with('{') && line.text.ends_with('}'),
                "line was split at a window boundary: {line:?}"
            );
        }
    }

    #[test]
    fn the_sink_redacts_before_bytes_reach_the_file() {
        let directory = TempDirectory::new();
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(directory.file())
            .unwrap_or_else(|error| panic!("open: {error}"));
        let secret = "9f4c1d2e8b7a6350f1e2d3c4b5a69788";
        let mut sink = RedactingSink {
            file: Arc::new(Mutex::new(file)),
            redactor: Redactor::new([secret]),
        };

        sink.write_all(format!("credential={secret}\n").as_bytes())
            .unwrap_or_else(|error| panic!("write: {error}"));
        sink.flush()
            .unwrap_or_else(|error| panic!("flush: {error}"));

        let written = fs::read_to_string(directory.file())
            .unwrap_or_else(|error| panic!("read file: {error}"));
        assert!(!written.contains(secret), "the sink leaked: {written:?}");
        assert!(written.contains("REDACTED"));
    }
}
