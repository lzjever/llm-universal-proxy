use std::io;
use std::sync::Arc;
use std::time::Duration;

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::{Alignment, Color, CrosstermBackend, Line, Modifier, Span, Style},
    text::Text,
    widgets::{Block, Borders, Cell, Gauge, Paragraph, Row, Sparkline, Table, Wrap},
    Terminal,
};

use crate::config::{AuthPolicy, Config};
use crate::dashboard_logs::DashboardLogSnapshot;
use crate::hooks::HookSnapshot;
use crate::server::{DashboardNamespaceSnapshot, DashboardRuntimeHandle, DashboardUpstreamStatus};
use crate::telemetry::{RequestOutcome, RuntimeMetrics};

pub(crate) async fn run_dashboard(
    runtime: DashboardRuntimeHandle,
    metrics: Arc<RuntimeMetrics>,
) -> io::Result<()> {
    tokio::task::spawn_blocking(move || run_dashboard_blocking(runtime, metrics))
        .await
        .unwrap_or_else(|join_err| Err(io::Error::other(join_err.to_string())))
}

fn run_dashboard_blocking(
    runtime: DashboardRuntimeHandle,
    metrics: Arc<RuntimeMetrics>,
) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let result = dashboard_loop(&mut terminal, runtime, metrics);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn dashboard_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    runtime: DashboardRuntimeHandle,
    metrics: Arc<RuntimeMetrics>,
) -> io::Result<()> {
    loop {
        let snapshot = runtime.snapshot();
        let metrics_snapshot = metrics.snapshot(&snapshot.config);
        let log_snapshot = crate::dashboard_logs::shared().snapshot();
        terminal
            .draw(|frame| draw_dashboard(frame, &snapshot, &metrics_snapshot, &log_snapshot))?;

        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                        _ => {}
                    }
                }
            }
        }
    }
}

fn draw_dashboard(
    frame: &mut ratatui::Frame<'_>,
    runtime_snapshot: &DashboardNamespaceSnapshot,
    metrics_snapshot: &crate::telemetry::MetricsSnapshot,
    log_snapshot: &DashboardLogSnapshot,
) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(Color::Rgb(14, 17, 24))),
        area,
    );

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Min(10),
        ])
        .split(area);

    render_header(frame, vertical[0], metrics_snapshot);
    render_summary(
        frame,
        vertical[1],
        &runtime_snapshot.config,
        metrics_snapshot,
        runtime_snapshot.hooks.as_ref(),
    );
    render_config(
        frame,
        vertical[2],
        &runtime_snapshot.config,
        &runtime_snapshot.upstreams,
        runtime_snapshot.hooks.as_ref(),
    );

    if vertical[3].height >= 18 {
        let lower = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(10), Constraint::Length(8)])
            .split(vertical[3]);
        let activity = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(56), Constraint::Percentage(44)])
            .split(lower[0]);
        render_upstreams(frame, activity[0], metrics_snapshot);
        render_recent(frame, activity[1], metrics_snapshot, None);
        render_logs(frame, lower[1], log_snapshot);
    } else {
        let lower = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(56), Constraint::Percentage(44)])
            .split(vertical[3]);
        render_upstreams(frame, lower[0], metrics_snapshot);
        render_recent(frame, lower[1], metrics_snapshot, Some(log_snapshot));
    }
}

fn render_header(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    snapshot: &crate::telemetry::MetricsSnapshot,
) {
    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            "Proxec Dashboard",
            Style::default()
                .fg(Color::Rgb(248, 208, 111))
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            format!("uptime {}", format_duration(snapshot.uptime_secs)),
            Style::default().fg(Color::Rgb(143, 199, 255)),
        ),
        Span::raw("  "),
        Span::styled(
            "press q to exit",
            Style::default().fg(Color::Rgb(142, 150, 170)),
        ),
    ]))
    .block(panel("Overview"))
    .alignment(Alignment::Left);
    frame.render_widget(title, area);
}

