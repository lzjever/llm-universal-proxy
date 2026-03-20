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
    symbols,
    text::Text,
    widgets::{Axis, Block, Borders, Cell, Chart, Dataset, Gauge, Paragraph, Row, Table, Wrap},
    Terminal,
};

use crate::config::{AuthPolicy, Config};
use crate::hooks::HookSnapshot;
use crate::telemetry::RuntimeMetrics;

pub async fn run_dashboard(
    config: Arc<Config>,
    metrics: Arc<RuntimeMetrics>,
    hooks: Option<crate::hooks::HookDispatcher>,
) -> io::Result<()> {
    tokio::task::spawn_blocking(move || run_dashboard_blocking(config, metrics, hooks))
        .await
        .unwrap_or_else(|join_err| Err(io::Error::other(join_err.to_string())))
}

fn run_dashboard_blocking(
    config: Arc<Config>,
    metrics: Arc<RuntimeMetrics>,
    hooks: Option<crate::hooks::HookDispatcher>,
) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let result = dashboard_loop(&mut terminal, config, metrics, hooks);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn dashboard_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    config: Arc<Config>,
    metrics: Arc<RuntimeMetrics>,
    hooks: Option<crate::hooks::HookDispatcher>,
) -> io::Result<()> {
    loop {
        let snapshot = metrics.snapshot(&config);
        let hook_snapshot = hooks.as_ref().map(|dispatcher| dispatcher.snapshot());
        terminal.draw(|frame| draw_dashboard(frame, &config, &snapshot, hook_snapshot.as_ref()))?;

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
    config: &Config,
    snapshot: &crate::telemetry::MetricsSnapshot,
    hooks: Option<&HookSnapshot>,
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

    render_header(frame, vertical[0], snapshot);
    render_summary(frame, vertical[1], config, snapshot, hooks);
    render_config(frame, vertical[2], config, hooks);

    let lower = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(56), Constraint::Percentage(44)])
        .split(vertical[3]);
    render_upstreams(frame, lower[0], snapshot);
    render_recent(frame, lower[1], snapshot);
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
        snapshot.success_responses as f64 / snapshot.total_requests as f64
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
            format!("{}  [{format}]  {mode}", upstream.name)
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
        ],
    )
    .header(
        Row::new(vec!["Upstream", "Total", "Active", "Streams", "OK", "Err"]).style(
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
) {
    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(10), Constraint::Min(8)])
        .split(area);

    let history: Vec<(f64, f64)> = snapshot
        .recent_requests
        .iter()
        .enumerate()
        .rev()
        .map(|(index, item)| (index as f64, item.duration_ms as f64))
        .collect();
    let chart = Chart::new(vec![Dataset::default()
        .name("Latency")
        .marker(symbols::Marker::Braille)
        .style(Style::default().fg(Color::Rgb(106, 227, 199)))
        .data(&history)])
    .block(panel("Recent Latency ms"))
    .x_axis(
        Axis::default()
            .bounds([0.0, 12.0])
            .labels([Line::from("old"), Line::from("new")]),
    )
    .y_axis(
        Axis::default()
            .bounds([0.0, max_latency(snapshot) as f64])
            .labels([
                Line::from("0"),
                Line::from(max_latency(snapshot).to_string()),
            ]),
    );
    frame.render_widget(chart, inner[0]);

    let rows = snapshot.recent_requests.iter().map(|req| {
        let status_style = if req.status < 400 {
            Style::default().fg(Color::Rgb(113, 221, 130))
        } else {
            Style::default().fg(Color::Rgb(255, 123, 114))
        };
        Row::new(vec![
            Cell::from(req.path.clone()),
            Cell::from(req.client_model.clone()),
            Cell::from(req.upstream_name.clone().unwrap_or_else(|| "-".to_string())),
            Cell::from(if req.stream { "yes" } else { "no" }),
            Cell::from(format!("{}", req.status)).style(status_style),
            Cell::from(format!("{} ms", req.duration_ms)),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Percentage(22),
            Constraint::Percentage(24),
            Constraint::Percentage(20),
            Constraint::Percentage(10),
            Constraint::Percentage(10),
            Constraint::Percentage(14),
        ],
    )
    .header(
        Row::new(vec![
            "Path",
            "Client Model",
            "Upstream",
            "SSE",
            "Status",
            "Latency",
        ])
        .style(
            Style::default()
                .fg(Color::Rgb(248, 208, 111))
                .add_modifier(Modifier::BOLD),
        ),
    )
    .block(panel("Recent Requests"))
    .column_spacing(1);
    frame.render_widget(table, inner[1]);
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
