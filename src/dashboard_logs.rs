use std::collections::VecDeque;
use std::io::{self, Write};
use std::sync::{Arc, Mutex, OnceLock};

use tracing_subscriber::fmt::writer::MakeWriter;

const DEFAULT_LOG_CAPACITY: usize = 200;

static DASHBOARD_LOGS: OnceLock<Arc<DashboardLogBuffer>> = OnceLock::new();

#[derive(Debug)]
pub(crate) struct DashboardLogBuffer {
    capacity: usize,
    lines: Mutex<VecDeque<String>>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DashboardLogSnapshot {
    pub lines: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct DashboardLogMakeWriter {
    buffer: Arc<DashboardLogBuffer>,
}

#[derive(Debug)]
pub(crate) struct DashboardLogWriter {
    buffer: Arc<DashboardLogBuffer>,
    pending: String,
}

pub(crate) fn shared() -> Arc<DashboardLogBuffer> {
    DASHBOARD_LOGS
        .get_or_init(|| Arc::new(DashboardLogBuffer::new(DEFAULT_LOG_CAPACITY)))
        .clone()
}

pub(crate) fn make_writer() -> DashboardLogMakeWriter {
    DashboardLogMakeWriter::new(shared())
}

impl DashboardLogBuffer {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            lines: Mutex::new(VecDeque::new()),
        }
    }

    pub(crate) fn snapshot(&self) -> DashboardLogSnapshot {
        let lines = self.lines.lock().unwrap().iter().cloned().collect();
        DashboardLogSnapshot { lines }
    }

    fn push_line(&self, line: impl Into<String>) {
        let line = sanitize_log_line(&line.into());
        if line.is_empty() {
            return;
        }

        let mut lines = self.lines.lock().unwrap();
        if lines.len() == self.capacity {
            lines.pop_front();
        }
        lines.push_back(line);
    }
}

impl DashboardLogMakeWriter {
    pub(crate) fn new(buffer: Arc<DashboardLogBuffer>) -> Self {
        Self { buffer }
    }
}

impl<'a> MakeWriter<'a> for DashboardLogMakeWriter {
    type Writer = DashboardLogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        DashboardLogWriter::new(self.buffer.clone())
    }
}

impl DashboardLogWriter {
    pub(crate) fn new(buffer: Arc<DashboardLogBuffer>) -> Self {
        Self {
            buffer,
            pending: String::new(),
        }
    }

    fn drain_complete_lines(&mut self, flush_tail: bool) {
        while let Some(position) = self.pending.find('\n') {
            let mut line = self.pending.drain(..=position).collect::<String>();
            if line.ends_with('\n') {
                line.pop();
            }
            if line.ends_with('\r') {
                line.pop();
            }
            self.buffer.push_line(line);
        }

        if flush_tail && !self.pending.is_empty() {
            let line = std::mem::take(&mut self.pending);
            self.buffer.push_line(line);
        }
    }
}

impl Write for DashboardLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pending.push_str(&String::from_utf8_lossy(buf));
        self.drain_complete_lines(false);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.drain_complete_lines(true);
        Ok(())
    }
}

impl Drop for DashboardLogWriter {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

fn sanitize_log_line(line: &str) -> String {
    line.trim_matches(|ch| ch == '\n' || ch == '\r')
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::{DashboardLogBuffer, DashboardLogMakeWriter};
    use std::io::Write;
    use std::sync::Arc;
    use tracing_subscriber::fmt::writer::MakeWriter;

    #[test]
    fn buffer_keeps_only_recent_entries() {
        let buffer = DashboardLogBuffer::new(2);
        buffer.push_line("one");
        buffer.push_line("two");
        buffer.push_line("three");

        assert_eq!(
            buffer.snapshot().lines,
            vec!["two".to_string(), "three".to_string()]
        );
    }

    #[test]
    fn writer_coalesces_partial_lines_and_flushes_tail() {
        let buffer = Arc::new(DashboardLogBuffer::new(4));
        let mut writer = DashboardLogMakeWriter::new(buffer.clone()).make_writer();

        writer.write_all(b"WARN partial").unwrap();
        writer.write_all(b" line\nINFO tail").unwrap();
        drop(writer);

        assert_eq!(
            buffer.snapshot().lines,
            vec!["WARN partial line".to_string(), "INFO tail".to_string(),]
        );
    }
}
