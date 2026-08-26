use fixtrace_protocol::{ApprovalChoice, ItemStatus, TimelineItem};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Tabs, Wrap},
};
use unicode_width::UnicodeWidthStr;

use crate::{InspectorTab, Modal, Model, commands};

pub fn render(frame: &mut Frame<'_>, model: &Model) {
    let area = frame.area();
    if area.width < 60 || area.height < 16 {
        frame.render_widget(
            Paragraph::new(format!(
                "FixTrace needs at least 60×16\nCurrent terminal: {}×{}\nResize to continue safely.",
                area.width, area.height
            ))
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL).title(" FixTrace ")),
            area,
        );
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(6),
            Constraint::Length(5),
            Constraint::Length(1),
        ])
        .split(area);
    render_header(frame, rows[0], model);
    render_body(frame, rows[1], model);
    frame.render_widget(&model.composer, rows[2]);
    render_status(frame, rows[3], model);

    if let Some(modal) = &model.modal {
        render_modal(frame, area, model, modal);
    } else if model.composer_text().trim_start().starts_with('/') {
        render_palette(frame, area, model, false);
    } else if let Some(query) = model.reference_query() {
        render_references(frame, area, model, query);
    }
}

fn render_header(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let session = model
        .session
        .as_ref()
        .map(|session| session.summary.project_name.as_str())
        .unwrap_or("No session");
    let task = model
        .active_task()
        .map(|task| format!("{:?}", task.status))
        .unwrap_or_else(|| "Idle".to_owned());
    let usage = model.session.as_ref().map(|session| &session.usage);
    let tokens = usage.map_or(0, |usage| usage.total_tokens);
    let cost = usage.map_or(0.0, |usage| usage.total_cost_usd);
    let ratio = usage.map_or(0.0, |usage| usage.budget_ratio);
    let offline = if model.offline { "OFFLINE" } else { "online" };
    let line = Line::from(vec![
        Span::styled(
            " FixTrace ",
            Style::default()
                .fg(Color::Black)
                .bg(model.theme.accent())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(" {session}  ")),
        Span::styled(
            &model.initialized.config_summary.model,
            Style::default().fg(model.theme.accent()),
        ),
        Span::raw(format!(
            " · {} · {:?} · task {task} · {tokens} tok · ${cost:.4} · {:.0}% · {} · {offline}",
            model.initialized.config_summary.reasoning_mode,
            model.initialized.config_summary.approval_policy,
            ratio * 100.0,
            model.connection.label(),
        )),
    ]);
    frame.render_widget(
        Paragraph::new(line)
            .block(Block::default().borders(Borders::ALL))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_body(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    if area.width >= 120 {
        let mut constraints = Vec::new();
        if model.show_sidebar {
            constraints.push(Constraint::Length(24));
        }
        constraints.push(Constraint::Min(44));
        if model.show_inspector {
            constraints.push(Constraint::Length(38));
        }
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(constraints)
            .split(area);
        let mut index = 0;
        if model.show_sidebar {
            render_sessions(frame, columns[index], model);
            index += 1;
        }
        render_transcript(frame, columns[index], model);
        index += 1;
        if model.show_inspector {
            render_inspector(frame, columns[index], model);
        }
    } else if area.width >= 80 {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
            .split(area);
        render_transcript(frame, columns[0], model);
        render_inspector(frame, columns[1], model);
    } else if model.inspector_tab == InspectorTab::Overview {
        render_transcript(frame, area, model);
    } else {
        render_inspector(frame, area, model);
    }
}

fn render_sessions(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let items: Vec<ListItem<'static>> = model
        .sessions
        .iter()
        .map(|session| {
            let selected = model.selected_session_id == Some(session.id);
            let marker = if selected { "▶" } else { " " };
            let task = if session.active_task_id.is_some() {
                " • running"
            } else if session.archived {
                " • archived"
            } else {
                ""
            };
            ListItem::new(vec![
                Line::from(vec![
                    Span::styled(
                        format!("{marker} {}", session.project_name),
                        if selected {
                            Style::default()
                                .fg(model.theme.accent())
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default()
                        },
                    ),
                    Span::styled(task, Style::default().fg(model.theme.warning())),
                ]),
                Line::styled(
                    format!(
                        "  {:?} · {}",
                        session.status,
                        session.updated_at.format("%m-%d %H:%M")
                    ),
                    Style::default().fg(Color::DarkGray),
                ),
            ])
        })
        .collect();
    frame.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title(" Sessions ")),
        area,
    );
}