fn render_summary(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    config: &Config,
    snapshot: &crate::telemetry::MetricsSnapshot,
    hooks: Option<&HookSnapshot>,
) {
    let sections = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(26),
            Constraint::Percentage(26),
            Constraint::Percentage(24),
            Constraint::Percentage(24),
        ])
        .split(area);

    let success_rate = if snapshot.total_requests == 0 {
        0.0
    } else {
        let attempts = snapshot.success_responses + snapshot.error_responses;
        if attempts == 0 {
            0.0
        } else {
            snapshot.success_responses as f64 / attempts as f64
        }
    };
    let pending_ratio = hooks
        .map(|hook| {
            if hook.max_pending_bytes == 0 {
                0.0
            } else {
                hook.pending_bytes as f64 / hook.max_pending_bytes as f64
            }
        })
        .unwrap_or(0.0);

    frame.render_widget(
        stat_paragraph(
            "Traffic",
            vec![
                line_value("Total Requests", snapshot.total_requests.to_string()),
                line_value("Active Requests", snapshot.active_requests.to_string()),
                line_value(
                    "Active Streams",
                    snapshot.active_stream_requests.to_string(),
                ),
                line_value(
                    "Configured Upstreams",
                    snapshot.configured_upstreams.to_string(),
                ),
            ],
        ),
        sections[0],
    );
    frame.render_widget(
        stat_paragraph(
            "Reliability",
            vec![
                line_value("Success", snapshot.success_responses.to_string()),
                line_value("Errors", snapshot.error_responses.to_string()),
                line_value("Cancelled", snapshot.cancelled_responses.to_string()),
                line_value("Success Rate", format!("{:.1}%", success_rate * 100.0)),
                line_value("Aliases", snapshot.configured_aliases.to_string()),
            ],
        ),
        sections[1],
    );
    frame.render_widget(
        Gauge::default()
            .block(panel("Hooks Buffer"))
            .gauge_style(
                Style::default()
                    .fg(Color::Rgb(116, 192, 252))
                    .bg(Color::Rgb(24, 30, 42)),
            )
            .label(if let Some(hook) = hooks {
                format!(
                    "{} / {}",
                    human_bytes(hook.pending_bytes),
                    human_bytes(hook.max_pending_bytes)
                )
            } else {
                "disabled".to_string()
            })
            .ratio(pending_ratio.clamp(0.0, 1.0)),
        sections[2],
    );
    frame.render_widget(
        stat_paragraph(
            "Hooks",
            vec![
                line_value(
                    "Exchange",
                    hook_enabled_label(config.hooks.exchange.is_some(), hooks.map(|v| &v.exchange)),
                ),
                line_value(
                    "Usage",
                    hook_enabled_label(config.hooks.usage.is_some(), hooks.map(|v| &v.usage)),
                ),
                line_value(
                    "Hook Timeout",
                    format!("{}s", config.hooks.timeout.as_secs()),
                ),
                line_value("Cooldown", format!("{}s", config.hooks.cooldown.as_secs())),
            ],
        ),
        sections[3],
    );
}

