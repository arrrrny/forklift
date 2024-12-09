//! # REPL Output Module
//!
//! This module provides the core functionality for handling and displaying
//! various types of output from Jupyter kernels.
//!
//! ## Key Components
//!
//! - `OutputContent`: An enum that encapsulates different types of output content.
//! - `ExecutionView`: Manages the display of outputs for a single execution.
//! - `ExecutionStatus`: Represents the current status of an execution.
//!
//! ## Output Types
//!
//! The module supports several output types, including:
//! - Plain text
//! - Markdown
//! - Images (PNG and JPEG)
//! - Tables
//! - Error messages
//!
//! ## Clipboard Support
//!
//! Most output types implement the `SupportsClipboard` trait, allowing
//! users to easily copy output content to the system clipboard.
//!
//! ## Rendering
//!
//! The module provides rendering capabilities for each output type,
//! ensuring proper display within the REPL interface.
//!
//! ## Jupyter Integration
//!
//! This module is designed to work with Jupyter message protocols,
//! interpreting and displaying various types of Jupyter output.

use std::time::Duration;

use editor::{Editor, MultiBuffer};
use gpui::{
    percentage, Animation, AnimationExt, AnyElement, ClipboardItem, Model, Render, Transformation,
    View, WeakView,
};
use language::Buffer;
use runtimelib::{ExecutionState, JupyterMessageContent, MimeBundle, MimeType};
use ui::{div, prelude::*, v_flex, IntoElement, Styled, Tooltip};

mod image;
use image::ImageView;

mod markdown;
use markdown::MarkdownView;

mod table;
use table::TableView;

pub mod plain;
use plain::TerminalOutput;

pub(crate) mod user_error;
use user_error::ErrorView;
use workspace::Workspace;

/// When deciding what to render from a collection of mediatypes, we need to rank them in order of importance
fn rank_mime_type(mimetype: &MimeType) -> usize {
    match mimetype {
        MimeType::DataTable(_) => 6,
        MimeType::Png(_) => 4,
        MimeType::Jpeg(_) => 3,
        MimeType::Markdown(_) => 2,
        MimeType::Plain(_) => 1,
        // All other media types are not supported in Zed at this time
        _ => 0,
    }
}

pub(crate) trait OutputContent {
    fn clipboard_content(&self, window: &Window, cx: &AppContext) -> Option<ClipboardItem>;
    fn has_clipboard_content(&self, _window: &Window, cx: &AppContext) -> bool {
        false
    }
    fn has_buffer_content(&self, _window: &Window, cx: &AppContext) -> bool {
        false
    }
    fn buffer_content(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::AppContext,
    ) -> Option<Model<Buffer>> {
        None
    }
}

impl<V: OutputContent + 'static> OutputContent for View<V> {
    fn clipboard_content(&self, window: &Window, cx: &AppContext) -> Option<ClipboardItem> {
        self.read(cx).clipboard_content(cx)
    }

    fn has_clipboard_content(&self, window: &Window, cx: &AppContext) -> bool {
        self.read(cx).has_clipboard_content(cx)
    }

    fn has_buffer_content(&self, window: &Window, cx: &AppContext) -> bool {
        self.read(cx).has_buffer_content(cx)
    }

    fn buffer_content(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::AppContext,
    ) -> Option<Model<Buffer>> {
        self.update(cx, |item, model, cx| item.buffer_content(cx))
    }
}

pub enum Output {
    Plain {
        content: View<TerminalOutput>,
        display_id: Option<String>,
    },
    Stream {
        content: View<TerminalOutput>,
    },
    Image {
        content: View<ImageView>,
        display_id: Option<String>,
    },
    ErrorOutput(ErrorView),
    Message(String),
    Table {
        content: View<TableView>,
        display_id: Option<String>,
    },
    Markdown {
        content: View<MarkdownView>,
        display_id: Option<String>,
    },
    ClearOutputWaitMarker,
}