fn render_transcript(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let inner_width = usize::from(area.width.saturating_sub(4)).max(8);
    let mut lines = Vec::new();
    let mut title = " Transcript ".to_owned();
    if let Some(session) = &model.session {
        let total = session.timeline.len();
        let window_size = usize::from(area.height).saturating_mul(3).max(64);
        let start = if model.follow_tail {
            total.saturating_sub(window_size)
        } else {
            usize::from(model.scroll).min(total.saturating_sub(1))
        };
        let end = start.saturating_add(window_size).min(total);
        if total > window_size {
            title = format!(
                " Transcript · {}–{} / {} ",
                start.saturating_add(1),
                end,
                total
            );
        }
        for item in &session.timeline[start..end] {
            timeline_lines(&mut lines, item, inner_width, model);
            lines.push(Line::raw(""));
        }
        if lines.is_empty() {
            lines.push(Line::styled(
                "No timeline items yet. Send a message or run /analyze.",
                Style::default().fg(Color::DarkGray),
            ));
        }
    } else {
        lines.push(Line::styled(
            "Open a Session with Ctrl+O or /open.",
            Style::default().fg(Color::DarkGray),
        ));
    }
    let viewport = usize::from(area.height.saturating_sub(2));
    let max_scroll = lines.len().saturating_sub(viewport);
    let scroll = if model.follow_tail {
        u16::try_from(max_scroll).unwrap_or(u16::MAX)
    } else {
        0
    };
    frame.render_widget(
        Paragraph::new(lines)
            .scroll((scroll, 0))
            .block(Block::default().borders(Borders::ALL).title(title))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn timeline_lines(
    lines: &mut Vec<Line<'static>>,
    item: &TimelineItem,
    width: usize,
    model: &Model,
) {
    match item {
        TimelineItem::UserMessage(item) => {
            lines.push(label(" YOU ", model.theme.accent(), item.header.status));
            push_wrapped(lines, &item.text, width, Style::default());
        }
        TimelineItem::AgentMessage(item) => {
            lines.push(label(" AGENT ", model.theme.success(), item.header.status));
            push_wrapped(lines, &item.text, width, Style::default());
        }
        TimelineItem::ToolCall(item) => {
            let expanded = model.expanded_items.contains(&item.header.id);
            lines.push(Line::from(vec![
                Span::styled(
                    if expanded { "▼ TOOL " } else { "▶ TOOL " },
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(item.name.clone(), Style::default().fg(Color::Magenta)),
                Span::raw(format!(
                    " {}",
                    truncate(&item.arguments_summary, width.saturating_sub(18))
                )),
            ]));
            if expanded {
                push_wrapped(
                    lines,
                    &item.arguments_summary,
                    width,
                    Style::default().fg(Color::Gray),
                );
                if let Some(result) = &item.result_summary {
                    push_wrapped(
                        lines,
                        result,
                        width,
                        Style::default().fg(model.theme.success()),
                    );
                }
            }
        }
        TimelineItem::CommandExecution(item) => {
            lines.push(label(
                " COMMAND ",
                model.theme.warning(),
                item.header.status,
            ));
            lines.push(Line::raw(format!("$ {}", item.command)));
            lines.push(Line::styled(
                format!(
                    "cwd {} · exit {:?} · {:?}ms",
                    item.cwd, item.exit_code, item.duration_ms
                ),
                Style::default().fg(Color::DarkGray),
            ));
            push_wrapped(
                lines,
                &item.stdout_preview,
                width,
                Style::default().fg(Color::Gray),
            );
            push_wrapped(
                lines,
                &item.stderr_preview,
                width,
                Style::default().fg(model.theme.danger()),
            );
        }
        TimelineItem::FilePatch(item) => {
            lines.push(label(" PATCH ", Color::Blue, item.header.status));
            lines.push(Line::raw(item.summary.clone()));
            for file in &item.files {
                lines.push(Line::raw(format!(
                    "  {} {} (+{:?}/-{:?})",
                    file.change_kind, file.path, file.additions, file.deletions
                )));
            }
        }
        TimelineItem::Trial(item) => {
            let color = match item.classification {
                fixtrace_protocol::TrialClassification::StablePass => model.theme.success(),
                fixtrace_protocol::TrialClassification::StableFail => model.theme.danger(),
                fixtrace_protocol::TrialClassification::Flaky => model.theme.warning(),
                _ => Color::Gray,
            };
            lines.push(label(" TRIAL ", color, item.header.status));
            lines.push(Line::raw(format!(
                "{} · actions {:?}",
                item.summary, item.action_ids
            )));
        }
        TimelineItem::Minimization(item) => {
            lines.push(label(" MINIMIZE ", Color::Blue, item.header.status));
            lines.push(Line::raw(format!(
                "{} · {} → {} actions",
                item.summary,
                item.before_action_ids.len(),
                item.candidate_action_ids.len()
            )));
        }
        TimelineItem::Diagnosis(item) => {
            lines.push(label(
                " DIAGNOSIS ",
                model.theme.success(),
                item.header.status,
            ));
            push_wrapped(
                lines,
                &item.statement,
                width,
                Style::default().add_modifier(Modifier::BOLD),
            );
            lines.push(Line::raw(format!(
                "minimal {:?} · confidence {}",
                item.minimal_action_ids, item.confidence
            )));
        }
        TimelineItem::RecordedAction(item) => {
            lines.push(label(" ACTION ", Color::Blue, item.header.status));
            lines.push(Line::raw(format!(
                "[{}] {} · {}",
                item.action_id, item.kind, item.summary
            )));
        }
        TimelineItem::PlanSummary(item) => {
            lines.push(label(" PLAN ", Color::Blue, item.header.status));
            for step in &item.steps {
                lines.push(Line::raw(format!("  {:?} {}", step.status, step.text)));
            }
        }
        TimelineItem::Approval(item) => {
            lines.push(label(
                " APPROVAL ",
                model.theme.warning(),
                item.header.status,
            ));
            lines.push(Line::raw(item.approval.request.title.clone()));
        }
        TimelineItem::Usage(item) => {
            lines.push(label(" USAGE ", Color::Blue, item.header.status));
            lines.push(Line::raw(format!(
                "{} tokens · ${:.4}",
                item.usage.total_tokens, item.usage.total_cost_usd
            )));
        }
        TimelineItem::Notice(item) => {
            lines.push(label(" NOTICE ", Color::Gray, item.header.status));
            push_wrapped(lines, &item.notice.message, width, Style::default());
        }
        TimelineItem::Error(item) => {
            lines.push(label(" ERROR ", model.theme.danger(), item.header.status));
            push_wrapped(
                lines,
                &item.error.message,
                width,
                Style::default().fg(model.theme.danger()),
            );
        }
    }
}

fn label(text: &'static str, color: Color, status: ItemStatus) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            text,
            Style::default()
                .fg(Color::Black)
                .bg(color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {:?}", status),
            Style::default().fg(Color::DarkGray),
        ),
    ])
}

fn render_inspector(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(area);
    let titles = InspectorTab::ALL
        .iter()
        .map(|tab| Line::from(tab.label()))
        .collect::<Vec<_>>();
    let selected = InspectorTab::ALL
        .iter()
        .position(|tab| *tab == model.inspector_tab)
        .unwrap_or(0);
    frame.render_widget(
        Tabs::new(titles)
            .select(selected)
            .highlight_style(
                Style::default()
                    .fg(model.theme.accent())
                    .add_modifier(Modifier::BOLD),
            )
            .divider("·")
            .block(Block::default().borders(Borders::ALL).title(" Inspector ")),
        rows[0],
    );
    let lines = inspector_lines(model);
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        rows[1],
    );
}