fn render_config(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    config: &Config,
    upstream_statuses: &[DashboardUpstreamStatus],
    hooks: Option<&HookSnapshot>,
) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(34),
            Constraint::Percentage(33),
            Constraint::Percentage(33),
        ])
        .split(area);

    let aliases = if config.model_aliases.is_empty() {
        "No aliases configured".to_string()
    } else {
        config
            .model_aliases
            .iter()
            .take(8)
            .map(|(alias, target)| {
                format!(
                    "{alias} -> {}:{}",
                    target.upstream_name, target.upstream_model
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    frame.render_widget(
        Paragraph::new(Text::from(aliases))
            .block(panel("Model Aliases"))
            .wrap(Wrap { trim: false }),
        chunks[0],
    );

    let upstream_summary = config
        .upstreams
        .iter()
        .map(|upstream| {
            let mode = match upstream.auth_policy {
                AuthPolicy::ClientOrFallback => "client_or_fallback",
                AuthPolicy::ForceServer => "force_server",
            };
            let format = upstream
                .fixed_upstream_format
                .map(|value| format!("{value:?}"))
                .unwrap_or_else(|| "auto".to_string());
            let availability = upstream_statuses
                .iter()
                .find(|status| status.name == upstream.name)
                .map(format_dashboard_availability)
                .unwrap_or_else(|| "status unknown".to_string());
            format!("{}  [{format}]  {mode}  {availability}", upstream.name)
        })
        .collect::<Vec<_>>()
        .join("\n");
    frame.render_widget(
        Paragraph::new(Text::from(upstream_summary))
            .block(panel("Routing"))
            .wrap(Wrap { trim: false }),
        chunks[1],
    );

    let hook_lines = if let Some(snapshot) = hooks {
        format!(
            "Pending Bytes: {}\nFailure Threshold: {}\nExchange Breaker: {}\nUsage Breaker: {}",
            human_bytes(snapshot.pending_bytes),
            snapshot.failure_threshold,
            circuit_label(&snapshot.exchange),
            circuit_label(&snapshot.usage),
        )
    } else {
        "Hooks disabled".to_string()
    };
    frame.render_widget(
        Paragraph::new(Text::from(hook_lines))
            .block(panel("Hook State"))
            .wrap(Wrap { trim: false }),
        chunks[2],
    );
}

fn format_dashboard_availability(status: &DashboardUpstreamStatus) -> String {
    match status.availability_reason.as_deref() {
        Some(reason) => format!("{} ({reason})", status.availability_status),
        None => status.availability_status.clone(),
    }
}

fn render_upstreams(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    snapshot: &crate::telemetry::MetricsSnapshot,
) {
    let rows = snapshot.upstreams.iter().map(|(name, stats)| {
        Row::new(vec![
            Cell::from(name.clone()),
            Cell::from(stats.total_requests.to_string()),
            Cell::from(stats.active_requests.to_string()),
            Cell::from(stats.stream_requests.to_string()),
            Cell::from(stats.success_responses.to_string()),
            Cell::from(stats.error_responses.to_string()),
            Cell::from(stats.cancelled_responses.to_string()),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Percentage(30),
            Constraint::Percentage(14),
            Constraint::Percentage(14),
            Constraint::Percentage(14),
            Constraint::Percentage(14),
            Constraint::Percentage(14),
            Constraint::Percentage(14),
        ],
    )
    .header(
        Row::new(vec![
            "Upstream", "Total", "Active", "Streams", "OK", "Err", "Cancel",
        ])
        .style(
            Style::default()
                .fg(Color::Rgb(248, 208, 111))
                .add_modifier(Modifier::BOLD),
        ),
    )
    .block(panel("Per-Upstream Traffic"))
    .column_spacing(1);
    frame.render_widget(table, area);
}

fn render_recent(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    snapshot: &crate::telemetry::MetricsSnapshot,
    logs: Option<&DashboardLogSnapshot>,
) {
    let constraints = if logs.is_some() {
        vec![
            Constraint::Length(5),
            Constraint::Min(8),
            Constraint::Length(8),
        ]
    } else {
        vec![Constraint::Length(5), Constraint::Min(8)]
    };
    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    render_latency(frame, inner[0], snapshot);
    render_recent_requests(frame, inner[1], snapshot);
    if let Some(logs) = logs {
        render_logs(frame, inner[2], logs);
    }
}

fn render_latency(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    snapshot: &crate::telemetry::MetricsSnapshot,
) {
    let history: Vec<u64> = snapshot
        .recent_requests
        .iter()
        .rev()
        .map(|item| item.duration_ms.min(u64::MAX as u128) as u64)
        .collect();
    let latest = snapshot
        .recent_requests
        .first()
        .map(|item| item.duration_ms);
    let title = latest
        .map(|value| format!("Latency Trend (latest {value} ms)"))
        .unwrap_or_else(|| "Latency Trend".to_string());
    let sparkline = Sparkline::default()
        .block(panel(title.as_str()))
        .style(Style::default().fg(Color::Rgb(106, 227, 199)))
        .max(max_latency(snapshot).min(u64::MAX as u128) as u64)
        .data(if history.is_empty() { vec![0] } else { history });
    frame.render_widget(sparkline, area);
}

fn render_recent_requests(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    snapshot: &crate::telemetry::MetricsSnapshot,
) {
    let rows = snapshot.recent_requests.iter().map(|req| {
        let (status_text, status_style) = match req.outcome {
            RequestOutcome::Success => (
                req.status.to_string(),
                Style::default().fg(Color::Rgb(113, 221, 130)),
            ),
            RequestOutcome::Error => (
                req.status.to_string(),
                Style::default().fg(Color::Rgb(255, 123, 114)),
            ),
            RequestOutcome::Cancelled => (
                "cancel".to_string(),
                Style::default().fg(Color::Rgb(255, 184, 108)),
            ),
        };
        Row::new(vec![
            Cell::from(truncate_text(&req.path, 22)),
            Cell::from(truncate_text(&req.client_model, 20)),
            Cell::from(truncate_text(
                req.upstream_name.as_deref().unwrap_or("-"),
                14,
            )),
            Cell::from(if req.stream { "Y" } else { "-" }),
            Cell::from(status_text).style(status_style),
            Cell::from(req.duration_ms.to_string()),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Percentage(24),
            Constraint::Percentage(24),
            Constraint::Percentage(18),
            Constraint::Percentage(8),
            Constraint::Percentage(10),
            Constraint::Percentage(16),
        ],
    )
    .header(
        Row::new(vec!["Path", "Model", "Upstream", "SSE", "Code", "Ms"]).style(
            Style::default()
                .fg(Color::Rgb(248, 208, 111))
                .add_modifier(Modifier::BOLD),
        ),
    )
    .block(panel("Recent Requests"))
    .column_spacing(1);
    frame.render_widget(table, area);
}

fn render_logs(frame: &mut ratatui::Frame<'_>, area: Rect, snapshot: &DashboardLogSnapshot) {
    let content_width = area.width.saturating_sub(4) as usize;
    let visible_lines = area.height.saturating_sub(2) as usize;
    let entries = snapshot
        .lines
        .iter()
        .rev()
        .take(visible_lines)
        .cloned()
        .collect::<Vec<_>>();

    let lines = if entries.is_empty() {
        vec![Line::styled(
            "No runtime logs yet",
            Style::default().fg(Color::Rgb(142, 150, 170)),
        )]
    } else {
        entries
            .into_iter()
            .rev()
            .map(|entry| {
                let truncated = truncate_text(&entry, content_width.max(8));
                Line::styled(truncated, log_style(&entry))
            })
            .collect()
    };

    frame.render_widget(
        Paragraph::new(Text::from(lines)).block(panel("Runtime Logs")),
        area,
    );
}

fn panel(title: &str) -> Block<'_> {
    Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(Color::Rgb(196, 167, 231))
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(52, 63, 88)))
        .style(
            Style::default()
                .bg(Color::Rgb(18, 22, 31))
                .fg(Color::Rgb(222, 226, 230)),
        )
}

fn stat_paragraph<'a>(title: &'a str, lines: Vec<Line<'a>>) -> Paragraph<'a> {
    Paragraph::new(Text::from(lines))
        .block(panel(title))
        .wrap(Wrap { trim: false })
}

fn line_value<'a>(label: &'a str, value: String) -> Line<'a> {
    Line::from(vec![
        Span::styled(label, Style::default().fg(Color::Rgb(142, 150, 170))),
        Span::raw(": "),
        Span::styled(value, Style::default().fg(Color::Rgb(242, 245, 247))),
    ])
}

fn hook_enabled_label(enabled: bool, state: Option<&crate::hooks::CircuitSnapshot>) -> String {
    if !enabled {
        return "disabled".to_string();
    }
    state
        .map(circuit_label)
        .unwrap_or_else(|| "enabled".to_string())
}

fn circuit_label(state: &crate::hooks::CircuitSnapshot) -> String {
    if state.open {
        format!("cooldown {}s", state.remaining_cooldown_secs)
    } else {
        format!("ready (fails={})", state.consecutive_failures)
    }
}

fn format_duration(total_secs: u64) -> String {
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    if hours > 0 {
        format!("{hours}h {minutes:02}m {seconds:02}s")
    } else {
        format!("{minutes:02}m {seconds:02}s")
    }
}

fn human_bytes(bytes: usize) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    if bytes as f64 >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB)
    } else if bytes as f64 >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB)
    } else {
        format!("{bytes} B")
    }
}

fn max_latency(snapshot: &crate::telemetry::MetricsSnapshot) -> u128 {
    snapshot
        .recent_requests
        .iter()
        .map(|item| item.duration_ms)
        .max()
        .unwrap_or(1)
        .max(1)
}

fn truncate_text(value: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }

    let char_count = value.chars().count();
    if char_count <= max_chars {
        return value.to_string();
    }
    if max_chars <= 3 {
        return value.chars().take(max_chars).collect();
    }

    let kept = max_chars.saturating_sub(3);
    format!("{}...", value.chars().take(kept).collect::<String>())
}

fn log_style(line: &str) -> Style {
    match line.split_whitespace().next().unwrap_or_default() {
        "ERROR" => Style::default()
            .fg(Color::Rgb(255, 123, 114))
            .add_modifier(Modifier::BOLD),
        "WARN" => Style::default()
            .fg(Color::Rgb(255, 184, 108))
            .add_modifier(Modifier::BOLD),
        "INFO" => Style::default().fg(Color::Rgb(143, 199, 255)),
        "DEBUG" => Style::default().fg(Color::Rgb(196, 167, 231)),
        _ => Style::default().fg(Color::Rgb(222, 226, 230)),
    }
}