impl Output {
    fn render_output_controls<V: OutputContent + 'static>(
        v: View<V>,
        workspace: WeakView<Workspace>,
        model: &Model<ExecutionView>, cx: &mut AppContext,
    ) -> Option<AnyElement> {
        if !v.has_clipboard_content(cx) && !v.has_buffer_content(cx) {
            return None;
        }

        Some(
            h_flex()
                .pl_1()
                .when(v.has_clipboard_content(cx), |el| {
                    let v = v.clone();
                    el.child(
                        IconButton::new(ElementId::Name("copy-output".into()), IconName::Copy)
                            .style(ButtonStyle::Transparent)
                            .tooltip(move |cx| Tooltip::text("Copy Output", cx))
                            .on_click(cx.listener(move |_, _, cx| {
                                let clipboard_content = v.clipboard_content(cx);

                                if let Some(clipboard_content) = clipboard_content.as_ref() {
                                    cx.write_to_clipboard(clipboard_content.clone());
                                }
                            })),
                    )
                })
                .when(v.has_buffer_content(cx), |el| {
                    let v = v.clone();
                    el.child(
                        IconButton::new(
                            ElementId::Name("open-in-buffer".into()),
                            IconName::FileText,
                        )
                        .style(ButtonStyle::Transparent)
                        .tooltip(move |cx| Tooltip::text("Open in Buffer", cx))
                        .on_click(cx.listener({
                            let workspace = workspace.clone();

                            move |_, _, cx| {
                                let buffer_content =
                                    v.update(cx, |item, model, cx| item.buffer_content(cx));

                                if let Some(buffer_content) = buffer_content.as_ref() {
                                    let buffer = buffer_content.clone();
                                    let editor = Box::new(cx.new_model(|model, cx| {
                                        let multibuffer = cx.new_model(|model, cx| {
                                            let mut multi_buffer =
                                                MultiBuffer::singleton(buffer.clone(), model, cx);

                                            multi_buffer.set_title("REPL Output".to_string(), model, cx);
                                            multi_buffer
                                        });

                                        Editor::for_multibuffer(multibuffer, None, false, model, cx)
                                    }));
                                    workspace
                                        .update(cx, |workspace, model, cx| {
                                            workspace
                                                .add_item_to_active_pane(editor, None, true, cx);
                                        })
                                        .ok();
                                }
                            }
                        })),
                    )
                })
                .into_any_element(),
        )
    }

    pub fn render(
        &self,

        workspace: WeakView<Workspace>,
        model: &Model<ExecutionView>, cx: &mut AppContext,
    ) -> impl IntoElement {
        let content = match self {
            Self::Plain { content, .. } => Some(content.clone().into_any_element()),
            Self::Markdown { content, .. } => Some(content.clone().into_any_element()),
            Self::Stream { content, .. } => Some(content.clone().into_any_element()),
            Self::Image { content, .. } => Some(content.clone().into_any_element()),
            Self::Message(message) => Some(div().child(message.clone()).into_any_element()),
            Self::Table { content, .. } => Some(content.clone().into_any_element()),
            Self::ErrorOutput(error_view) => error_view.render(cx),
            Self::ClearOutputWaitMarker => None,
        };

        h_flex()
            .w_full()
            .items_start()
            .child(div().flex_1().children(content))
            .children(match self {
                Self::Plain { content, .. } => {
                    Self::render_output_controls(content.clone(), workspace.clone(), model, cx)
                }
                Self::Markdown { content, .. } => {
                    Self::render_output_controls(content.clone(), workspace.clone(), model, cx)
                }
                Self::Stream { content, .. } => {
                    Self::render_output_controls(content.clone(), workspace.clone(), model, cx)
                }
                Self::Image { content, .. } => {
                    Self::render_output_controls(content.clone(), workspace.clone(), model, cx)
                }
                Self::ErrorOutput(err) => {
                    Self::render_output_controls(err.traceback.clone(), workspace.clone(), model, cx)
                }
                Self::Message(_) => None,
                Self::Table { content, .. } => {
                    Self::render_output_controls(content.clone(), workspace.clone(), model, cx)
                }
                Self::ClearOutputWaitMarker => None,
            })
    }

    pub fn display_id(&self) -> Option<String> {
        match self {
            Output::Plain { display_id, .. } => display_id.clone(),
            Output::Stream { .. } => None,
            Output::Image { display_id, .. } => display_id.clone(),
            Output::ErrorOutput(_) => None,
            Output::Message(_) => None,
            Output::Table { display_id, .. } => display_id.clone(),
            Output::Markdown { display_id, .. } => display_id.clone(),
            Output::ClearOutputWaitMarker => None,
        }
    }

    pub fn new(
        data: &MimeBundle,
        display_id: Option<String>,
        window: &mut gpui::Window,
        cx: &mut gpui::AppContext,
    ) -> Self {
        match data.richest(rank_mime_type) {
            Some(MimeType::Plain(text)) => Output::Plain {
                content: cx.new_model(|model, cx| TerminalOutput::from(text, cx)),
                display_id,
            },
            Some(MimeType::Markdown(text)) => {
                let view = cx.new_model(|model, cx| MarkdownView::from(text.clone(), model, cx));
                Output::Markdown {
                    content: view,
                    display_id,
                }
            }
            Some(MimeType::Png(data)) | Some(MimeType::Jpeg(data)) => match ImageView::from(data) {
                Ok(view) => Output::Image {
                    content: cx.new_model(|_| view),
                    display_id,
                },
                Err(error) => Output::Message(format!("Failed to load image: {}", error)),
            },
            Some(MimeType::DataTable(data)) => Output::Table {
                content: cx.new_model(|model, cx| TableView::new(data, model, cx)),
                display_id,
            },
            // Any other media types are not supported
            _ => Output::Message("Unsupported media type".to_string()),
        }
    }
}