fn inspector_lines(model: &Model) -> Vec<Line<'static>> {
    let Some(session) = &model.session else {
        return vec![Line::styled(
            "No session selected",
            Style::default().fg(Color::DarkGray),
        )];
    };
    match model.inspector_tab {
        InspectorTab::Overview => {
            let mut lines = vec![
                Line::raw(format!("Project     {}", session.summary.project_name)),
                Line::raw(format!("Status      {:?}", session.summary.status)),
                Line::raw(format!("Actions     {}", session.actions.len())),
                Line::raw(format!("Trials      {}", session.trials.len())),
                Line::raw(format!(
                    "Task        {}",
                    session
                        .task
                        .as_ref()
                        .map_or("idle".to_owned(), |task| format!(
                            "{} · {:?}",
                            task.title, task.status
                        ))
                )),
                Line::raw(""),
            ];
            if let Some(diagnosis) = &session.diagnosis {
                lines.push(Line::styled(
                    "Diagnosis",
                    Style::default()
                        .fg(model.theme.success())
                        .add_modifier(Modifier::BOLD),
                ));
                lines.push(Line::raw(diagnosis.statement.clone()));
                lines.push(Line::raw(format!(
                    "Minimal: {:?}",
                    diagnosis.minimal_action_ids
                )));
                lines.push(Line::raw(format!("Confidence: {}", diagnosis.confidence)));
            } else {
                lines.push(Line::styled(
                    "No diagnosis yet",
                    Style::default().fg(Color::DarkGray),
                ));
            }
            lines
        }
        InspectorTab::Actions => session
            .actions
            .iter()
            .flat_map(|action| {
                vec![
                    Line::styled(
                        format!("[{}] {}", action.id, action.summary),
                        Style::default().fg(if action.replayable {
                            model.theme.success()
                        } else {
                            model.theme.warning()
                        }),
                    ),
                    Line::styled(
                        format!("  {} · cwd {}", action.kind, action.cwd),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]
            })
            .collect(),
        InspectorTab::Trials => session
            .trials
            .iter()
            .flat_map(|trial| {
                vec![
                    Line::styled(
                        format!("{:?} {}", trial.classification, trial.id),
                        Style::default().fg(match trial.classification {
                            fixtrace_protocol::TrialClassification::StablePass => {
                                model.theme.success()
                            }
                            fixtrace_protocol::TrialClassification::StableFail => {
                                model.theme.danger()
                            }
                            _ => model.theme.warning(),
                        }),
                    ),
                    Line::raw(format!(
                        "  actions {:?} · {} attempts",
                        trial.action_ids,
                        trial.attempts.len()
                    )),
                ]
            })
            .collect(),
        InspectorTab::Graph => {
            let mut lines = vec![Line::styled(
                "Resource dependency / experimental attribution",
                Style::default().fg(Color::DarkGray),
            )];
            for node in &session.dependency_graph.nodes {
                lines.push(Line::styled(
                    format!("[{}] {}", node.action_id, node.label),
                    Style::default().fg(if node.in_minimal_set {
                        model.theme.success()
                    } else {
                        Color::White
                    }),
                ));
                for edge in session
                    .dependency_graph
                    .edges
                    .iter()
                    .filter(|edge| edge.from_action_id == node.action_id)
                {
                    lines.push(Line::raw(format!(
                        "  └─▶ [{}] {}",
                        edge.to_action_id, edge.reason
                    )));
                }
            }
            lines
        }
        InspectorTab::Diff => {
            let mut lines = Vec::new();
            for file in &session.diff.files {
                lines.push(Line::styled(
                    format!("{} {}", file.change_kind, file.path),
                    Style::default().fg(Color::Blue),
                ));
                if let Some(diff) = &file.unified_diff {
                    lines.extend(diff.lines().map(|line| {
                        Line::styled(
                            line.to_owned(),
                            Style::default().fg(if line.starts_with('+') {
                                model.theme.success()
                            } else if line.starts_with('-') {
                                model.theme.danger()
                            } else {
                                Color::Gray
                            }),
                        )
                    }));
                }
            }
            if lines.is_empty() {
                vec![Line::styled(
                    "No diff available",
                    Style::default().fg(Color::DarkGray),
                )]
            } else {
                lines
            }
        }
        InspectorTab::Artifacts => {
            let artifacts: Vec<_> = session
                .timeline
                .iter()
                .flat_map(timeline_artifacts)
                .collect();
            if artifacts.is_empty() {
                vec![Line::styled(
                    "No artifacts",
                    Style::default().fg(Color::DarkGray),
                )]
            } else {
                artifacts
                    .into_iter()
                    .map(|artifact| {
                        Line::raw(format!("{} · {} bytes", artifact.name, artifact.size))
                    })
                    .collect()
            }
        }
        InspectorTab::Usage => vec![
            Line::raw(format!("Input       {}", session.usage.input_tokens)),
            Line::raw(format!("Output      {}", session.usage.output_tokens)),
            Line::raw(format!("Total       {}", session.usage.total_tokens)),
            Line::raw(format!("Cost        ${:.6}", session.usage.total_cost_usd)),
            Line::raw(format!("Token limit {}", session.usage.token_limit)),
            Line::raw(format!("Cost limit  ${:.4}", session.usage.cost_limit_usd)),
            Line::raw(format!(
                "Budget      {:.1}%",
                session.usage.budget_ratio * 100.0
            )),
        ],
        InspectorTab::Settings => vec![
            Line::raw(format!("Theme       {}", model.theme.label())),
            Line::raw(format!(
                "Provider    {}",
                model.initialized.config_summary.provider
            )),
            Line::raw(format!(
                "Endpoint    {}",
                model.initialized.config_summary.endpoint
            )),
            Line::raw(format!(
                "Model       {}",
                model.initialized.config_summary.model
            )),
            Line::raw(format!(
                "API style   {}",
                model.initialized.config_summary.api_style
            )),
            Line::raw(format!(
                "Context     {}",
                model.initialized.config_summary.context_length
            )),
            Line::raw(format!(
                "Effort      {}",
                model.initialized.config_summary.reasoning_mode
            )),
            Line::raw(format!(
                "Approval    {:?}",
                model.initialized.config_summary.approval_policy
            )),
            Line::raw(format!(
                "Credential  {}",
                if model.initialized.config_summary.has_api_key {
                    "available"
                } else {
                    "not set"
                }
            )),
        ],
    }
}

