use anyhow::{Context as _, Result, anyhow};
use assistant_tool::{ActionLog, Tool, ToolResult};
use gpui::{AnyWindowHandle, App, AsyncApp, Entity, Task};
use language::{self, Anchor, Buffer, BufferSnapshot, Location, Point, ToPoint, ToPointUtf16};
use language_model::{LanguageModel, LanguageModelRequest, LanguageModelToolSchemaFormat};
use project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{fmt::Write, ops::Range, sync::Arc};
use ui::IconName;
use util::markdown::MarkdownInlineCode;

use crate::schema::json_schema_for;

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SymbolInfoToolInput {
    pub path: String,

    pub command: Info,

    pub context_before_symbol: String,

    pub symbol: String,

    pub context_after_symbol: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Info {
    Definition,

    Declaration,

    Implementation,

    TypeDefinition,

    References,
}

pub struct SymbolInfoTool;

impl Tool for SymbolInfoTool {
    fn name(&self) -> String {
        "symbol-info".into()
    }

    fn needs_confirmation(&self, _input: &serde_json::Value, _cx: &App) -> bool {
        false
    }

    fn description(&self) -> String {
        include_str!("./symbol_info_tool/description.md").into()
    }

    fn icon(&self) -> IconName {
        IconName::Eye
    }

    fn input_schema(&self, format: LanguageModelToolSchemaFormat) -> Result<serde_json::Value> {
        json_schema_for::<SymbolInfoToolInput>(format)
    }

    fn ui_text(&self, input: &serde_json::Value) -> String {
        match serde_json::from_value::<SymbolInfoToolInput>(input.clone()) {
            Ok(input) => {
                let symbol = MarkdownInlineCode(&input.symbol);

                match input.command {
                    Info::Definition => {
                        format!("Find definition for {symbol}")
                    }
                    Info::Declaration => {
                        format!("Find declaration for {symbol}")
                    }
                    Info::Implementation => {
                        format!("Find implementation for {symbol}")
                    }
                    Info::TypeDefinition => {
                        format!("Find type definition for {symbol}")
                    }
                    Info::References => {
                        format!("Find references for {symbol}")
                    }
                }
            }
            Err(_) => "Get symbol info".to_string(),
        }
    }

    fn run(
        self: Arc<Self>,
        input: serde_json::Value,
        _request: Arc<LanguageModelRequest>,
        project: Entity<Project>,
        action_log: Entity<ActionLog>,
        _model: Arc<dyn LanguageModel>,
        _window: Option<AnyWindowHandle>,
        cx: &mut App,
    ) -> ToolResult {
        let input = match serde_json::from_value::<SymbolInfoToolInput>(input) {
            Ok(input) => input,
            Err(err) => return Task::ready(Err(anyhow!(err))).into(),
        };

        cx.spawn(async move |cx| -> Result<assistant_tool::ToolResultOutput> {
            let buffer = {
                let project_path = project.read_with(cx, |project, cx| {
                    project
                        .find_project_path(&input.path, cx)
                        .context("Path not found in project")
                })??;

                project.update(cx, |project, cx| project.open_buffer(project_path, cx))?.await?
            };

            action_log.update(cx, |action_log, cx| {
                action_log.buffer_read(buffer.clone(), cx);
            })?;

            let position = {
                let Some(range) = buffer.read_with(cx, |buffer, _cx| {
                    find_symbol_range(&buffer, &input.context_before_symbol, &input.symbol, &input.context_after_symbol)
                })? else {
                    return Err(anyhow!(
                        "Failed to locate the text specified by context_before_symbol, symbol, and context_after_symbol. Make sure context_before_symbol and context_after_symbol each match exactly once in the file."
                    ));
                };

                buffer.read_with(cx, |buffer, _| {
                    range.start.to_point_utf16(&buffer.snapshot())
                })?
            };

            let output: String = match input.command {
                Info::Definition => {
                    render_locations(project
                        .update(cx, |project, cx| {
                            project.definition(&buffer, position, cx)
                        })?
                        .await?.into_iter().map(|link| link.target),
                        cx)
                }
                Info::Declaration => {
                    render_locations(project
                        .update(cx, |project, cx| {
                            project.declaration(&buffer, position, cx)
                        })?
                        .await?.into_iter().map(|link| link.target),
                        cx)
                }
                Info::Implementation => {
                    render_locations(project
                        .update(cx, |project, cx| {
                            project.implementation(&buffer, position, cx)
                        })?
                        .await?.into_iter().map(|link| link.target),
                        cx)
                }
                Info::TypeDefinition => {
                    render_locations(project
                        .update(cx, |project, cx| {
                            project.type_definition(&buffer, position, cx)
                        })?
                        .await?.into_iter().map(|link| link.target),
                        cx)
                }
                Info::References => {
                    render_locations(project
                        .update(cx, |project, cx| {
                            project.references(&buffer, position, cx)
                        })?
                        .await?,
                        cx)
                }
            };

            if output.is_empty() {
                Err(anyhow!("None found."))
            } else {
                Ok(output.into())
            }
        })
        .into()
    }
}

fn find_symbol_range(
    buffer: &Buffer,
    context_before_symbol: &str,
    symbol: &str,
    context_after_symbol: &str,
) -> Option<Range<Anchor>> {
    let snapshot = buffer.snapshot();
    let text = snapshot.text();
    let search_string = format!("{context_before_symbol}{symbol}{context_after_symbol}");
    let mut positions = text.match_indices(&search_string);
    let position = positions.next()?.0;

    // The combined string must appear exactly once.
    if positions.next().is_some() {
        return None;
    }

    let symbol_start = position + context_before_symbol.len();
    let symbol_end = symbol_start + symbol.len();
    let symbol_start_anchor = snapshot.anchor_before(snapshot.offset_to_point(symbol_start));
    let symbol_end_anchor = snapshot.anchor_before(snapshot.offset_to_point(symbol_end));

    Some(symbol_start_anchor..symbol_end_anchor)
}

fn render_locations(locations: impl IntoIterator<Item = Location>, cx: &mut AsyncApp) -> String {
    let mut answer = String::new();

    for location in locations {
        location
            .buffer
            .read_with(cx, |buffer, _cx| {
                if let Some(target_path) = buffer
                    .file()
                    .and_then(|file| file.path().as_os_str().to_str())
                {
                    let snapshot = buffer.snapshot();
                    let start = location.range.start.to_point(&snapshot);
                    let end = location.range.end.to_point(&snapshot);
                    let start_line = start.row + 1;
                    let start_col = start.column + 1;
                    let end_line = end.row + 1;
                    let end_col = end.column + 1;

                    if start_line == end_line {
                        writeln!(answer, "{target_path}:{start_line},{start_col}")
                    } else {
                        writeln!(
                            answer,
                            "{target_path}:{start_line},{start_col}-{end_line},{end_col}",
                        )
                    }
                    .ok();

                    write_code_excerpt(&mut answer, &snapshot, &location.range);
                }
            })
            .ok();
    }

    // Trim trailing newlines without reallocating.
    answer.truncate(answer.trim_end().len());

    answer
}

fn write_code_excerpt(buf: &mut String, snapshot: &BufferSnapshot, range: &Range<Anchor>) {
    const MAX_LINE_LEN: u32 = 200;

    let start = range.start.to_point(snapshot);
    let end = range.end.to_point(snapshot);

    for row in start.row..=end.row {
        let row_start = Point::new(row, 0);
        let row_end = if row < snapshot.max_point().row {
            Point::new(row + 1, 0)
        } else {
            Point::new(row, u32::MAX)
        };

        buf.extend(
            snapshot
                .text_for_range(row_start..row_end)
                .take(MAX_LINE_LEN as usize),
        );

        if row_end.column > MAX_LINE_LEN {
            buf.push_str("…\n");
        }

        buf.push('\n');
    }
}
