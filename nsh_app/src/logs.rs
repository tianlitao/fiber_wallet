//! In-memory tail of the app's tracing output, surfaced on 首页 → 节点事件.
//!
//! The binary installs a second `fmt` layer writing into [`LOG_TAIL`]; the
//! root poller copies the newest lines into [`crate::bridge::UiState::log_tail`]
//! a few times a second, so the card renders a rolling console of the same
//! lines the terminal prints (app + embedded Fiber node).

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex, OnceLock};

/// Kept lines between drains (the UI shows only the newest few).
const CAPACITY: usize = 400;

#[derive(Default)]
struct Buffer {
    /// A partially written line not yet terminated with `\n`.
    pending: String,
    lines: VecDeque<String>,
}

/// Cloneable handle to the shared log tail. Acts as a
/// `tracing_subscriber::fmt::MakeWriter`.
#[derive(Clone, Default)]
pub struct LogTail(Arc<Mutex<Buffer>>);

impl LogTail {
    pub fn new() -> Self {
        Self::default()
    }

    /// The newest `n` lines, oldest first.
    pub fn tail(&self, n: usize) -> Vec<String> {
        let buffer = self.0.lock().expect("log tail poisoned");
        let start = buffer.lines.len().saturating_sub(n);
        buffer.lines.iter().skip(start).cloned().collect()
    }
}

pub struct LogWriter(LogTail);

impl io::Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let chunk = String::from_utf8_lossy(buf);
        let mut buffer = self.0.0.lock().expect("log tail poisoned");
        buffer.pending.push_str(&chunk);
        while let Some(end) = buffer.pending.find('\n') {
            let raw: String = buffer.pending.drain(..=end).collect();
            let line = strip_ansi(raw.trim_end_matches(['\n', '\r']));
            if line.trim().is_empty() {
                continue;
            }
            if buffer.lines.len() >= CAPACITY {
                buffer.lines.pop_front();
            }
            buffer.lines.push_back(line);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Removes ANSI escape sequences (the FNN log lines carry SGR codes that the
/// UI's text renderer would otherwise show as `[3m`-style garbage).
fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        if let Some('[') = chars.next() {
            // CSI: parameter and intermediate bytes up to the final byte.
            for c in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    break;
                }
            }
        }
    }
    out
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogTail {
    type Writer = LogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        LogWriter(self.clone())
    }
}

/// The global tail the binary's memory layer writes into.
pub static LOG_TAIL: OnceLock<LogTail> = OnceLock::new();

/// Returns the global tail, creating it on first use. Must match the instance
/// installed into the subscriber in `main`.
pub fn global() -> &'static LogTail {
    LOG_TAIL.get_or_init(LogTail::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::fmt::MakeWriter as _;

    #[test]
    fn writes_are_split_into_lines_and_capped() {
        let tail = LogTail::new();
        let mut writer = tail.make_writer();
        use io::Write as _;
        writer.write_all(b"first line\nsecond ").unwrap();
        writer.write_all(b"line\nthird").unwrap();
        writer.write_all(b"\n\n\n").unwrap();
        assert_eq!(tail.tail(10), ["first line", "second line", "third"]);
    }

    #[test]
    fn ansi_sequences_are_stripped_from_lines() {
        let tail = LogTail::new();
        let mut writer = tail.make_writer();
        use io::Write as _;
        // A fmt-style line: dim timestamp, red level, italic field names.
        writer
            .write_all(
                "\x1b[2m2026-10-25T00:00:00Z\x1b[0m \x1b[31mERROR\x1b[0m Actor{\x1b[3mid\x1b[0m\x1b[2m=\x1b[0m\"0.3\"}: hello\n"
                    .as_bytes(),
            )
            .unwrap();
        let lines = tail.tail(5);
        assert_eq!(
            lines,
            ["2026-10-25T00:00:00Z ERROR Actor{id=\"0.3\"}: hello"]
        );
        assert!(!lines[0].contains('\x1b'));
        assert!(!lines[0].contains("[3m"));
        // A sequence split across two writes is still removed cleanly.
        writer.write_all("\x1b[3".as_bytes()).unwrap();
        writer.write_all("2mgreen\x1b[0m\n".as_bytes()).unwrap();
        assert_eq!(tail.tail(1), ["green"]);
    }

    #[test]
    fn capacity_evicts_the_oldest_lines() {
        let tail = LogTail::new();
        let mut writer = tail.make_writer();
        use io::Write as _;
        for i in 0..(CAPACITY + 10) {
            writeln!(writer, "line {i}").unwrap();
        }
        let lines = tail.tail(CAPACITY + 5);
        assert_eq!(lines.len(), CAPACITY);
        assert_eq!(lines.last().unwrap(), &format!("line {}", CAPACITY + 9));
        assert_eq!(lines.first().unwrap(), "line 10");
    }
}