fn timeline_artifacts(item: &TimelineItem) -> &[fixtrace_protocol::ArtifactSummary] {
    match item {
        TimelineItem::UserMessage(item) => &item.header.artifacts,
        TimelineItem::AgentMessage(item) => &item.header.artifacts,
        TimelineItem::PlanSummary(item) => &item.header.artifacts,
        TimelineItem::ToolCall(item) => &item.header.artifacts,
        TimelineItem::CommandExecution(item) => &item.header.artifacts,
        TimelineItem::FilePatch(item) => &item.header.artifacts,
        TimelineItem::RecordedAction(item) => &item.header.artifacts,
        TimelineItem::Trial(item) => &item.header.artifacts,
        TimelineItem::Minimization(item) => &item.header.artifacts,
        TimelineItem::Diagnosis(item) => &item.header.artifacts,
        TimelineItem::Approval(item) => &item.header.artifacts,
        TimelineItem::Usage(item) => &item.header.artifacts,
        TimelineItem::Notice(item) => &item.header.artifacts,
        TimelineItem::Error(item) => &item.header.artifacts,
    }
}

fn render_status(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let style = if model.offline {
        Style::default().fg(model.theme.danger())
    } else {
        Style::default().fg(Color::DarkGray)
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!(
                    " {} ",
                    if model.offline {
                        "OFFLINE"
                    } else {
                        "CONNECTED"
                    }
                ),
                style.add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                "{}  ·  Ctrl+P commands  Tab inspector  Ctrl+C cancel/exit  ? help",
                model.status
            )),
        ])),
        area,
    );
}

