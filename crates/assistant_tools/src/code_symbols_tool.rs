use std::fmt::Write;
use std::path::PathBuf;
use std::sync::Arc;

use crate::schema::json_schema_for;
use anyhow::{Result, anyhow};
use assistant_tool::outline;
use assistant_tool::{ActionLog, Tool, ToolResult};
use collections::IndexMap;
use gpui::{AnyWindowHandle, App, AsyncApp, Entity, Task};
use language_model::{LanguageModelRequestMessage, LanguageModelToolSchemaFormat};
use project::{Project, Symbol};
use regex::{Regex, RegexBuilder};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ui::IconName;
use util::markdown::MarkdownInlineCode;

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct CodeSymbolsInput {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub regex: Option<String>,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub offset: u32,
}

impl CodeSymbolsInput {
    pub fn page(&self) -> u32 {
        1 + (self.offset / RESULTS_PER_PAGE)
    }
}

const RESULTS_PER_PAGE: u32 = 100;

pub struct CodeSymbolsTool;

impl Tool for CodeSymbolsTool {
    fn name(&self) -> String {
        "code_symbols".into()
    }

    fn needs_confirmation(&self, _: &serde_json::Value, _: &App) -> bool {
        false
    }

    fn description(&self) -> String {
        include_str!("./code_symbols_tool/description.md").into()
    }

    fn icon(&self) -> IconName {
        IconName::Code
    }

    fn input_schema(&self, format: LanguageModelToolSchemaFormat) -> Result<serde_json::Value> {
        json_schema_for::<CodeSymbolsInput>(format)
    }

    fn ui_text(&self, input: &serde_json::Value) -> String {
        match serde_json::from_value::<CodeSymbolsInput>(input.clone()) {
            Ok(input) => {
                let page = input.page();

                match &input.path {
                    Some(path) => {
                        let path = MarkdownInlineCode(path);
                        if page > 1 {
                            format!("List page {page} of code symbols for {path}")
                        } else {
                            format!("List code symbols for {path}")
                        }
                    }
                    None => {
                        if page > 1 {
                            format!("List page {page} of project symbols")
                        } else {
                            "List all project symbols".to_string()
                        }
                    }
                }
            }
            Err(_) => "List code symbols".to_string(),
        }
    }

    fn run(
        self: Arc<Self>,
        input: serde_json::Value,
        _messages: &[LanguageModelRequestMessage],
        project: Entity<Project>,
        action_log: Entity<ActionLog>,
        _window: Option<AnyWindowHandle>,
        cx: &mut App,
    ) -> ToolResult {
        let input = match serde_json::from_value::<CodeSymbolsInput>(input) {
            Ok(input) => input,
            Err(err) => return Task::ready(Err(anyhow!(err))).into(),
        };

        let regex = match input.regex {
            Some(regex_str) => match RegexBuilder::new(&regex_str)
                .case_insensitive(!input.case_sensitive)
                .build()
            {
                Ok(regex) => Some(regex),
                Err(err) => return Task::ready(Err(anyhow!("Invalid regex: {err}"))).into(),
            },
            None => None,
        };

        cx.spawn(async move |cx| match input.path {
            Some(path) => outline::file_outline(project, path, action_log, regex, cx).await,
            None => project_symbols(project, regex, input.offset, cx).await,
        })
        .into()
    }
}

async fn project_symbols(
    project: Entity<Project>,
    regex: Option<Regex>,
    offset: u32,
    cx: &mut AsyncApp,
) -> anyhow::Result<String> {
    let symbols = project
        .update(cx, |project, cx| project.symbols("", cx))?
        .await?;

    if symbols.is_empty() {
        return Err(anyhow!("No symbols found in project."));
    }

    let mut symbols_by_path: IndexMap<PathBuf, Vec<&Symbol>> = IndexMap::default();

    for symbol in symbols
        .iter()
        .filter(|symbol| {
            if let Some(regex) = &regex {
                regex.is_match(&symbol.name)
            } else {
                true
            }
        })
        .skip(offset as usize)
        .take((RESULTS_PER_PAGE as usize).saturating_add(1))
    {
        if let Some(worktree_path) = project.read_with(cx, |project, cx| {
            project
                .worktree_for_id(symbol.path.worktree_id, cx)
                .map(|worktree| PathBuf::from(worktree.read(cx).root_name()))
        })? {
            let path = worktree_path.join(&symbol.path.path);
            symbols_by_path.entry(path).or_default().push(symbol);
        }
    }

    if symbols_by_path.is_empty() {
        return Err(anyhow!("No symbols found matching the criteria."));
    }

    let mut symbols_rendered = 0;
    let mut has_more_symbols = false;
    let mut output = String::new();

    'outer: for (file_path, file_symbols) in symbols_by_path {
        if symbols_rendered > 0 {
            output.push('\n');
        }

        writeln!(&mut output, "{}", file_path.display()).ok();

        for symbol in file_symbols {
            if symbols_rendered >= RESULTS_PER_PAGE {
                has_more_symbols = true;
                break 'outer;
            }

            write!(&mut output, "  {} ", symbol.label.text()).ok();

            let start_line = symbol.range.start.0.row as usize + 1;
            let end_line = symbol.range.end.0.row as usize + 1;

            if start_line == end_line {
                writeln!(&mut output, "[L{}]", start_line).ok();
            } else {
                writeln!(&mut output, "[L{}-{}]", start_line, end_line).ok();
            }

            symbols_rendered += 1;
        }
    }

    Ok(if symbols_rendered == 0 {
        "No symbols found in the requested page.".to_string()
    } else if has_more_symbols {
        format!(
            "{output}\nShowing symbols {}-{} (more symbols were found; use offset: {} to see next page)",
            offset + 1,
            offset + symbols_rendered,
            offset + RESULTS_PER_PAGE,
        )
    } else {
        output
    })
}