#[derive(Default, Clone, Debug)]
pub enum ExecutionStatus {
    #[default]
    Unknown,
    ConnectingToKernel,
    Queued,
    Executing,
    Finished,
    ShuttingDown,
    Shutdown,
    KernelErrored(String),
    Restarting,
}

/// An ExecutionView shows the outputs of an execution.
/// It can hold zero or more outputs, which the user
/// sees as "the output" for a single execution.
pub struct ExecutionView {
    #[allow(unused)]
    workspace: WeakView<Workspace>,
    pub outputs: Vec<Output>,
    pub status: ExecutionStatus,
}

impl ExecutionView {
    pub fn new(
        status: ExecutionStatus,
        workspace: WeakView<Workspace>,
        model: &Model<>Self, _cx: &mut AppContext,
    ) -> Self {
        Self {
            workspace,
            outputs: Default::default(),
            status,
        }
    }

    /// Accept a Jupyter message belonging to this execution
    pub fn push_message(&mut self, message: &JupyterMessageContent, model: &Model<Self>, cx: &mut AppContext) {
        let output: Output = match message {
            JupyterMessageContent::ExecuteResult(result) => Output::new(
                &result.data,
                result.transient.as_ref().and_then(|t| t.display_id.clone()),
                cx,
            ),
            JupyterMessageContent::DisplayData(result) => Output::new(
                &result.data,
                result.transient.as_ref().and_then(|t| t.display_id.clone()),
                cx,
            ),
            JupyterMessageContent::StreamContent(result) => {
                // Previous stream data will combine together, handling colors, carriage returns, etc
                if let Some(new_terminal) = self.apply_terminal_text(&result.text, model, cx) {
                    new_terminal
                } else {
                    return;
                }
            }
            JupyterMessageContent::ErrorOutput(result) => {
                let terminal =
                    cx.new_model(|model, cx| TerminalOutput::from(&result.traceback.join("\n"), cx));

                Output::ErrorOutput(ErrorView {
                    ename: result.ename.clone(),
                    evalue: result.evalue.clone(),
                    traceback: terminal,
                })
            }
            JupyterMessageContent::ExecuteReply(reply) => {
                for payload in reply.payload.iter() {
                    if let runtimelib::Payload::Page { data, .. } = payload {
                        let output = Output::new(data, None, model, cx);
                        self.outputs.push(output);
                    }
                }
                model.notify(cx);
                return;
            }
            JupyterMessageContent::ClearOutput(options) => {
                if !options.wait {
                    self.outputs.clear();
                    model.notify(cx);
                    return;
                }

                // Create a marker to clear the output after we get in a new output
                Output::ClearOutputWaitMarker
            }
            JupyterMessageContent::Status(status) => {
                match status.execution_state {
                    ExecutionState::Busy => {
                        self.status = ExecutionStatus::Executing;
                    }
                    ExecutionState::Idle => self.status = ExecutionStatus::Finished,
                }
                model.notify(cx);
                return;
            }
            _msg => {
                return;
            }
        };

        // Check for a clear output marker as the previous output, so we can clear it out
        if let Some(output) = self.outputs.last() {
            if let Output::ClearOutputWaitMarker = output {
                self.outputs.clear();
            }
        }

        self.outputs.push(output);

        model.notify(cx);
    }