fn render_modal(frame: &mut Frame<'_>, area: Rect, model: &Model, modal: &Modal) {
    match modal {
        Modal::CommandPalette => render_palette(frame, area, model, true),
        Modal::SessionPicker => {
            let popup = centered(area, 70, 70);
            frame.render_widget(Clear, popup);
            render_sessions(frame, popup, model);
        }
        Modal::Help => {
            let popup = centered(area, 76, 78);
            frame.render_widget(Clear, popup);
            let text = "Keys\n  Enter send/steer · Alt+Enter/Ctrl+J newline · Ctrl+X $EDITOR\n  Ctrl+P palette · Ctrl+N new · Ctrl+O sessions · Ctrl+B sidebar\n  Ctrl+I inspector · Tab panels/reference completion · x expand Tool\n  PageUp/PageDown scroll · g/G top/bottom · bracketed paste supported\n  Ctrl+C cancels active task; press again within 1.5s to exit\n\nWorkflow\n  /new /fork /archive /record /verify /replay /analyze /diagnose\n  /repeat /cancel /demo /actions /trials /graph /diff /artifacts\n  /usage /model /effort /permissions /config /export /import /theme\n\nType @ to reference an Action, Trial, or Artifact. Esc/Enter closes.";
            frame.render_widget(
                Paragraph::new(text)
                    .block(Block::default().borders(Borders::ALL).title(" Help "))
                    .wrap(Wrap { trim: false }),
                popup,
            );
        }
        Modal::Approval(id) => render_approval(frame, area, model, *id),
        Modal::Settings => {
            let popup = centered(area, 72, 70);
            frame.render_widget(Clear, popup);
            frame.render_widget(
                Paragraph::new(inspector_lines(model))
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(" Settings · Esc close "),
                    )
                    .wrap(Wrap { trim: false }),
                popup,
            );
        }
        Modal::Error(error) => {
            let popup = centered(area, 70, 35);
            frame.render_widget(Clear, popup);
            frame.render_widget(
                Paragraph::new(error.clone())
                    .style(Style::default().fg(model.theme.danger()))
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_style(Style::default().fg(model.theme.danger()))
                            .title(" Error · Esc close "),
                    )
                    .wrap(Wrap { trim: false }),
                popup,
            );
        }
        Modal::Export => {}
    }
}

fn render_palette(frame: &mut Frame<'_>, area: Rect, model: &Model, modal: bool) {
    let query = model.composer_text();
    let query = query.trim().trim_start_matches('/');
    let matches = commands::matching(query);
    let height = u16::try_from(matches.len().min(10) + 2).unwrap_or(12);
    let popup = if modal {
        centered_fixed(area, 76, height.max(8))
    } else {
        Rect {
            x: area.x.saturating_add(2),
            y: area.bottom().saturating_sub(height + 6),
            width: area.width.saturating_sub(4).min(76),
            height,
        }
    };
    frame.render_widget(Clear, popup);
    let items = matches
        .into_iter()
        .take(10)
        .map(|command| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{:<14}", command.name),
                    Style::default()
                        .fg(model.theme.accent())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(command.description),
            ]))
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Commands · type to filter · Enter run "),
        ),
        popup,
    );
}

fn render_references(frame: &mut Frame<'_>, area: Rect, model: &Model, query: &str) {
    let suggestions = model.reference_suggestions(query);
    let height = u16::try_from(suggestions.len().clamp(1, 10) + 2).unwrap_or(12);
    let popup = Rect {
        x: area.x.saturating_add(2),
        y: area.bottom().saturating_sub(height + 6),
        width: area.width.saturating_sub(4).min(88),
        height,
    };
    frame.render_widget(Clear, popup);
    let items = if suggestions.is_empty() {
        vec![ListItem::new("No matching Action, Trial, or Artifact")]
    } else {
        suggestions
            .into_iter()
            .map(|(token, label)| {
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{token:<48}"),
                        Style::default()
                            .fg(model.theme.accent())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(label),
                ]))
            })
            .collect()
    };
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" References · Tab complete "),
        ),
        popup,
    );
}