    pub fn update_display_data(
        &mut self,
        data: &MimeBundle,
        display_id: &str,
        model: &Model<Self>, cx: &mut AppContext,
    ) {
        let mut any = false;

        self.outputs.iter_mut().for_each(|output| {
            if let Some(other_display_id) = output.display_id().as_ref() {
                if other_display_id == display_id {
                    *output = Output::new(data, Some(display_id.to_owned()), model, cx);
                    any = true;
                }
            }
        });

        if any {
            model.notify(cx);
        }
    }

    fn apply_terminal_text(&mut self, text: &str, model: &Model<Self>, cx: &mut AppContext) -> Option<Output> {
        if let Some(last_output) = self.outputs.last_mut() {
            if let Output::Stream {
                content: last_stream,
            } = last_output
            {
                // Don't need to add a new output, we already have a terminal output
                // and can just update the most recent terminal output
                last_stream.update(cx, |last_stream, model, cx| {
                    last_stream.append_text(text, cx);
                    model.notify(cx);
                });
                return None;
            }
        }

        Some(Output::Stream {
            content: cx.new_model(|model, cx| TerminalOutput::from(text, cx)),
        })
    }
}

impl Render for ExecutionView {
    fn render(&mut self, model: &Model<Self>, cx: &mut AppContext) -> impl IntoElement {
        let status = match &self.status {
            ExecutionStatus::ConnectingToKernel => Label::new("Connecting to kernel...")
                .color(Color::Muted)
                .into_any_element(),
            ExecutionStatus::Executing => h_flex()
                .gap_2()
                .child(
                    Icon::new(IconName::ArrowCircle)
                        .size(IconSize::Small)
                        .color(Color::Muted)
                        .with_animation(
                            "arrow-circle",
                            Animation::new(Duration::from_secs(3)).repeat(),
                            |icon, delta| icon.transform(Transformation::rotate(percentage(delta))),
                        ),
                )
                .child(Label::new("Executing...").color(Color::Muted))
                .into_any_element(),
            ExecutionStatus::Finished => Icon::new(IconName::Check)
                .size(IconSize::Small)
                .into_any_element(),
            ExecutionStatus::Unknown => Label::new("Unknown status")
                .color(Color::Muted)
                .into_any_element(),
            ExecutionStatus::ShuttingDown => Label::new("Kernel shutting down...")
                .color(Color::Muted)
                .into_any_element(),
            ExecutionStatus::Restarting => Label::new("Kernel restarting...")
                .color(Color::Muted)
                .into_any_element(),
            ExecutionStatus::Shutdown => Label::new("Kernel shutdown")
                .color(Color::Muted)
                .into_any_element(),
            ExecutionStatus::Queued => Label::new("Queued...")
                .color(Color::Muted)
                .into_any_element(),
            ExecutionStatus::KernelErrored(error) => Label::new(format!("Kernel error: {}", error))
                .color(Color::Error)
                .into_any_element(),
        };

        if self.outputs.is_empty() {
            return v_flex()
                .min_h(cx.line_height())
                .justify_center()
                .child(status)
                .into_any_element();
        }

        div()
            .w_full()
            .children(
                self.outputs
                    .iter()
                    .map(|output| output.render(self.workspace.clone(), model, cx)),
            )
            .children(match self.status {
                ExecutionStatus::Executing => vec![status],
                ExecutionStatus::Queued => vec![status],
                _ => vec![],
            })
            .into_any_element()
    }
}