fn render_approval(frame: &mut Frame<'_>, area: Rect, model: &Model, id: uuid::Uuid) {
    let popup = centered(area, 76, 72);
    frame.render_widget(Clear, popup);
    let approval = model.session.as_ref().and_then(|session| {
        session
            .approvals
            .iter()
            .find(|approval| approval.request.id == id)
    });
    let lines = approval.map_or_else(
        || vec![Line::raw("Approval details are being refreshed…")],
        |approval| {
            let request = &approval.request;
            let shortcuts = [
                (ApprovalChoice::ApproveOnce, "y once"),
                (ApprovalChoice::ApproveForTask, "t task"),
                (
                    ApprovalChoice::ApproveEquivalentForSession,
                    "s equivalent/session",
                ),
                (ApprovalChoice::Deny, "n deny"),
                (ApprovalChoice::CancelTask, "c cancel task"),
            ]
            .into_iter()
            .filter_map(|(choice, label)| request.choices.contains(&choice).then_some(label))
            .collect::<Vec<_>>()
            .join(" · ");
            vec![
                Line::styled(
                    request.title.clone(),
                    Style::default()
                        .fg(model.theme.warning())
                        .add_modifier(Modifier::BOLD),
                ),
                Line::raw(format!("Why       {}", request.reason)),
                Line::raw(format!("Risk      {:?}", request.risk)),
                Line::raw(format!("Scope     {:?}", request.requested_scope)),
                Line::raw(format!(
                    "Command   {}",
                    request.command_preview.as_deref().unwrap_or("n/a")
                )),
                Line::raw(format!(
                    "Cwd       {}",
                    request
                        .cwd
                        .as_ref()
                        .map_or("n/a".to_owned(), |path| path.display().to_string())
                )),
                Line::raw(format!(
                    "Sandbox   {}",
                    request
                        .sandbox_path
                        .as_ref()
                        .map_or("n/a".to_owned(), |path| path.display().to_string())
                )),
                Line::raw(format!("Actions   {:?}", request.action_ids)),
                Line::raw(format!("Paths     {:?}", request.affected_paths)),
                Line::raw(format!("Network   {}", request.accesses_network)),
                Line::raw(""),
                Line::styled(shortcuts, Style::default().fg(model.theme.accent())),
            ]
        },
    );
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(model.theme.warning()))
                    .title(" Approval required "),
            )
            .wrap(Wrap { trim: false }),
        popup,
    );
}

fn push_wrapped(lines: &mut Vec<Line<'static>>, text: &str, width: usize, style: Style) {
    for line in text.lines() {
        if line.is_empty() {
            lines.push(Line::raw(""));
        } else {
            lines.extend(
                textwrap::wrap(line, width.max(8))
                    .into_iter()
                    .map(|part| Line::styled(part.into_owned(), style)),
            );
        }
    }
}

fn truncate(text: &str, width: usize) -> String {
    if UnicodeWidthStr::width(text) <= width {
        return text.to_owned();
    }
    let mut output = String::new();
    for character in text.chars() {
        output.push(character);
        if UnicodeWidthStr::width(output.as_str()) >= width.saturating_sub(1) {
            break;
        }
    }
    output.push('…');
    output
}

fn centered(area: Rect, width_percent: u16, height_percent: u16) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - height_percent) / 2),
            Constraint::Percentage(height_percent),
            Constraint::Percentage((100 - height_percent) / 2),
        ])
        .split(area);
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - width_percent) / 2),
            Constraint::Percentage(width_percent),
            Constraint::Percentage((100 - width_percent) / 2),
        ])
        .split(vertical[1]);
    horizontal[1]
}

fn centered_fixed(area: Rect, width: u16, height: u16) -> Rect {
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width: width.min(area.width),
        height: height.min(area.height),
    }
}
