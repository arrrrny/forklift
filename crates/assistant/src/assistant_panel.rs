use crate::{
    assistant_settings::{AssistantDockPosition, AssistantSettings},
    humanize_token_count,
    prompt_library::open_prompt_library,
    prompts::PromptBuilder,
    slash_command::{
        default_command::DefaultSlashCommand,
        docs_command::{DocsSlashCommand, DocsSlashCommandArgs},
        file_command::{self, codeblock_fence_for_path},
        SlashCommandCompletionProvider, SlashCommandRegistry,
    },
    slash_command_picker,
    terminal_inline_assistant::TerminalInlineAssistant,
    Assist, CacheStatus, ConfirmCommand, Content, Context, ContextEvent, ContextId, ContextStore,
    ContextStoreEvent, CopyCode, CycleMessageRole, DeployHistory, DeployPromptLibrary,
    InlineAssistId, InlineAssistant, InsertDraggedFiles, InsertIntoEditor, Message, MessageId,
    MessageMetadata, MessageStatus, ModelPickerDelegate, ModelSelector, NewContext,
    PendingSlashCommand, PendingSlashCommandStatus, QuoteSelection, RemoteContextMetadata,
    SavedContextMetadata, Split, ToggleFocus, ToggleModelSelector, WorkflowStepResolution,
};
use anyhow::Result;
use assistant_slash_command::{SlashCommand, SlashCommandOutputSection};
use assistant_tool::ToolRegistry;
use client::{proto, Client, Status};
use collections::{BTreeSet, HashMap, HashSet};
use editor::{
    actions::{FoldAt, MoveToEndOfLine, Newline, ShowCompletions, UnfoldAt},
    display_map::{
        BlockDisposition, BlockId, BlockProperties, BlockStyle, Crease, CreaseMetadata,
        CustomBlockId, FoldId, RenderBlock, ToDisplayPoint,
    },
    scroll::{Autoscroll, AutoscrollStrategy, ScrollAnchor},
    Anchor, Editor, EditorEvent, ExcerptRange, MultiBuffer, RowExt, ToOffset as _, ToPoint,
};
use editor::{display_map::CreaseId, FoldPlaceholder};
use fs::Fs;
use futures::FutureExt;
use gpui::{
    canvas, div, img, percentage, point, pulsating_between, size, Action, Animation, AnimationExt,
    AnyElement, AnyView, AppContext, AsyncWindowContext, ClipboardEntry, ClipboardItem,
    Context as _, Empty, Entity, EntityId, EventEmitter, ExternalPaths, FocusHandle, FocusableView,
    FontWeight, InteractiveElement, IntoElement, Model, ParentElement, Pixels, ReadGlobal, Render,
    RenderImage, SharedString, Size, StatefulInteractiveElement, Styled, Subscription, Task,
    Transformation, UpdateGlobal, View, VisualContext, WeakView, WindowContext,
};
use indexed_docs::IndexedDocsStore;
use language::{
    language_settings::SoftWrap, BufferSnapshot, Capability, LanguageRegistry, LspAdapterDelegate,
    ToOffset,
};
use language_model::{
    provider::cloud::PROVIDER_ID, LanguageModelProvider, LanguageModelProviderId,
    LanguageModelRegistry, Role,
};
use language_model::{LanguageModelImage, LanguageModelToolUse};
use multi_buffer::MultiBufferRow;
use picker::{Picker, PickerDelegate};
use project::lsp_store::LocalLspAdapterDelegate;
use project::{Project, Worktree};
use rope::Point;
use search::{buffer_search::DivRegistrar, BufferSearchBar};
use serde::{Deserialize, Serialize};
use settings::{update_settings_file, Settings};
use smol::stream::StreamExt;
use std::{
    borrow::Cow,
    cmp,
    collections::hash_map,
    ops::{ControlFlow, Range},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use terminal_view::{terminal_panel::TerminalPanel, TerminalView};
use text::SelectionGoal;
use ui::TintColor;
use ui::{
    prelude::*,
    utils::{format_distance_from_now, DateTimeType},
    Avatar, ButtonLike, ContextMenu, Disclosure, ElevationIndex, KeyBinding, ListItem,
    ListItemSpacing, PopoverMenu, PopoverMenuHandle, Tooltip,
};
use util::{maybe, ResultExt};
use workspace::{
    dock::{DockPosition, Panel, PanelEvent},
    item::{self, FollowableItem, Item, ItemHandle},
    notifications::NotificationId,
    pane::{self, SaveIntent},
    searchable::{SearchEvent, SearchableItem},
    DraggedSelection, Pane, Save, ShowConfiguration, Toast, ToggleZoom, ToolbarItemEvent,
    ToolbarItemLocation, ToolbarItemView, Workspace,
};
use workspace::{searchable::SearchableItemHandle, DraggedTab};
use zed_actions::InlineAssist;

pub fn init(cx: &mut AppContext) {
    workspace::FollowableViewRegistry::register::<ContextEditor>(cx);
    cx.observe_new_views(
        |workspace: &mut Workspace, _cx: &mut ViewContext<Workspace>| {
            workspace
                .register_action(|workspace, _: &ToggleFocus, cx| {
                    let settings = AssistantSettings::get_global(cx);
                    if !settings.enabled {
                        return;
                    }

                    workspace.toggle_panel_focus::<AssistantPanel>(cx);
                })
                .register_action(AssistantPanel::inline_assist)
                .register_action(ContextEditor::quote_selection)
                .register_action(ContextEditor::insert_selection)
                .register_action(ContextEditor::copy_code)
                .register_action(ContextEditor::insert_dragged_files)
                .register_action(AssistantPanel::show_configuration)
                .register_action(AssistantPanel::create_new_context);
        },
    )
    .detach();

    cx.observe_new_views(
        |terminal_panel: &mut TerminalPanel, cx: &mut ViewContext<TerminalPanel>| {
            let settings = AssistantSettings::get_global(cx);
            terminal_panel.asssistant_enabled(settings.enabled, cx);
        },
    )
    .detach();
}

pub enum AssistantPanelEvent {
    ContextEdited,
}

pub struct AssistantPanel {
    pane: View<Pane>,
    workspace: WeakView<Workspace>,
    width: Option<Pixels>,
    height: Option<Pixels>,
    project: Model<Project>,
    context_store: Model<ContextStore>,
    languages: Arc<LanguageRegistry>,
    fs: Arc<dyn Fs>,
    subscriptions: Vec<Subscription>,
    model_selector_menu_handle: PopoverMenuHandle<Picker<ModelPickerDelegate>>,
    model_summary_editor: View<Editor>,
    authenticate_provider_task: Option<(LanguageModelProviderId, Task<()>)>,
    configuration_subscription: Option<Subscription>,
    client_status: Option<client::Status>,
    watch_client_status: Option<Task<()>>,
    show_zed_ai_notice: bool,
}

#[derive(Clone)]
enum ContextMetadata {
    Remote(RemoteContextMetadata),
    Saved(SavedContextMetadata),
}

struct SavedContextPickerDelegate {
    store: Model<ContextStore>,
    project: Model<Project>,
    matches: Vec<ContextMetadata>,
    selected_index: usize,
}

enum SavedContextPickerEvent {
    Confirmed(ContextMetadata),
}

enum InlineAssistTarget {
    Editor(View<Editor>, bool),
    Terminal(View<TerminalView>),
}

impl EventEmitter<SavedContextPickerEvent> for Picker<SavedContextPickerDelegate> {}

impl SavedContextPickerDelegate {
    fn new(project: Model<Project>, store: Model<ContextStore>) -> Self {
        Self {
            project,
            store,
            matches: Vec::new(),
            selected_index: 0,
        }
    }
}

impl PickerDelegate for SavedContextPickerDelegate {
    type ListItem = ListItem;

    fn match_count(&self) -> usize {
        self.matches.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn set_selected_index(&mut self, ix: usize, _cx: &mut ViewContext<Picker<Self>>) {
        self.selected_index = ix;
    }

    fn placeholder_text(&self, _cx: &mut WindowContext) -> Arc<str> {
        "Search...".into()
    }

    fn update_matches(&mut self, query: String, cx: &mut ViewContext<Picker<Self>>) -> Task<()> {
        let search = self.store.read(cx).search(query, cx);
        cx.spawn(|this, mut cx| async move {
            let matches = search.await;
            this.update(&mut cx, |this, cx| {
                let host_contexts = this.delegate.store.read(cx).host_contexts();
                this.delegate.matches = host_contexts
                    .iter()
                    .cloned()
                    .map(ContextMetadata::Remote)
                    .chain(matches.into_iter().map(ContextMetadata::Saved))
                    .collect();
                this.delegate.selected_index = 0;
                cx.notify();
            })
            .ok();
        })
    }

    fn confirm(&mut self, _secondary: bool, cx: &mut ViewContext<Picker<Self>>) {
        if let Some(metadata) = self.matches.get(self.selected_index) {
            cx.emit(SavedContextPickerEvent::Confirmed(metadata.clone()));
        }
    }

    fn dismissed(&mut self, _cx: &mut ViewContext<Picker<Self>>) {}

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        cx: &mut ViewContext<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let context = self.matches.get(ix)?;
        let item = match context {
            ContextMetadata::Remote(context) => {
                let host_user = self.project.read(cx).host().and_then(|collaborator| {
                    self.project
                        .read(cx)
                        .user_store()
                        .read(cx)
                        .get_cached_user(collaborator.user_id)
                });
                div()
                    .flex()
                    .w_full()
                    .justify_between()
                    .gap_2()
                    .child(
                        h_flex().flex_1().overflow_x_hidden().child(
                            Label::new(context.summary.clone().unwrap_or(DEFAULT_TAB_TITLE.into()))
                                .size(LabelSize::Small),
                        ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .children(if let Some(host_user) = host_user {
                                vec![
                                    Avatar::new(host_user.avatar_uri.clone()).into_any_element(),
                                    Label::new(format!("Shared by @{}", host_user.github_login))
                                        .color(Color::Muted)
                                        .size(LabelSize::Small)
                                        .into_any_element(),
                                ]
                            } else {
                                vec![Label::new("Shared by host")
                                    .color(Color::Muted)
                                    .size(LabelSize::Small)
                                    .into_any_element()]
                            }),
                    )
            }
            ContextMetadata::Saved(context) => div()
                .flex()
                .w_full()
                .justify_between()
                .gap_2()
                .child(
                    h_flex()
                        .flex_1()
                        .child(Label::new(context.title.clone()).size(LabelSize::Small))
                        .overflow_x_hidden(),
                )
                .child(
                    Label::new(format_distance_from_now(
                        DateTimeType::Local(context.mtime),
                        false,
                        true,
                        true,
                    ))
                    .color(Color::Muted)
                    .size(LabelSize::Small),
                ),
        };
        Some(
            ListItem::new(ix)
                .inset(true)
                .spacing(ListItemSpacing::Sparse)
                .selected(selected)
                .child(item),
        )
    }
}

impl AssistantPanel {
    pub fn load(
        workspace: WeakView<Workspace>,
        prompt_builder: Arc<PromptBuilder>,
        cx: AsyncWindowContext,
    ) -> Task<Result<View<Self>>> {
        cx.spawn(|mut cx| async move {
            let context_store = workspace
                .update(&mut cx, |workspace, cx| {
                    let project = workspace.project().clone();
                    ContextStore::new(project, prompt_builder.clone(), cx)
                })?
                .await?;

            workspace.update(&mut cx, |workspace, cx| {
                // TODO: deserialize state.
                cx.new_view(|cx| Self::new(workspace, context_store, cx))
            })
        })
    }

    fn new(
        workspace: &Workspace,
        context_store: Model<ContextStore>,
        cx: &mut ViewContext<Self>,
    ) -> Self {
        let model_selector_menu_handle = PopoverMenuHandle::default();
        let model_summary_editor = cx.new_view(Editor::single_line);
        let context_editor_toolbar = cx.new_view(|_| {
            ContextEditorToolbarItem::new(
                workspace,
                model_selector_menu_handle.clone(),
                model_summary_editor.clone(),
            )
        });

        let pane = cx.new_view(|cx| {
            let mut pane = Pane::new(
                workspace.weak_handle(),
                workspace.project().clone(),
                Default::default(),
                None,
                NewContext.boxed_clone(),
                cx,
            );

            let project = workspace.project().clone();
            pane.set_custom_drop_handle(cx, move |_, dropped_item, cx| {
                let action = maybe!({
                    if let Some(paths) = dropped_item.downcast_ref::<ExternalPaths>() {
                        return Some(InsertDraggedFiles::ExternalFiles(paths.paths().to_vec()));
                    }

                    let project_paths = if let Some(tab) = dropped_item.downcast_ref::<DraggedTab>()
                    {
                        if &tab.pane == cx.view() {
                            return None;
                        }
                        let item = tab.pane.read(cx).item_for_index(tab.ix);
                        Some(
                            item.and_then(|item| item.project_path(cx))
                                .into_iter()
                                .collect::<Vec<_>>(),
                        )
                    } else if let Some(selection) = dropped_item.downcast_ref::<DraggedSelection>()
                    {
                        Some(
                            selection
                                .items()
                                .filter_map(|item| {
                                    project.read(cx).path_for_entry(item.entry_id, cx)
                                })
                                .collect::<Vec<_>>(),
                        )
                    } else {
                        None
                    }?;

                    let paths = project_paths
                        .into_iter()
                        .filter_map(|project_path| {
                            let worktree = project
                                .read(cx)
                                .worktree_for_id(project_path.worktree_id, cx)?;

                            let mut full_path = PathBuf::from(worktree.read(cx).root_name());
                            full_path.push(&project_path.path);
                            Some(full_path)
                        })
                        .collect::<Vec<_>>();

                    Some(InsertDraggedFiles::ProjectPaths(paths))
                });

                if let Some(action) = action {
                    cx.dispatch_action(action.boxed_clone());
                }

                ControlFlow::Break(())
            });

            pane.set_can_split(false, cx);
            pane.set_can_navigate(true, cx);
            pane.display_nav_history_buttons(None);
            pane.set_should_display_tab_bar(|_| true);
            pane.set_render_tab_bar_buttons(cx, move |pane, cx| {
                let focus_handle = pane.focus_handle(cx);
                let left_children = IconButton::new("history", IconName::HistoryRerun)
                    .icon_size(IconSize::Small)
                    .on_click(cx.listener({
                        let focus_handle = focus_handle.clone();
                        move |_, _, cx| {
                            focus_handle.focus(cx);
                            cx.dispatch_action(DeployHistory.boxed_clone())
                        }
                    }))
                    .tooltip({
                        let focus_handle = focus_handle.clone();
                        move |cx| {
                            Tooltip::for_action_in(
                                "Open History",
                                &DeployHistory,
                                &focus_handle,
                                cx,
                            )
                        }
                    })
                    .selected(
                        pane.active_item()
                            .map_or(false, |item| item.downcast::<ContextHistory>().is_some()),
                    );
                let _pane = cx.view().clone();
                let right_children = h_flex()
                    .gap(Spacing::Small.rems(cx))
                    .child(
                        IconButton::new("new-context", IconName::Plus)
                            .on_click(
                                cx.listener(|_, _, cx| {
                                    cx.dispatch_action(NewContext.boxed_clone())
                                }),
                            )
                            .tooltip(move |cx| {
                                Tooltip::for_action_in(
                                    "New Context",
                                    &NewContext,
                                    &focus_handle,
                                    cx,
                                )
                            }),
                    )
                    .child(
                        PopoverMenu::new("assistant-panel-popover-menu")
                            .trigger(
                                IconButton::new("menu", IconName::Menu).icon_size(IconSize::Small),
                            )
                            .menu(move |cx| {
                                let zoom_label = if _pane.read(cx).is_zoomed() {
                                    "Zoom Out"
                                } else {
                                    "Zoom In"
                                };
                                let focus_handle = _pane.focus_handle(cx);
                                Some(ContextMenu::build(cx, move |menu, _| {
                                    menu.context(focus_handle.clone())
                                        .action("New Context", Box::new(NewContext))
                                        .action("History", Box::new(DeployHistory))
                                        .action("Prompt Library", Box::new(DeployPromptLibrary))
                                        .action("Configure", Box::new(ShowConfiguration))
                                        .action(zoom_label, Box::new(ToggleZoom))
                                }))
                            }),
                    )
                    .into_any_element()
                    .into();

                (Some(left_children.into_any_element()), right_children)
            });
            pane.toolbar().update(cx, |toolbar, cx| {
                toolbar.add_item(context_editor_toolbar.clone(), cx);
                toolbar.add_item(cx.new_view(BufferSearchBar::new), cx)
            });
            pane
        });

        let subscriptions = vec![
            cx.observe(&pane, |_, _, cx| cx.notify()),
            cx.subscribe(&pane, Self::handle_pane_event),
            cx.subscribe(&context_editor_toolbar, Self::handle_toolbar_event),
            cx.subscribe(&model_summary_editor, Self::handle_summary_editor_event),
            cx.subscribe(&context_store, Self::handle_context_store_event),
            cx.subscribe(
                &LanguageModelRegistry::global(cx),
                |this, _, event: &language_model::Event, cx| match event {
                    language_model::Event::ActiveModelChanged => {
                        this.completion_provider_changed(cx);
                    }
                    language_model::Event::ProviderStateChanged => {
                        this.ensure_authenticated(cx);
                        cx.notify()
                    }
                    language_model::Event::AddedProvider(_)
                    | language_model::Event::RemovedProvider(_) => {
                        this.ensure_authenticated(cx);
                    }
                },
            ),
        ];

        let watch_client_status = Self::watch_client_status(workspace.client().clone(), cx);

        let mut this = Self {
            pane,
            workspace: workspace.weak_handle(),
            width: None,
            height: None,
            project: workspace.project().clone(),
            context_store,
            languages: workspace.app_state().languages.clone(),
            fs: workspace.app_state().fs.clone(),
            subscriptions,
            model_selector_menu_handle,
            model_summary_editor,
            authenticate_provider_task: None,
            configuration_subscription: None,
            client_status: None,
            watch_client_status: Some(watch_client_status),
            show_zed_ai_notice: false,
        };
        this.new_context(cx);
        this
    }

    fn watch_client_status(client: Arc<Client>, cx: &mut ViewContext<Self>) -> Task<()> {
        let mut status_rx = client.status();

        cx.spawn(|this, mut cx| async move {
            while let Some(status) = status_rx.next().await {
                this.update(&mut cx, |this, cx| {
                    if this.client_status.is_none()
                        || this
                            .client_status
                            .map_or(false, |old_status| old_status != status)
                    {
                        this.update_zed_ai_notice_visibility(status, cx);
                    }
                    this.client_status = Some(status);
                })
                .log_err();
            }
            this.update(&mut cx, |this, _cx| this.watch_client_status = None)
                .log_err();
        })
    }

    fn handle_pane_event(
        &mut self,
        pane: View<Pane>,
        event: &pane::Event,
        cx: &mut ViewContext<Self>,
    ) {
        let update_model_summary = match event {
            pane::Event::Remove { .. } => {
                cx.emit(PanelEvent::Close);
                false
            }
            pane::Event::ZoomIn => {
                cx.emit(PanelEvent::ZoomIn);
                false
            }
            pane::Event::ZoomOut => {
                cx.emit(PanelEvent::ZoomOut);
                false
            }

            pane::Event::AddItem { item } => {
                self.workspace
                    .update(cx, |workspace, cx| {
                        item.added_to_pane(workspace, self.pane.clone(), cx)
                    })
                    .ok();
                true
            }

            pane::Event::ActivateItem { local } => {
                if *local {
                    self.workspace
                        .update(cx, |workspace, cx| {
                            workspace.unfollow_in_pane(&pane, cx);
                        })
                        .ok();
                }
                cx.emit(AssistantPanelEvent::ContextEdited);
                true
            }
            pane::Event::RemovedItem { .. } => {
                let has_configuration_view = self
                    .pane
                    .read(cx)
                    .items_of_type::<ConfigurationView>()
                    .next()
                    .is_some();

                if !has_configuration_view {
                    self.configuration_subscription = None;
                }

                cx.emit(AssistantPanelEvent::ContextEdited);
                true
            }

            _ => false,
        };

        if update_model_summary {
            if let Some(editor) = self.active_context_editor(cx) {
                self.show_updated_summary(&editor, cx)
            }
        }
    }

    fn handle_summary_editor_event(
        &mut self,
        model_summary_editor: View<Editor>,
        event: &EditorEvent,
        cx: &mut ViewContext<Self>,
    ) {
        if matches!(event, EditorEvent::Edited { .. }) {
            if let Some(context_editor) = self.active_context_editor(cx) {
                let new_summary = model_summary_editor.read(cx).text(cx);
                context_editor.update(cx, |context_editor, cx| {
                    context_editor.context.update(cx, |context, cx| {
                        if context.summary().is_none()
                            && (new_summary == DEFAULT_TAB_TITLE || new_summary.trim().is_empty())
                        {
                            return;
                        }
                        context.custom_summary(new_summary, cx)
                    });
                });
            }
        }
    }

    fn update_zed_ai_notice_visibility(
        &mut self,
        client_status: Status,
        cx: &mut ViewContext<Self>,
    ) {
        let active_provider = LanguageModelRegistry::read_global(cx).active_provider();

        // If we're signed out and don't have a provider configured, or we're signed-out AND Zed.dev is
        // the provider, we want to show a nudge to sign in.
        let show_zed_ai_notice = client_status.is_signed_out()
            && active_provider.map_or(true, |provider| provider.id().0 == PROVIDER_ID);

        self.show_zed_ai_notice = show_zed_ai_notice;
        cx.notify();
    }

    fn handle_toolbar_event(
        &mut self,
        _: View<ContextEditorToolbarItem>,
        _: &ContextEditorToolbarItemEvent,
        cx: &mut ViewContext<Self>,
    ) {
        if let Some(context_editor) = self.active_context_editor(cx) {
            context_editor.update(cx, |context_editor, cx| {
                context_editor.context.update(cx, |context, cx| {
                    context.summarize(true, cx);
                })
            })
        }
    }

    fn handle_context_store_event(
        &mut self,
        _context_store: Model<ContextStore>,
        event: &ContextStoreEvent,
        cx: &mut ViewContext<Self>,
    ) {
        let ContextStoreEvent::ContextCreated(context_id) = event;
        let Some(context) = self
            .context_store
            .read(cx)
            .loaded_context_for_id(&context_id, cx)
        else {
            log::error!("no context found with ID: {}", context_id.to_proto());
            return;
        };
        let lsp_adapter_delegate = make_lsp_adapter_delegate(&self.project, cx)
            .log_err()
            .flatten();

        let assistant_panel = cx.view().downgrade();
        let editor = cx.new_view(|cx| {
            let mut editor = ContextEditor::for_context(
                context,
                self.fs.clone(),
                self.workspace.clone(),
                self.project.clone(),
                lsp_adapter_delegate,
                assistant_panel,
                cx,
            );
            editor.insert_default_prompt(cx);
            editor
        });

        self.show_context(editor.clone(), cx);
    }

    fn completion_provider_changed(&mut self, cx: &mut ViewContext<Self>) {
        if let Some(editor) = self.active_context_editor(cx) {
            editor.update(cx, |active_context, cx| {
                active_context
                    .context
                    .update(cx, |context, cx| context.completion_provider_changed(cx))
            })
        }

        let Some(new_provider_id) = LanguageModelRegistry::read_global(cx)
            .active_provider()
            .map(|p| p.id())
        else {
            return;
        };

        if self
            .authenticate_provider_task
            .as_ref()
            .map_or(true, |(old_provider_id, _)| {
                *old_provider_id != new_provider_id
            })
        {
            self.authenticate_provider_task = None;
            self.ensure_authenticated(cx);
        }

        if let Some(status) = self.client_status {
            self.update_zed_ai_notice_visibility(status, cx);
        }
    }

    fn ensure_authenticated(&mut self, cx: &mut ViewContext<Self>) {
        if self.is_authenticated(cx) {
            return;
        }

        let Some(provider) = LanguageModelRegistry::read_global(cx).active_provider() else {
            return;
        };

        let load_credentials = self.authenticate(cx);

        if self.authenticate_provider_task.is_none() {
            self.authenticate_provider_task = Some((
                provider.id(),
                cx.spawn(|this, mut cx| async move {
                    if let Some(future) = load_credentials {
                        let _ = future.await;
                    }
                    this.update(&mut cx, |this, _cx| {
                        this.authenticate_provider_task = None;
                    })
                    .log_err();
                }),
            ));
        }
    }

    pub fn inline_assist(
        workspace: &mut Workspace,
        action: &InlineAssist,
        cx: &mut ViewContext<Workspace>,
    ) {
        let settings = AssistantSettings::get_global(cx);
        if !settings.enabled {
            return;
        }

        let Some(assistant_panel) = workspace.panel::<AssistantPanel>(cx) else {
            return;
        };

        let Some(inline_assist_target) =
            Self::resolve_inline_assist_target(workspace, &assistant_panel, cx)
        else {
            return;
        };

        let initial_prompt = action.prompt.clone();

        if assistant_panel.update(cx, |assistant, cx| assistant.is_authenticated(cx)) {
            match inline_assist_target {
                InlineAssistTarget::Editor(active_editor, include_context) => {
                    InlineAssistant::update_global(cx, |assistant, cx| {
                        assistant.assist(
                            &active_editor,
                            Some(cx.view().downgrade()),
                            include_context.then_some(&assistant_panel),
                            initial_prompt,
                            cx,
                        )
                    })
                }
                InlineAssistTarget::Terminal(active_terminal) => {
                    TerminalInlineAssistant::update_global(cx, |assistant, cx| {
                        assistant.assist(
                            &active_terminal,
                            Some(cx.view().downgrade()),
                            Some(&assistant_panel),
                            initial_prompt,
                            cx,
                        )
                    })
                }
            }
        } else {
            let assistant_panel = assistant_panel.downgrade();
            cx.spawn(|workspace, mut cx| async move {
                let Some(task) =
                    assistant_panel.update(&mut cx, |assistant, cx| assistant.authenticate(cx))?
                else {
                    let answer = cx
                        .prompt(
                            gpui::PromptLevel::Warning,
                            "No language model provider configured",
                            None,
                            &["Configure", "Cancel"],
                        )
                        .await
                        .ok();
                    if let Some(answer) = answer {
                        if answer == 0 {
                            cx.update(|cx| cx.dispatch_action(Box::new(ShowConfiguration)))
                                .ok();
                        }
                    }
                    return Ok(());
                };
                task.await?;
                if assistant_panel.update(&mut cx, |panel, cx| panel.is_authenticated(cx))? {
                    cx.update(|cx| match inline_assist_target {
                        InlineAssistTarget::Editor(active_editor, include_context) => {
                            let assistant_panel = if include_context {
                                assistant_panel.upgrade()
                            } else {
                                None
                            };
                            InlineAssistant::update_global(cx, |assistant, cx| {
                                assistant.assist(
                                    &active_editor,
                                    Some(workspace),
                                    assistant_panel.as_ref(),
                                    initial_prompt,
                                    cx,
                                )
                            })
                        }
                        InlineAssistTarget::Terminal(active_terminal) => {
                            TerminalInlineAssistant::update_global(cx, |assistant, cx| {
                                assistant.assist(
                                    &active_terminal,
                                    Some(workspace),
                                    assistant_panel.upgrade().as_ref(),
                                    initial_prompt,
                                    cx,
                                )
                            })
                        }
                    })?
                } else {
                    workspace.update(&mut cx, |workspace, cx| {
                        workspace.focus_panel::<AssistantPanel>(cx)
                    })?;
                }

                anyhow::Ok(())
            })
            .detach_and_log_err(cx)
        }
    }

    fn resolve_inline_assist_target(
        workspace: &mut Workspace,
        assistant_panel: &View<AssistantPanel>,
        cx: &mut WindowContext,
    ) -> Option<InlineAssistTarget> {
        if let Some(terminal_panel) = workspace.panel::<TerminalPanel>(cx) {
            if terminal_panel
                .read(cx)
                .focus_handle(cx)
                .contains_focused(cx)
            {
                if let Some(terminal_view) = terminal_panel.read(cx).pane().and_then(|pane| {
                    pane.read(cx)
                        .active_item()
                        .and_then(|t| t.downcast::<TerminalView>())
                }) {
                    return Some(InlineAssistTarget::Terminal(terminal_view));
                }
            }
        }
        let context_editor =
            assistant_panel
                .read(cx)
                .active_context_editor(cx)
                .and_then(|editor| {
                    let editor = &editor.read(cx).editor;
                    if editor.read(cx).is_focused(cx) {
                        Some(editor.clone())
                    } else {
                        None
                    }
                });

        if let Some(context_editor) = context_editor {
            Some(InlineAssistTarget::Editor(context_editor, false))
        } else if let Some(workspace_editor) = workspace
            .active_item(cx)
            .and_then(|item| item.act_as::<Editor>(cx))
        {
            Some(InlineAssistTarget::Editor(workspace_editor, true))
        } else if let Some(terminal_view) = workspace
            .active_item(cx)
            .and_then(|item| item.act_as::<TerminalView>(cx))
        {
            Some(InlineAssistTarget::Terminal(terminal_view))
        } else {
            None
        }
    }

    pub fn create_new_context(
        workspace: &mut Workspace,
        _: &NewContext,
        cx: &mut ViewContext<Workspace>,
    ) {
        if let Some(panel) = workspace.panel::<AssistantPanel>(cx) {
            let did_create_context = panel
                .update(cx, |panel, cx| {
                    panel.new_context(cx)?;

                    Some(())
                })
                .is_some();
            if did_create_context {
                ContextEditor::quote_selection(workspace, &Default::default(), cx);
            }
        }
    }

    fn new_context(&mut self, cx: &mut ViewContext<Self>) -> Option<View<ContextEditor>> {
        let project = self.project.read(cx);
        if project.is_via_collab() && project.dev_server_project_id().is_none() {
            let task = self
                .context_store
                .update(cx, |store, cx| store.create_remote_context(cx));

            cx.spawn(|this, mut cx| async move {
                let context = task.await?;

                this.update(&mut cx, |this, cx| {
                    let workspace = this.workspace.clone();
                    let project = this.project.clone();
                    let lsp_adapter_delegate =
                        make_lsp_adapter_delegate(&project, cx).log_err().flatten();

                    let fs = this.fs.clone();
                    let project = this.project.clone();
                    let weak_assistant_panel = cx.view().downgrade();

                    let editor = cx.new_view(|cx| {
                        ContextEditor::for_context(
                            context,
                            fs,
                            workspace,
                            project,
                            lsp_adapter_delegate,
                            weak_assistant_panel,
                            cx,
                        )
                    });

                    this.show_context(editor, cx);

                    anyhow::Ok(())
                })??;

                anyhow::Ok(())
            })
            .detach_and_log_err(cx);

            None
        } else {
            let context = self.context_store.update(cx, |store, cx| store.create(cx));
            let lsp_adapter_delegate = make_lsp_adapter_delegate(&self.project, cx)
                .log_err()
                .flatten();

            let assistant_panel = cx.view().downgrade();
            let editor = cx.new_view(|cx| {
                let mut editor = ContextEditor::for_context(
                    context,
                    self.fs.clone(),
                    self.workspace.clone(),
                    self.project.clone(),
                    lsp_adapter_delegate,
                    assistant_panel,
                    cx,
                );
                editor.insert_default_prompt(cx);
                editor
            });

            self.show_context(editor.clone(), cx);
            let workspace = self.workspace.clone();
            cx.spawn(move |_, mut cx| async move {
                workspace
                    .update(&mut cx, |workspace, cx| {
                        workspace.focus_panel::<AssistantPanel>(cx);
                    })
                    .ok();
            })
            .detach();
            Some(editor)
        }
    }

    fn show_context(&mut self, context_editor: View<ContextEditor>, cx: &mut ViewContext<Self>) {
        let focus = self.focus_handle(cx).contains_focused(cx);
        let prev_len = self.pane.read(cx).items_len();
        self.pane.update(cx, |pane, cx| {
            pane.add_item(Box::new(context_editor.clone()), focus, focus, None, cx)
        });

        if prev_len != self.pane.read(cx).items_len() {
            self.subscriptions
                .push(cx.subscribe(&context_editor, Self::handle_context_editor_event));
        }

        self.show_updated_summary(&context_editor, cx);

        cx.emit(AssistantPanelEvent::ContextEdited);
        cx.notify();
    }

    fn show_updated_summary(
        &self,
        context_editor: &View<ContextEditor>,
        cx: &mut ViewContext<Self>,
    ) {
        context_editor.update(cx, |context_editor, cx| {
            let new_summary = context_editor.title(cx).to_string();
            self.model_summary_editor.update(cx, |summary_editor, cx| {
                if summary_editor.text(cx) != new_summary {
                    summary_editor.set_text(new_summary, cx);
                }
            });
        });
    }

    fn handle_context_editor_event(
        &mut self,
        context_editor: View<ContextEditor>,
        event: &EditorEvent,
        cx: &mut ViewContext<Self>,
    ) {
        match event {
            EditorEvent::TitleChanged => {
                self.show_updated_summary(&context_editor, cx);
                cx.notify()
            }
            EditorEvent::Edited { .. } => cx.emit(AssistantPanelEvent::ContextEdited),
            _ => {}
        }
    }

    fn show_configuration(
        workspace: &mut Workspace,
        _: &ShowConfiguration,
        cx: &mut ViewContext<Workspace>,
    ) {
        let Some(panel) = workspace.panel::<AssistantPanel>(cx) else {
            return;
        };

        if !panel.focus_handle(cx).contains_focused(cx) {
            workspace.toggle_panel_focus::<AssistantPanel>(cx);
        }

        panel.update(cx, |this, cx| {
            this.show_configuration_tab(cx);
        })
    }

    fn show_configuration_tab(&mut self, cx: &mut ViewContext<Self>) {
        let configuration_item_ix = self
            .pane
            .read(cx)
            .items()
            .position(|item| item.downcast::<ConfigurationView>().is_some());

        if let Some(configuration_item_ix) = configuration_item_ix {
            self.pane.update(cx, |pane, cx| {
                pane.activate_item(configuration_item_ix, true, true, cx);
            });
        } else {
            let configuration = cx.new_view(ConfigurationView::new);
            self.configuration_subscription = Some(cx.subscribe(
                &configuration,
                |this, _, event: &ConfigurationViewEvent, cx| match event {
                    ConfigurationViewEvent::NewProviderContextEditor(provider) => {
                        if LanguageModelRegistry::read_global(cx)
                            .active_provider()
                            .map_or(true, |p| p.id() != provider.id())
                        {
                            if let Some(model) = provider.provided_models(cx).first().cloned() {
                                update_settings_file::<AssistantSettings>(
                                    this.fs.clone(),
                                    cx,
                                    move |settings, _| settings.set_model(model),
                                );
                            }
                        }

                        this.new_context(cx);
                    }
                },
            ));
            self.pane.update(cx, |pane, cx| {
                pane.add_item(Box::new(configuration), true, true, None, cx);
            });
        }
    }

    fn deploy_history(&mut self, _: &DeployHistory, cx: &mut ViewContext<Self>) {
        let history_item_ix = self
            .pane
            .read(cx)
            .items()
            .position(|item| item.downcast::<ContextHistory>().is_some());

        if let Some(history_item_ix) = history_item_ix {
            self.pane.update(cx, |pane, cx| {
                pane.activate_item(history_item_ix, true, true, cx);
            });
        } else {
            let assistant_panel = cx.view().downgrade();
            let history = cx.new_view(|cx| {
                ContextHistory::new(
                    self.project.clone(),
                    self.context_store.clone(),
                    assistant_panel,
                    cx,
                )
            });
            self.pane.update(cx, |pane, cx| {
                pane.add_item(Box::new(history), true, true, None, cx);
            });
        }
    }

    fn deploy_prompt_library(&mut self, _: &DeployPromptLibrary, cx: &mut ViewContext<Self>) {
        open_prompt_library(self.languages.clone(), cx).detach_and_log_err(cx);
    }

    fn toggle_model_selector(&mut self, _: &ToggleModelSelector, cx: &mut ViewContext<Self>) {
        self.model_selector_menu_handle.toggle(cx);
    }

    fn active_context_editor(&self, cx: &AppContext) -> Option<View<ContextEditor>> {
        self.pane
            .read(cx)
            .active_item()?
            .downcast::<ContextEditor>()
    }

    pub fn active_context(&self, cx: &AppContext) -> Option<Model<Context>> {
        Some(self.active_context_editor(cx)?.read(cx).context.clone())
    }

    fn open_saved_context(
        &mut self,
        path: PathBuf,
        cx: &mut ViewContext<Self>,
    ) -> Task<Result<()>> {
        let existing_context = self.pane.read(cx).items().find_map(|item| {
            item.downcast::<ContextEditor>()
                .filter(|editor| editor.read(cx).context.read(cx).path() == Some(&path))
        });
        if let Some(existing_context) = existing_context {
            return cx.spawn(|this, mut cx| async move {
                this.update(&mut cx, |this, cx| this.show_context(existing_context, cx))
            });
        }

        let context = self
            .context_store
            .update(cx, |store, cx| store.open_local_context(path.clone(), cx));
        let fs = self.fs.clone();
        let project = self.project.clone();
        let workspace = self.workspace.clone();

        let lsp_adapter_delegate = make_lsp_adapter_delegate(&project, cx).log_err().flatten();

        cx.spawn(|this, mut cx| async move {
            let context = context.await?;
            let assistant_panel = this.clone();
            this.update(&mut cx, |this, cx| {
                let editor = cx.new_view(|cx| {
                    ContextEditor::for_context(
                        context,
                        fs,
                        workspace,
                        project,
                        lsp_adapter_delegate,
                        assistant_panel,
                        cx,
                    )
                });
                this.show_context(editor, cx);
                anyhow::Ok(())
            })??;
            Ok(())
        })
    }

    fn open_remote_context(
        &mut self,
        id: ContextId,
        cx: &mut ViewContext<Self>,
    ) -> Task<Result<View<ContextEditor>>> {
        let existing_context = self.pane.read(cx).items().find_map(|item| {
            item.downcast::<ContextEditor>()
                .filter(|editor| *editor.read(cx).context.read(cx).id() == id)
        });
        if let Some(existing_context) = existing_context {
            return cx.spawn(|this, mut cx| async move {
                this.update(&mut cx, |this, cx| {
                    this.show_context(existing_context.clone(), cx)
                })?;
                Ok(existing_context)
            });
        }

        let context = self
            .context_store
            .update(cx, |store, cx| store.open_remote_context(id, cx));
        let fs = self.fs.clone();
        let workspace = self.workspace.clone();
        let lsp_adapter_delegate = make_lsp_adapter_delegate(&self.project, cx)
            .log_err()
            .flatten();

        cx.spawn(|this, mut cx| async move {
            let context = context.await?;
            let assistant_panel = this.clone();
            this.update(&mut cx, |this, cx| {
                let editor = cx.new_view(|cx| {
                    ContextEditor::for_context(
                        context,
                        fs,
                        workspace,
                        this.project.clone(),
                        lsp_adapter_delegate,
                        assistant_panel,
                        cx,
                    )
                });
                this.show_context(editor.clone(), cx);
                anyhow::Ok(editor)
            })?
        })
    }

    fn is_authenticated(&mut self, cx: &mut ViewContext<Self>) -> bool {
        LanguageModelRegistry::read_global(cx)
            .active_provider()
            .map_or(false, |provider| provider.is_authenticated(cx))
    }

    fn authenticate(&mut self, cx: &mut ViewContext<Self>) -> Option<Task<Result<()>>> {
        LanguageModelRegistry::read_global(cx)
            .active_provider()
            .map_or(None, |provider| Some(provider.authenticate(cx)))
    }
}

impl Render for AssistantPanel {
    fn render(&mut self, cx: &mut ViewContext<Self>) -> impl IntoElement {
        let mut registrar = DivRegistrar::new(
            |panel, cx| {
                panel
                    .pane
                    .read(cx)
                    .toolbar()
                    .read(cx)
                    .item_of_type::<BufferSearchBar>()
            },
            cx,
        );
        BufferSearchBar::register(&mut registrar);
        let registrar = registrar.into_div();

        v_flex()
            .key_context("AssistantPanel")
            .size_full()
            .on_action(cx.listener(|this, _: &NewContext, cx| {
                this.new_context(cx);
            }))
            .on_action(
                cx.listener(|this, _: &ShowConfiguration, cx| this.show_configuration_tab(cx)),
            )
            .on_action(cx.listener(AssistantPanel::deploy_history))
            .on_action(cx.listener(AssistantPanel::deploy_prompt_library))
            .on_action(cx.listener(AssistantPanel::toggle_model_selector))
            .child(registrar.size_full().child(self.pane.clone()))
            .into_any_element()
    }
}

impl Panel for AssistantPanel {
    fn persistent_name() -> &'static str {
        "AssistantPanel"
    }

    fn position(&self, cx: &WindowContext) -> DockPosition {
        match AssistantSettings::get_global(cx).dock {
            AssistantDockPosition::Left => DockPosition::Left,
            AssistantDockPosition::Bottom => DockPosition::Bottom,
            AssistantDockPosition::Right => DockPosition::Right,
        }
    }

    fn position_is_valid(&self, _: DockPosition) -> bool {
        true
    }

    fn set_position(&mut self, position: DockPosition, cx: &mut ViewContext<Self>) {
        settings::update_settings_file::<AssistantSettings>(
            self.fs.clone(),
            cx,
            move |settings, _| {
                let dock = match position {
                    DockPosition::Left => AssistantDockPosition::Left,
                    DockPosition::Bottom => AssistantDockPosition::Bottom,
                    DockPosition::Right => AssistantDockPosition::Right,
                };
                settings.set_dock(dock);
            },
        );
    }

    fn size(&self, cx: &WindowContext) -> Pixels {
        let settings = AssistantSettings::get_global(cx);
        match self.position(cx) {
            DockPosition::Left | DockPosition::Right => {
                self.width.unwrap_or(settings.default_width)
            }
            DockPosition::Bottom => self.height.unwrap_or(settings.default_height),
        }
    }

    fn set_size(&mut self, size: Option<Pixels>, cx: &mut ViewContext<Self>) {
        match self.position(cx) {
            DockPosition::Left | DockPosition::Right => self.width = size,
            DockPosition::Bottom => self.height = size,
        }
        cx.notify();
    }

    fn is_zoomed(&self, cx: &WindowContext) -> bool {
        self.pane.read(cx).is_zoomed()
    }

    fn set_zoomed(&mut self, zoomed: bool, cx: &mut ViewContext<Self>) {
        self.pane.update(cx, |pane, cx| pane.set_zoomed(zoomed, cx));
    }

    fn set_active(&mut self, active: bool, cx: &mut ViewContext<Self>) {
        if active {
            if self.pane.read(cx).items_len() == 0 {
                self.new_context(cx);
            }

            self.ensure_authenticated(cx);
        }
    }

    fn pane(&self) -> Option<View<Pane>> {
        Some(self.pane.clone())
    }

    fn remote_id() -> Option<proto::PanelId> {
        Some(proto::PanelId::AssistantPanel)
    }

    fn icon(&self, cx: &WindowContext) -> Option<IconName> {
        let settings = AssistantSettings::get_global(cx);
        if !settings.enabled || !settings.button {
            return None;
        }

        Some(IconName::ZedAssistant)
    }

    fn icon_tooltip(&self, _cx: &WindowContext) -> Option<&'static str> {
        Some("Assistant Panel")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }
}

impl EventEmitter<PanelEvent> for AssistantPanel {}
impl EventEmitter<AssistantPanelEvent> for AssistantPanel {}

impl FocusableView for AssistantPanel {
    fn focus_handle(&self, cx: &AppContext) -> FocusHandle {
        self.pane.focus_handle(cx)
    }
}

pub enum ContextEditorEvent {
    Edited,
    TabContentChanged,
}

#[derive(Copy, Clone, Debug, PartialEq)]
struct ScrollPosition {
    offset_before_cursor: gpui::Point<f32>,
    cursor: Anchor,
}

struct WorkflowStepViewState {
    header_block_id: CustomBlockId,
    header_crease_id: CreaseId,
    footer_block_id: Option<CustomBlockId>,
    footer_crease_id: Option<CreaseId>,
    assist: Option<WorkflowAssist>,
    resolution: Option<Arc<Result<WorkflowStepResolution>>>,
}

impl WorkflowStepViewState {
    fn status(&self, cx: &AppContext) -> WorkflowStepStatus {
        if let Some(assist) = &self.assist {
            match assist.status(cx) {
                WorkflowAssistStatus::Idle => WorkflowStepStatus::Idle,
                WorkflowAssistStatus::Pending => WorkflowStepStatus::Pending,
                WorkflowAssistStatus::Done => WorkflowStepStatus::Done,
                WorkflowAssistStatus::Confirmed => WorkflowStepStatus::Confirmed,
            }
        } else if let Some(resolution) = self.resolution.as_deref() {
            match resolution {
                Err(err) => WorkflowStepStatus::Error(err),
                Ok(_) => WorkflowStepStatus::Idle,
            }
        } else {
            WorkflowStepStatus::Resolving
        }
    }
}

#[derive(Clone, Copy)]
enum WorkflowStepStatus<'a> {
    Resolving,
    Error(&'a anyhow::Error),
    Idle,
    Pending,
    Done,
    Confirmed,
}

impl<'a> WorkflowStepStatus<'a> {
    pub(crate) fn is_confirmed(&self) -> bool {
        matches!(self, Self::Confirmed)
    }
}

#[derive(Debug, Eq, PartialEq)]
struct ActiveWorkflowStep {
    range: Range<language::Anchor>,
    resolved: bool,
}

struct WorkflowAssist {
    editor: WeakView<Editor>,
    editor_was_open: bool,
    assist_ids: Vec<InlineAssistId>,
}

type MessageHeader = MessageMetadata;

#[derive(Clone)]
enum AssistError {
    PaymentRequired,
    MaxMonthlySpendReached,
    Message(SharedString),
}

pub struct ContextEditor {
    context: Model<Context>,
    fs: Arc<dyn Fs>,
    workspace: WeakView<Workspace>,
    project: Model<Project>,
    lsp_adapter_delegate: Option<Arc<dyn LspAdapterDelegate>>,
    editor: View<Editor>,
    blocks: HashMap<MessageId, (MessageHeader, CustomBlockId)>,
    image_blocks: HashSet<CustomBlockId>,
    scroll_position: Option<ScrollPosition>,
    remote_id: Option<workspace::ViewId>,
    pending_slash_command_creases: HashMap<Range<language::Anchor>, CreaseId>,
    pending_slash_command_blocks: HashMap<Range<language::Anchor>, CustomBlockId>,
    pending_tool_use_creases: HashMap<Range<language::Anchor>, CreaseId>,
    _subscriptions: Vec<Subscription>,
    workflow_steps: HashMap<Range<language::Anchor>, WorkflowStepViewState>,
    active_workflow_step: Option<ActiveWorkflowStep>,
    assistant_panel: WeakView<AssistantPanel>,
    last_error: Option<AssistError>,
    show_accept_terms: bool,
    pub(crate) slash_menu_handle:
        PopoverMenuHandle<Picker<slash_command_picker::SlashCommandDelegate>>,
    // dragged_file_worktrees is used to keep references to worktrees that were added
    // when the user drag/dropped an external file onto the context editor. Since
    // the worktree is not part of the project panel, it would be dropped as soon as
    // the file is opened. In order to keep the worktree alive for the duration of the
    // context editor, we keep a reference here.
    dragged_file_worktrees: Vec<Model<Worktree>>,
}

const DEFAULT_TAB_TITLE: &str = "New Context";
const MAX_TAB_TITLE_LEN: usize = 16;

impl ContextEditor {
    fn for_context(
        context: Model<Context>,
        fs: Arc<dyn Fs>,
        workspace: WeakView<Workspace>,
        project: Model<Project>,
        lsp_adapter_delegate: Option<Arc<dyn LspAdapterDelegate>>,
        assistant_panel: WeakView<AssistantPanel>,
        cx: &mut ViewContext<Self>,
    ) -> Self {
        let completion_provider = SlashCommandCompletionProvider::new(
            Some(cx.view().downgrade()),
            Some(workspace.clone()),
        );

        let editor = cx.new_view(|cx| {
            let mut editor = Editor::for_buffer(context.read(cx).buffer().clone(), None, cx);
            editor.set_soft_wrap_mode(SoftWrap::EditorWidth, cx);
            editor.set_show_line_numbers(false, cx);
            editor.set_show_git_diff_gutter(false, cx);
            editor.set_show_code_actions(false, cx);
            editor.set_show_runnables(false, cx);
            editor.set_show_wrap_guides(false, cx);
            editor.set_show_indent_guides(false, cx);
            editor.set_completion_provider(Some(Box::new(completion_provider)));
            editor.set_collaboration_hub(Box::new(project.clone()));
            editor
        });

        let _subscriptions = vec![
            cx.observe(&context, |_, _, cx| cx.notify()),
            cx.subscribe(&context, Self::handle_context_event),
            cx.subscribe(&editor, Self::handle_editor_event),
            cx.subscribe(&editor, Self::handle_editor_search_event),
        ];

        let sections = context.read(cx).slash_command_output_sections().to_vec();
        let edit_step_ranges = context.read(cx).workflow_step_ranges().collect::<Vec<_>>();
        let mut this = Self {
            context,
            editor,
            lsp_adapter_delegate,
            blocks: Default::default(),
            image_blocks: Default::default(),
            scroll_position: None,
            remote_id: None,
            fs,
            workspace,
            project,
            pending_slash_command_creases: HashMap::default(),
            pending_slash_command_blocks: HashMap::default(),
            pending_tool_use_creases: HashMap::default(),
            _subscriptions,
            workflow_steps: HashMap::default(),
            active_workflow_step: None,
            assistant_panel,
            last_error: None,
            show_accept_terms: false,
            slash_menu_handle: Default::default(),
            dragged_file_worktrees: Vec::new(),
        };
        this.update_message_headers(cx);
        this.update_image_blocks(cx);
        this.insert_slash_command_output_sections(sections, false, cx);
        this.workflow_steps_updated(&Vec::new(), &edit_step_ranges, cx);
        this
    }

    fn insert_default_prompt(&mut self, cx: &mut ViewContext<Self>) {
        let command_name = DefaultSlashCommand.name();
        self.editor.update(cx, |editor, cx| {
            editor.insert(&format!("/{command_name}\n\n"), cx)
        });
        let command = self.context.update(cx, |context, cx| {
            context.reparse(cx);
            context.pending_slash_commands()[0].clone()
        });
        self.run_command(
            command.source_range,
            &command.name,
            &command.arguments,
            false,
            false,
            self.workspace.clone(),
            cx,
        );
    }

    fn assist(&mut self, _: &Assist, cx: &mut ViewContext<Self>) {
        let provider = LanguageModelRegistry::read_global(cx).active_provider();
        if provider
            .as_ref()
            .map_or(false, |provider| provider.must_accept_terms(cx))
        {
            self.show_accept_terms = true;
            cx.notify();
            return;
        }

        if !self.apply_active_workflow_step(cx) {
            self.last_error = None;
            self.send_to_model(cx);
            cx.notify();
        }
    }

    fn apply_workflow_step(&mut self, range: Range<language::Anchor>, cx: &mut ViewContext<Self>) {
        self.show_workflow_step(range.clone(), cx);

        if let Some(workflow_step) = self.workflow_steps.get(&range) {
            if let Some(assist) = workflow_step.assist.as_ref() {
                let assist_ids = assist.assist_ids.clone();
                cx.spawn(|this, mut cx| async move {
                    for assist_id in assist_ids {
                        let mut receiver = this.update(&mut cx, |_, cx| {
                            cx.window_context().defer(move |cx| {
                                InlineAssistant::update_global(cx, |assistant, cx| {
                                    assistant.start_assist(assist_id, cx);
                                })
                            });
                            InlineAssistant::update_global(cx, |assistant, _| {
                                assistant.observe_assist(assist_id)
                            })
                        })?;
                        while !receiver.borrow().is_done() {
                            let _ = receiver.changed().await;
                        }
                    }
                    anyhow::Ok(())
                })
                .detach_and_log_err(cx);
            }
        }
    }

    fn apply_active_workflow_step(&mut self, cx: &mut ViewContext<Self>) -> bool {
        let Some((range, step)) = self.active_workflow_step() else {
            return false;
        };

        if let Some(assist) = step.assist.as_ref() {
            match assist.status(cx) {
                WorkflowAssistStatus::Pending => {}
                WorkflowAssistStatus::Confirmed => return false,
                WorkflowAssistStatus::Done => self.confirm_workflow_step(range, cx),
                WorkflowAssistStatus::Idle => self.apply_workflow_step(range, cx),
            }
        } else {
            match step.resolution.as_deref() {
                Some(Ok(_)) => self.apply_workflow_step(range, cx),
                Some(Err(_)) => self.resolve_workflow_step(range, cx),
                None => {}
            }
        }

        true
    }

    fn resolve_workflow_step(
        &mut self,
        range: Range<language::Anchor>,
        cx: &mut ViewContext<Self>,
    ) {
        self.context
            .update(cx, |context, cx| context.resolve_workflow_step(range, cx));
    }

    fn stop_workflow_step(&mut self, range: Range<language::Anchor>, cx: &mut ViewContext<Self>) {
        if let Some(workflow_step) = self.workflow_steps.get(&range) {
            if let Some(assist) = workflow_step.assist.as_ref() {
                let assist_ids = assist.assist_ids.clone();
                cx.window_context().defer(move |cx| {
                    InlineAssistant::update_global(cx, |assistant, cx| {
                        for assist_id in assist_ids {
                            assistant.stop_assist(assist_id, cx);
                        }
                    })
                });
            }
        }
    }

    fn undo_workflow_step(&mut self, range: Range<language::Anchor>, cx: &mut ViewContext<Self>) {
        if let Some(workflow_step) = self.workflow_steps.get_mut(&range) {
            if let Some(assist) = workflow_step.assist.take() {
                cx.window_context().defer(move |cx| {
                    InlineAssistant::update_global(cx, |assistant, cx| {
                        for assist_id in assist.assist_ids {
                            assistant.undo_assist(assist_id, cx);
                        }
                    })
                });
            }
        }
    }

    fn confirm_workflow_step(
        &mut self,
        range: Range<language::Anchor>,
        cx: &mut ViewContext<Self>,
    ) {
        if let Some(workflow_step) = self.workflow_steps.get(range) {
            if let Some(assist) = workflow_step.assist.as_ref() {
                let assist_ids = assist.assist_ids.clone();
                cx.window_context().defer(move |cx| {
                    InlineAssistant::update_global(cx, |assistant, cx| {
                        for assist_id in assist_ids {
                            assistant.finish_assist(assist_id, false, cx);
                        }
                    })
                });
            }
        }
    }

    fn reject_workflow_step(&mut self, range: Range<language::Anchor>, cx: &mut ViewContext<Self>) {
        if let Some(workflow_step) = self.workflow_steps.get_mut(&range) {
            if let Some(assist) = workflow_step.assist.take() {
                cx.window_context().defer(move |cx| {
                    InlineAssistant::update_global(cx, |assistant, cx| {
                        for assist_id in assist.assist_ids {
                            assistant.finish_assist(assist_id, true, cx);
                        }
                    })
                });
            }
        }
    }

    fn send_to_model(&mut self, cx: &mut ViewContext<Self>) {
        if let Some(user_message) = self.context.update(cx, |context, cx| context.assist(cx)) {
            let new_selection = {
                let cursor = user_message
                    .start
                    .to_offset(self.context.read(cx).buffer().read(cx));
                cursor..cursor
            };
            self.editor.update(cx, |editor, cx| {
                editor.change_selections(
                    Some(Autoscroll::Strategy(AutoscrollStrategy::Fit)),
                    cx,
                    |selections| selections.select_ranges([new_selection]),
                );
            });
            // Avoid scrolling to the new cursor position so the assistant's output is stable.
            cx.defer(|this, _| this.scroll_position = None);
        }
    }

    fn cancel(&mut self, _: &editor::actions::Cancel, cx: &mut ViewContext<Self>) {
        self.last_error = None;

        if self
            .context
            .update(cx, |context, cx| context.cancel_last_assist(cx))
        {
            return;
        }

        if let Some((range, active_step)) = self.active_workflow_step() {
            match active_step.status(cx) {
                WorkflowStepStatus::Pending => {
                    self.stop_workflow_step(range, cx);
                    return;
                }
                WorkflowStepStatus::Done => {
                    self.reject_workflow_step(range, cx);
                    return;
                }
                _ => {}
            }
        }
        cx.propagate();
    }

    fn cycle_message_role(&mut self, _: &CycleMessageRole, cx: &mut ViewContext<Self>) {
        let cursors = self.cursors(cx);
        self.context.update(cx, |context, cx| {
            let messages = context
                .messages_for_offsets(cursors, cx)
                .into_iter()
                .map(|message| message.id)
                .collect();
            context.cycle_message_roles(messages, cx)
        });
    }

    fn cursors(&self, cx: &AppContext) -> Vec<usize> {
        let selections = self.editor.read(cx).selections.all::<usize>(cx);
        selections
            .into_iter()
            .map(|selection| selection.head())
            .collect()
    }

    pub fn insert_command(&mut self, name: &str, cx: &mut ViewContext<Self>) {
        if let Some(command) = SlashCommandRegistry::global(cx).command(name) {
            self.editor.update(cx, |editor, cx| {
                editor.transact(cx, |editor, cx| {
                    editor.change_selections(Some(Autoscroll::fit()), cx, |s| s.try_cancel());
                    let snapshot = editor.buffer().read(cx).snapshot(cx);
                    let newest_cursor = editor.selections.newest::<Point>(cx).head();
                    if newest_cursor.column > 0
                        || snapshot
                            .chars_at(newest_cursor)
                            .next()
                            .map_or(false, |ch| ch != '\n')
                    {
                        editor.move_to_end_of_line(
                            &MoveToEndOfLine {
                                stop_at_soft_wraps: false,
                            },
                            cx,
                        );
                        editor.newline(&Newline, cx);
                    }

                    editor.insert(&format!("/{name}"), cx);
                    if command.accepts_arguments() {
                        editor.insert(" ", cx);
                        editor.show_completions(&ShowCompletions::default(), cx);
                    }
                });
            });
            if !command.requires_argument() {
                self.confirm_command(&ConfirmCommand, cx);
            }
        }
    }

    pub fn confirm_command(&mut self, _: &ConfirmCommand, cx: &mut ViewContext<Self>) {
        if self.editor.read(cx).has_active_completions_menu() {
            return;
        }

        let selections = self.editor.read(cx).selections.disjoint_anchors();
        let mut commands_by_range = HashMap::default();
        let workspace = self.workspace.clone();
        self.context.update(cx, |context, cx| {
            context.reparse(cx);
            for selection in selections.iter() {
                if let Some(command) =
                    context.pending_command_for_position(selection.head().text_anchor, cx)
                {
                    commands_by_range
                        .entry(command.source_range.clone())
                        .or_insert_with(|| command.clone());
                }
            }
        });

        if commands_by_range.is_empty() {
            cx.propagate();
        } else {
            for command in commands_by_range.into_values() {
                self.run_command(
                    command.source_range,
                    &command.name,
                    &command.arguments,
                    true,
                    false,
                    workspace.clone(),
                    cx,
                );
            }
            cx.stop_propagation();
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn run_command(
        &mut self,
        command_range: Range<language::Anchor>,
        name: &str,
        arguments: &[String],
        ensure_trailing_newline: bool,
        expand_result: bool,
        workspace: WeakView<Workspace>,
        cx: &mut ViewContext<Self>,
    ) {
        if let Some(command) = SlashCommandRegistry::global(cx).command(name) {
            let context = self.context.read(cx);
            let sections = context
                .slash_command_output_sections()
                .into_iter()
                .filter(|section| section.is_valid(context.buffer().read(cx)))
                .cloned()
                .collect::<Vec<_>>();
            let snapshot = context.buffer().read(cx).snapshot();
            let output = command.run(
                arguments,
                &sections,
                snapshot,
                workspace,
                self.lsp_adapter_delegate.clone(),
                cx,
            );
            self.context.update(cx, |context, cx| {
                context.insert_command_output(
                    command_range,
                    output,
                    ensure_trailing_newline,
                    expand_result,
                    cx,
                )
            });
        }
    }

    fn handle_context_event(
        &mut self,
        _: Model<Context>,
        event: &ContextEvent,
        cx: &mut ViewContext<Self>,
    ) {
        let context_editor = cx.view().downgrade();

        match event {
            ContextEvent::MessagesEdited => {
                self.update_message_headers(cx);
                self.update_image_blocks(cx);
                self.context.update(cx, |context, cx| {
                    context.save(Some(Duration::from_millis(500)), self.fs.clone(), cx);
                });
            }
            ContextEvent::SummaryChanged => {
                cx.emit(EditorEvent::TitleChanged);
                self.context.update(cx, |context, cx| {
                    context.save(Some(Duration::from_millis(500)), self.fs.clone(), cx);
                });
            }
            ContextEvent::StreamedCompletion => {
                self.editor.update(cx, |editor, cx| {
                    if let Some(scroll_position) = self.scroll_position {
                        let snapshot = editor.snapshot(cx);
                        let cursor_point = scroll_position.cursor.to_display_point(&snapshot);
                        let scroll_top =
                            cursor_point.row().as_f32() - scroll_position.offset_before_cursor.y;
                        editor.set_scroll_position(
                            point(scroll_position.offset_before_cursor.x, scroll_top),
                            cx,
                        );
                    }

                    let new_tool_uses = self
                        .context
                        .read(cx)
                        .pending_tool_uses()
                        .into_iter()
                        .filter(|tool_use| {
                            !self
                                .pending_tool_use_creases
                                .contains_key(&tool_use.source_range)
                        })
                        .cloned()
                        .collect::<Vec<_>>();

                    let buffer = editor.buffer().read(cx).snapshot(cx);
                    let (excerpt_id, _buffer_id, _) = buffer.as_singleton().unwrap();
                    let excerpt_id = *excerpt_id;

                    let mut buffer_rows_to_fold = BTreeSet::new();

                    let creases = new_tool_uses
                        .iter()
                        .map(|tool_use| {
                            let placeholder = FoldPlaceholder {
                                render: render_fold_icon_button(
                                    cx.view().downgrade(),
                                    IconName::PocketKnife,
                                    tool_use.name.clone().into(),
                                ),
                                constrain_width: false,
                                merge_adjacent: false,
                            };
                            let render_trailer =
                                move |_row, _unfold, _cx: &mut WindowContext| Empty.into_any();

                            let start = buffer
                                .anchor_in_excerpt(excerpt_id, tool_use.source_range.start)
                                .unwrap();
                            let end = buffer
                                .anchor_in_excerpt(excerpt_id, tool_use.source_range.end)
                                .unwrap();

                            let buffer_row = MultiBufferRow(start.to_point(&buffer).row);
                            buffer_rows_to_fold.insert(buffer_row);

                            self.context.update(cx, |context, cx| {
                                context.insert_content(
                                    Content::ToolUse {
                                        range: tool_use.source_range.clone(),
                                        tool_use: LanguageModelToolUse {
                                            id: tool_use.id.to_string(),
                                            name: tool_use.name.clone(),
                                            input: tool_use.input.clone(),
                                        },
                                    },
                                    cx,
                                );
                            });

                            Crease::new(
                                start..end,
                                placeholder,
                                fold_toggle("tool-use"),
                                render_trailer,
                            )
                        })
                        .collect::<Vec<_>>();

                    let crease_ids = editor.insert_creases(creases, cx);

                    for buffer_row in buffer_rows_to_fold.into_iter().rev() {
                        editor.fold_at(&FoldAt { buffer_row }, cx);
                    }

                    self.pending_tool_use_creases.extend(
                        new_tool_uses
                            .iter()
                            .map(|tool_use| tool_use.source_range.clone())
                            .zip(crease_ids),
                    );
                });
            }
            ContextEvent::WorkflowStepsUpdated { removed, updated } => {
                self.workflow_steps_updated(removed, updated, cx);
            }
            ContextEvent::PendingSlashCommandsUpdated { removed, updated } => {
                self.editor.update(cx, |editor, cx| {
                    let buffer = editor.buffer().read(cx).snapshot(cx);
                    let (excerpt_id, buffer_id, _) = buffer.as_singleton().unwrap();
                    let excerpt_id = *excerpt_id;

                    editor.remove_creases(
                        removed
                            .iter()
                            .filter_map(|range| self.pending_slash_command_creases.remove(range)),
                        cx,
                    );

                    editor.remove_blocks(
                        HashSet::from_iter(
                            removed.iter().filter_map(|range| {
                                self.pending_slash_command_blocks.remove(range)
                            }),
                        ),
                        None,
                        cx,
                    );

                    let crease_ids = editor.insert_creases(
                        updated.iter().map(|command| {
                            let workspace = self.workspace.clone();
                            let confirm_command = Arc::new({
                                let context_editor = context_editor.clone();
                                let command = command.clone();
                                move |cx: &mut WindowContext| {
                                    context_editor
                                        .update(cx, |context_editor, cx| {
                                            context_editor.run_command(
                                                command.source_range.clone(),
                                                &command.name,
                                                &command.arguments,
                                                false,
                                                false,
                                                workspace.clone(),
                                                cx,
                                            );
                                        })
                                        .ok();
                                }
                            });
                            let placeholder = FoldPlaceholder {
                                render: Arc::new(move |_, _, _| Empty.into_any()),
                                constrain_width: false,
                                merge_adjacent: false,
                            };
                            let render_toggle = {
                                let confirm_command = confirm_command.clone();
                                let command = command.clone();
                                move |row, _, _, _cx: &mut WindowContext| {
                                    render_pending_slash_command_gutter_decoration(
                                        row,
                                        &command.status,
                                        confirm_command.clone(),
                                    )
                                }
                            };
                            let render_trailer = {
                                let command = command.clone();
                                move |row, _unfold, cx: &mut WindowContext| {
                                    // TODO: In the future we should investigate how we can expose
                                    // this as a hook on the `SlashCommand` trait so that we don't
                                    // need to special-case it here.
                                    if command.name == DocsSlashCommand::NAME {
                                        return render_docs_slash_command_trailer(
                                            row,
                                            command.clone(),
                                            cx,
                                        );
                                    }

                                    Empty.into_any()
                                }
                            };

                            let start = buffer
                                .anchor_in_excerpt(excerpt_id, command.source_range.start)
                                .unwrap();
                            let end = buffer
                                .anchor_in_excerpt(excerpt_id, command.source_range.end)
                                .unwrap();
                            Crease::new(start..end, placeholder, render_toggle, render_trailer)
                        }),
                        cx,
                    );

                    let block_ids = editor.insert_blocks(
                        updated
                            .iter()
                            .filter_map(|command| match &command.status {
                                PendingSlashCommandStatus::Error(error) => {
                                    Some((command, error.clone()))
                                }
                                _ => None,
                            })
                            .map(|(command, error_message)| BlockProperties {
                                style: BlockStyle::Fixed,
                                position: Anchor {
                                    buffer_id: Some(buffer_id),
                                    excerpt_id,
                                    text_anchor: command.source_range.start,
                                },
                                height: 1,
                                disposition: BlockDisposition::Below,
                                render: slash_command_error_block_renderer(error_message),
                                priority: 0,
                            }),
                        None,
                        cx,
                    );

                    self.pending_slash_command_creases.extend(
                        updated
                            .iter()
                            .map(|command| command.source_range.clone())
                            .zip(crease_ids),
                    );

                    self.pending_slash_command_blocks.extend(
                        updated
                            .iter()
                            .map(|command| command.source_range.clone())
                            .zip(block_ids),
                    );
                })
            }
            ContextEvent::SlashCommandFinished {
                output_range,
                sections,
                run_commands_in_output,
                expand_result,
            } => {
                self.insert_slash_command_output_sections(
                    sections.iter().cloned(),
                    *expand_result,
                    cx,
                );

                if *run_commands_in_output {
                    let commands = self.context.update(cx, |context, cx| {
                        context.reparse(cx);
                        context
                            .pending_commands_for_range(output_range.clone(), cx)
                            .to_vec()
                    });

                    for command in commands {
                        self.run_command(
                            command.source_range,
                            &command.name,
                            &command.arguments,
                            false,
                            false,
                            self.workspace.clone(),
                            cx,
                        );
                    }
                }
            }
            ContextEvent::UsePendingTools => {
                let pending_tool_uses = self
                    .context
                    .read(cx)
                    .pending_tool_uses()
                    .into_iter()
                    .filter(|tool_use| tool_use.status.is_idle())
                    .cloned()
                    .collect::<Vec<_>>();

                for tool_use in pending_tool_uses {
                    let tool_registry = ToolRegistry::global(cx);
                    if let Some(tool) = tool_registry.tool(&tool_use.name) {
                        let task = tool.run(tool_use.input, self.workspace.clone(), cx);

                        self.context.update(cx, |context, cx| {
                            context.insert_tool_output(tool_use.id.clone(), task, cx);
                        });
                    }
                }
            }
            ContextEvent::ToolFinished {
                tool_use_id,
                output_range,
            } => {
                self.editor.update(cx, |editor, cx| {
                    let buffer = editor.buffer().read(cx).snapshot(cx);
                    let (excerpt_id, _buffer_id, _) = buffer.as_singleton().unwrap();
                    let excerpt_id = *excerpt_id;

                    let placeholder = FoldPlaceholder {
                        render: render_fold_icon_button(
                            cx.view().downgrade(),
                            IconName::PocketKnife,
                            format!("Tool Result: {tool_use_id}").into(),
                        ),
                        constrain_width: false,
                        merge_adjacent: false,
                    };
                    let render_trailer =
                        move |_row, _unfold, _cx: &mut WindowContext| Empty.into_any();

                    let start = buffer
                        .anchor_in_excerpt(excerpt_id, output_range.start)
                        .unwrap();
                    let end = buffer
                        .anchor_in_excerpt(excerpt_id, output_range.end)
                        .unwrap();

                    let buffer_row = MultiBufferRow(start.to_point(&buffer).row);

                    let crease = Crease::new(
                        start..end,
                        placeholder,
                        fold_toggle("tool-use"),
                        render_trailer,
                    );

                    editor.insert_creases(vec![crease], cx);
                    editor.fold_at(&FoldAt { buffer_row }, cx);
                });
            }
            ContextEvent::Operation(_) => {}
            ContextEvent::ShowAssistError(error_message) => {
                self.last_error = Some(AssistError::Message(error_message.clone()));
            }
            ContextEvent::ShowPaymentRequiredError => {
                self.last_error = Some(AssistError::PaymentRequired);
            }
            ContextEvent::ShowMaxMonthlySpendReachedError => {
                self.last_error = Some(AssistError::MaxMonthlySpendReached);
            }
        }
    }

    fn workflow_steps_updated(
        &mut self,
        removed: &Vec<Range<text::Anchor>>,
        updated: &Vec<Range<text::Anchor>>,
        cx: &mut ViewContext<ContextEditor>,
    ) {
        let this = cx.view().downgrade();
        let mut removed_crease_ids = Vec::new();
        let mut removed_block_ids = HashSet::default();
        let mut editors_to_close = Vec::new();
        for range in removed {
            if let Some(state) = self.workflow_steps.remove(range) {
                editors_to_close.extend(self.hide_workflow_step(range.clone(), cx));
                removed_block_ids.insert(state.header_block_id);
                removed_crease_ids.push(state.header_crease_id);
                removed_block_ids.extend(state.footer_block_id);
                removed_crease_ids.extend(state.footer_crease_id);
            }
        }

        for range in updated {
            editors_to_close.extend(self.hide_workflow_step(range.clone(), cx));
        }

        self.editor.update(cx, |editor, cx| {
            let snapshot = editor.snapshot(cx);
            let multibuffer = &snapshot.buffer_snapshot;
            let (&excerpt_id, _, buffer) = multibuffer.as_singleton().unwrap();

            for range in updated {
                let Some(step) = self.context.read(cx).workflow_step_for_range(&range, cx) else {
                    continue;
                };

                let resolution = step.resolution.clone();
                let header_start = step.range.start;
                let header_end = if buffer.contains_str_at(step.leading_tags_end, "\n") {
                    buffer.anchor_before(step.leading_tags_end.to_offset(&buffer) + 1)
                } else {
                    step.leading_tags_end
                };
                let header_range = multibuffer
                    .anchor_in_excerpt(excerpt_id, header_start)
                    .unwrap()
                    ..multibuffer
                        .anchor_in_excerpt(excerpt_id, header_end)
                        .unwrap();
                let footer_range = step.trailing_tag_start.map(|start| {
                    let mut step_range_end = step.range.end.to_offset(&buffer);
                    if buffer.contains_str_at(step_range_end, "\n") {
                        // Only include the newline if it belongs to the same message.
                        let messages = self
                            .context
                            .read(cx)
                            .messages_for_offsets([step_range_end, step_range_end + 1], cx);
                        if messages.len() == 1 {
                            step_range_end += 1;
                        }
                    }

                    let end = buffer.anchor_before(step_range_end);
                    multibuffer.anchor_in_excerpt(excerpt_id, start).unwrap()
                        ..multibuffer.anchor_in_excerpt(excerpt_id, end).unwrap()
                });

                let block_ids = editor.insert_blocks(
                    [BlockProperties {
                        position: header_range.start,
                        height: 1,
                        style: BlockStyle::Flex,
                        render: Box::new({
                            let this = this.clone();
                            let range = step.range.clone();
                            move |cx| {
                                let block_id = cx.block_id;
                                let max_width = cx.max_width;
                                let gutter_width = cx.gutter_dimensions.full_width();
                                this.update(&mut **cx, |this, cx| {
                                    this.render_workflow_step_header(
                                        range.clone(),
                                        max_width,
                                        gutter_width,
                                        block_id,
                                        cx,
                                    )
                                })
                                .ok()
                                .flatten()
                                .unwrap_or_else(|| Empty.into_any())
                            }
                        }),
                        disposition: BlockDisposition::Above,
                        priority: 0,
                    }]
                    .into_iter()
                    .chain(footer_range.as_ref().map(|footer_range| {
                        return BlockProperties {
                            position: footer_range.end,
                            height: 1,
                            style: BlockStyle::Flex,
                            render: Box::new({
                                let this = this.clone();
                                let range = step.range.clone();
                                move |cx| {
                                    let max_width = cx.max_width;
                                    let gutter_width = cx.gutter_dimensions.full_width();
                                    this.update(&mut **cx, |this, cx| {
                                        this.render_workflow_step_footer(
                                            range.clone(),
                                            max_width,
                                            gutter_width,
                                            cx,
                                        )
                                    })
                                    .ok()
                                    .flatten()
                                    .unwrap_or_else(|| Empty.into_any())
                                }
                            }),
                            disposition: BlockDisposition::Below,
                            priority: 0,
                        };
                    })),
                    None,
                    cx,
                );

                let header_placeholder = FoldPlaceholder {
                    render: Arc::new(move |_, _, _| Empty.into_any()),
                    constrain_width: false,
                    merge_adjacent: false,
                };
                let footer_placeholder = FoldPlaceholder {
                    render: render_fold_icon_button(
                        cx.view().downgrade(),
                        IconName::Code,
                        "Edits".into(),
                    ),
                    constrain_width: false,
                    merge_adjacent: false,
                };

                let new_crease_ids = editor.insert_creases(
                    [Crease::new(
                        header_range.clone(),
                        header_placeholder.clone(),
                        fold_toggle("step-header"),
                        |_, _, _| Empty.into_any_element(),
                    )]
                    .into_iter()
                    .chain(footer_range.clone().map(|footer_range| {
                        Crease::new(
                            footer_range,
                            footer_placeholder.clone(),
                            |row, is_folded, fold, _cx: &mut WindowContext| {
                                if is_folded {
                                    Empty.into_any_element()
                                } else {
                                    fold_toggle("step-footer")(row, is_folded, fold, _cx)
                                }
                            },
                            |_, _, _| Empty.into_any_element(),
                        )
                    })),
                    cx,
                );

                let state = WorkflowStepViewState {
                    header_block_id: block_ids[0],
                    header_crease_id: new_crease_ids[0],
                    footer_block_id: block_ids.get(1).copied(),
                    footer_crease_id: new_crease_ids.get(1).copied(),
                    resolution,
                    assist: None,
                };

                let mut folds_to_insert = [(header_range.clone(), header_placeholder)]
                    .into_iter()
                    .chain(
                        footer_range
                            .clone()
                            .map(|range| (range, footer_placeholder)),
                    )
                    .collect::<Vec<_>>();

                match self.workflow_steps.entry(range.clone()) {
                    hash_map::Entry::Vacant(entry) => {
                        entry.insert(state);
                    }
                    hash_map::Entry::Occupied(mut entry) => {
                        let entry = entry.get_mut();
                        removed_block_ids.insert(entry.header_block_id);
                        removed_crease_ids.push(entry.header_crease_id);
                        removed_block_ids.extend(entry.footer_block_id);
                        removed_crease_ids.extend(entry.footer_crease_id);
                        folds_to_insert.retain(|(range, _)| snapshot.intersects_fold(range.start));
                        *entry = state;
                    }
                }

                editor.unfold_ranges(
                    [header_range.clone()]
                        .into_iter()
                        .chain(footer_range.clone()),
                    true,
                    false,
                    cx,
                );

                if !folds_to_insert.is_empty() {
                    editor.fold_ranges(folds_to_insert, false, cx);
                }
            }

            editor.remove_creases(removed_crease_ids, cx);
            editor.remove_blocks(removed_block_ids, None, cx);
        });

        for (editor, editor_was_open) in editors_to_close {
            self.close_workflow_editor(cx, editor, editor_was_open);
        }

        self.update_active_workflow_step(cx);
    }

    fn insert_slash_command_output_sections(
        &mut self,
        sections: impl IntoIterator<Item = SlashCommandOutputSection<language::Anchor>>,
        expand_result: bool,
        cx: &mut ViewContext<Self>,
    ) {
        self.editor.update(cx, |editor, cx| {
            let buffer = editor.buffer().read(cx).snapshot(cx);
            let excerpt_id = *buffer.as_singleton().unwrap().0;
            let mut buffer_rows_to_fold = BTreeSet::new();
            let mut creases = Vec::new();
            for section in sections {
                let start = buffer
                    .anchor_in_excerpt(excerpt_id, section.range.start)
                    .unwrap();
                let end = buffer
                    .anchor_in_excerpt(excerpt_id, section.range.end)
                    .unwrap();
                let buffer_row = MultiBufferRow(start.to_point(&buffer).row);
                buffer_rows_to_fold.insert(buffer_row);
                creases.push(
                    Crease::new(
                        start..end,
                        FoldPlaceholder {
                            render: render_fold_icon_button(
                                cx.view().downgrade(),
                                IconName::PocketKnife,
                                section.name.clone().into(),
                            ),
                            constrain_width: false,
                            merge_adjacent: false,
                        },
                        render_slash_command_output_toggle,
                        |_, _, _| Empty.into_any(),
                    )
                    .with_metadata(CreaseMetadata {
                        icon: section.icon,
                        label: section.label,
                    }),
                );
            }

            editor.insert_creases(creases, cx);

            if expand_result {
                buffer_rows_to_fold.clear();
            }
            for buffer_row in buffer_rows_to_fold.into_iter().rev() {
                editor.fold_at(&FoldAt { buffer_row }, cx);
            }
        });
    }

    fn handle_editor_event(
        &mut self,
        _: View<Editor>,
        event: &EditorEvent,
        cx: &mut ViewContext<Self>,
    ) {
        match event {
            EditorEvent::ScrollPositionChanged { autoscroll, .. } => {
                let cursor_scroll_position = self.cursor_scroll_position(cx);
                if *autoscroll {
                    self.scroll_position = cursor_scroll_position;
                } else if self.scroll_position != cursor_scroll_position {
                    self.scroll_position = None;
                }
            }
            EditorEvent::SelectionsChanged { .. } => {
                self.scroll_position = self.cursor_scroll_position(cx);
                self.update_active_workflow_step(cx);
            }
            _ => {}
        }
        cx.emit(event.clone());
    }

    fn active_workflow_step(&self) -> Option<(Range<text::Anchor>, &WorkflowStepViewState)> {
        let step = self.active_workflow_step.as_ref()?;
        Some((step.range.clone(), self.workflow_steps.get(&step.range)?))
    }

    fn update_active_workflow_step(&mut self, cx: &mut ViewContext<Self>) {
        let newest_cursor = self.editor.read(cx).selections.newest::<usize>(cx).head();
        let context = self.context.read(cx);

        let new_step = context
            .workflow_step_containing(newest_cursor, cx)
            .map(|step| ActiveWorkflowStep {
                resolved: step.resolution.is_some(),
                range: step.range.clone(),
            });

        if new_step.as_ref() != self.active_workflow_step.as_ref() {
            let mut old_editor = None;
            let mut old_editor_was_open = None;
            if let Some(old_step) = self.active_workflow_step.take() {
                (old_editor, old_editor_was_open) =
                    self.hide_workflow_step(old_step.range, cx).unzip();
            }

            let mut new_editor = None;
            if let Some(new_step) = new_step {
                new_editor = self.show_workflow_step(new_step.range.clone(), cx);
                self.active_workflow_step = Some(new_step);
            }

            if new_editor != old_editor {
                if let Some((old_editor, old_editor_was_open)) = old_editor.zip(old_editor_was_open)
                {
                    self.close_workflow_editor(cx, old_editor, old_editor_was_open)
                }
            }
        }
    }

    fn hide_workflow_step(
        &mut self,
        step_range: Range<language::Anchor>,
        cx: &mut ViewContext<Self>,
    ) -> Option<(View<Editor>, bool)> {
        if let Some(step) = self.workflow_steps.get_mut(&step_range) {
            let assist = step.assist.as_ref()?;
            let editor = assist.editor.upgrade()?;

            if matches!(step.status(cx), WorkflowStepStatus::Idle) {
                let assist = step.assist.take().unwrap();
                InlineAssistant::update_global(cx, |assistant, cx| {
                    for assist_id in assist.assist_ids {
                        assistant.finish_assist(assist_id, true, cx)
                    }
                });
                return Some((editor, assist.editor_was_open));
            }
        }

        None
    }

    fn close_workflow_editor(
        &mut self,
        cx: &mut ViewContext<ContextEditor>,
        editor: View<Editor>,
        editor_was_open: bool,
    ) {
        self.workspace
            .update(cx, |workspace, cx| {
                if let Some(pane) = workspace.pane_for(&editor) {
                    pane.update(cx, |pane, cx| {
                        let item_id = editor.entity_id();
                        if !editor_was_open && !editor.read(cx).is_focused(cx) {
                            pane.close_item_by_id(item_id, SaveIntent::Skip, cx)
                                .detach_and_log_err(cx);
                        }
                    });
                }
            })
            .ok();
    }

    fn show_workflow_step(
        &mut self,
        step_range: Range<language::Anchor>,
        cx: &mut ViewContext<Self>,
    ) -> Option<View<Editor>> {
        let step = self.workflow_steps.get_mut(&step_range)?;

        let mut editor_to_return = None;
        let mut scroll_to_assist_id = None;
        match step.status(cx) {
            WorkflowStepStatus::Idle => {
                if let Some(assist) = step.assist.as_ref() {
                    scroll_to_assist_id = assist.assist_ids.first().copied();
                } else if let Some(Ok(resolved)) = step.resolution.clone().as_deref() {
                    step.assist = Self::open_assists_for_step(
                        &resolved,
                        &self.project,
                        &self.assistant_panel,
                        &self.workspace,
                        cx,
                    );
                    editor_to_return = step
                        .assist
                        .as_ref()
                        .and_then(|assist| assist.editor.upgrade());
                }
            }
            WorkflowStepStatus::Pending => {
                if let Some(assist) = step.assist.as_ref() {
                    let assistant = InlineAssistant::global(cx);
                    scroll_to_assist_id = assist
                        .assist_ids
                        .iter()
                        .copied()
                        .find(|assist_id| assistant.assist_status(*assist_id, cx).is_pending());
                }
            }
            WorkflowStepStatus::Done => {
                if let Some(assist) = step.assist.as_ref() {
                    scroll_to_assist_id = assist.assist_ids.first().copied();
                }
            }
            _ => {}
        }

        if let Some(assist_id) = scroll_to_assist_id {
            if let Some(assist_editor) = step
                .assist
                .as_ref()
                .and_then(|assists| assists.editor.upgrade())
            {
                editor_to_return = Some(assist_editor.clone());
                self.workspace
                    .update(cx, |workspace, cx| {
                        workspace.activate_item(&assist_editor, false, false, cx)
                    })
                    .ok();
                InlineAssistant::update_global(cx, |assistant, cx| {
                    assistant.scroll_to_assist(assist_id, cx)
                });
            }
        }

        editor_to_return
    }

    fn open_assists_for_step(
        resolved_step: &WorkflowStepResolution,
        project: &Model<Project>,
        assistant_panel: &WeakView<AssistantPanel>,
        workspace: &WeakView<Workspace>,
        cx: &mut ViewContext<Self>,
    ) -> Option<WorkflowAssist> {
        let assistant_panel = assistant_panel.upgrade()?;
        if resolved_step.suggestion_groups.is_empty() {
            return None;
        }

        let editor;
        let mut editor_was_open = false;
        let mut suggestion_groups = Vec::new();
        if resolved_step.suggestion_groups.len() == 1
            && resolved_step
                .suggestion_groups
                .values()
                .next()
                .unwrap()
                .len()
                == 1
        {
            // If there's only one buffer and one suggestion group, open it directly
            let (buffer, groups) = resolved_step.suggestion_groups.iter().next().unwrap();
            let group = groups.into_iter().next().unwrap();
            editor = workspace
                .update(cx, |workspace, cx| {
                    let active_pane = workspace.active_pane().clone();
                    editor_was_open =
                        workspace.is_project_item_open::<Editor>(&active_pane, buffer, cx);
                    workspace.open_project_item::<Editor>(
                        active_pane,
                        buffer.clone(),
                        false,
                        false,
                        cx,
                    )
                })
                .log_err()?;
            let (&excerpt_id, _, _) = editor
                .read(cx)
                .buffer()
                .read(cx)
                .as_singleton()
                .unwrap();

            // Scroll the editor to the suggested assist
            editor.update(cx, |editor, cx| {
                let multibuffer = editor.buffer().read(cx).snapshot(cx);
                let (&excerpt_id, _, buffer) = multibuffer.as_singleton().unwrap();
                let anchor = if group.context_range.start.to_offset(buffer) == 0 {
                    Anchor::min()
                } else {
                    multibuffer
                        .anchor_in_excerpt(excerpt_id, group.context_range.start)
                        .unwrap()
                };

                editor.set_scroll_anchor(
                    ScrollAnchor {
                        offset: gpui::Point::default(),
                        anchor,
                    },
                    cx,
                );
            });

            suggestion_groups.push((excerpt_id, group));
        } else {
            // If there are multiple buffers or suggestion groups, create a multibuffer
            let multibuffer = cx.new_model(|cx| {
                let mut multibuffer =
                    MultiBuffer::new(Capability::ReadWrite).with_title(resolved_step.title.clone());
                for (buffer, groups) in &resolved_step.suggestion_groups {
                    let excerpt_ids = multibuffer.push_excerpts(
                        buffer.clone(),
                        groups.iter().map(|suggestion_group| ExcerptRange {
                            context: suggestion_group.context_range.clone(),
                            primary: None,
                        }),
                        cx,
                    );
                    suggestion_groups.extend(excerpt_ids.into_iter().zip(groups));
                }
                multibuffer
            });

            editor = cx.new_view(|cx| {
                Editor::for_multibuffer(multibuffer, Some(project.clone()), true, cx)
            });
            workspace
                .update(cx, |workspace, cx| {
                    workspace.add_item_to_active_pane(Box::new(editor.clone()), None, false, cx)
                })
                .log_err()?;
        }

        let mut assist_ids = Vec::new();
        for (excerpt_id, suggestion_group) in suggestion_groups {
            for suggestion in &suggestion_group.suggestions {
                assist_ids.extend(suggestion.show(
                    &editor,
                    excerpt_id,
                    workspace,
                    &assistant_panel,
                    cx,
                ));
            }
        }

        Some(WorkflowAssist {
            assist_ids,
            editor: editor.downgrade(),
            editor_was_open,
        })
    }

    fn handle_editor_search_event(
        &mut self,
        _: View<Editor>,
        event: &SearchEvent,
        cx: &mut ViewContext<Self>,
    ) {
        cx.emit(event.clone());
    }

    fn cursor_scroll_position(&self, cx: &mut ViewContext<Self>) -> Option<ScrollPosition> {
        self.editor.update(cx, |editor, cx| {
            let snapshot = editor.snapshot(cx);
            let cursor = editor.selections.newest_anchor().head();
            let cursor_row = cursor
                .to_display_point(&snapshot.display_snapshot)
                .row()
                .as_f32();
            let scroll_position = editor
                .scroll_manager
                .anchor()
                .scroll_position(&snapshot.display_snapshot);

            let scroll_bottom = scroll_position.y + editor.visible_line_count().unwrap_or(0.);
            if (scroll_position.y..scroll_bottom).contains(&cursor_row) {
                Some(ScrollPosition {
                    cursor,
                    offset_before_cursor: point(scroll_position.x, cursor_row - scroll_position.y),
                })
            } else {
                None
            }
        })
    }

    fn update_message_headers(&mut self, cx: &mut ViewContext<Self>) {
        self.editor.update(cx, |editor, cx| {
            let buffer = editor.buffer().read(cx).snapshot(cx);

            let excerpt_id = *buffer.as_singleton().unwrap().0;
            let mut old_blocks = std::mem::take(&mut self.blocks);
            let mut blocks_to_remove: HashMap<_, _> = old_blocks
                .iter()
                .map(|(message_id, (_, block_id))| (*message_id, *block_id))
                .collect();
            let mut blocks_to_replace: HashMap<_, RenderBlock> = Default::default();

            let render_block = |message: MessageMetadata| -> RenderBlock {
                Box::new({
                    let context = self.context.clone();
                    move |cx| {
                        let message_id = MessageId(message.timestamp);
                        let show_spinner = message.role == Role::Assistant
                            && message.status == MessageStatus::Pending;

                        let label = match message.role {
                            Role::User => {
                                Label::new("You").color(Color::Default).into_any_element()
                            }
                            Role::Assistant => {
                                let label = Label::new("Assistant").color(Color::Info);
                                if show_spinner {
                                    label
                                        .with_animation(
                                            "pulsating-label",
                                            Animation::new(Duration::from_secs(2))
                                                .repeat()
                                                .with_easing(pulsating_between(0.4, 0.8)),
                                            |label, delta| label.alpha(delta),
                                        )
                                        .into_any_element()
                                } else {
                                    label.into_any_element()
                                }
                            }

                            Role::System => Label::new("System")
                                .color(Color::Warning)
                                .into_any_element(),
                        };

                        let sender = ButtonLike::new("role")
                            .style(ButtonStyle::Filled)
                            .child(label)
                            .tooltip(|cx| {
                                Tooltip::with_meta(
                                    "Toggle message role",
                                    None,
                                    "Available roles: You (User), Assistant, System",
                                    cx,
                                )
                            })
                            .on_click({
                                let context = context.clone();
                                move |_, cx| {
                                    context.update(cx, |context, cx| {
                                        context.cycle_message_roles(
                                            HashSet::from_iter(Some(message_id)),
                                            cx,
                                        )
                                    })
                                }
                            });

                        h_flex()
                            .id(("message_header", message_id.as_u64()))
                            .pl(cx.gutter_dimensions.full_width())
                            .h_11()
                            .w_full()
                            .relative()
                            .gap_1()
                            .child(sender)
                            .children(match &message.cache {
                                Some(cache) if cache.is_final_anchor => match cache.status {
                                    CacheStatus::Cached => Some(
                                        div()
                                            .id("cached")
                                            .child(
                                                Icon::new(IconName::DatabaseZap)
                                                    .size(IconSize::XSmall)
                                                    .color(Color::Hint),
                                            )
                                            .tooltip(|cx| {
                                                Tooltip::with_meta(
                                                    "Context cached",
                                                    None,
                                                    "Large messages cached to optimize performance",
                                                    cx,
                                                )
                                            })
                                            .into_any_element(),
                                    ),
                                    CacheStatus::Pending => Some(
                                        div()
                                            .child(
                                                Icon::new(IconName::Ellipsis)
                                                    .size(IconSize::XSmall)
                                                    .color(Color::Hint),
                                            )
                                            .into_any_element(),
                                    ),
                                },
                                _ => None,
                            })
                            .into_any_element()
                    }
                })
            };
            let create_block_properties = |message: &Message| BlockProperties {
                position: buffer
                    .anchor_in_excerpt(excerpt_id, message.anchor_range.start)
                    .unwrap(),
                height: 2,
                style: BlockStyle::Sticky,
                disposition: BlockDisposition::Above,
                priority: usize::MAX,
                render: render_block(MessageMetadata::from(message)),
            };
            let mut new_blocks = vec![];
            let mut block_index_to_message = vec![];
            for message in self.context.read(cx).messages(cx) {
                if let Some(_) = blocks_to_remove.remove(&message.id) {
                    // This is an old message that we might modify.
                    let Some((meta, block_id)) = old_blocks.get_mut(&message.id) else {
                        debug_assert!(
                            false,
                            "old_blocks should contain a message_id we've just removed."
                        );
                        continue;
                    };
                    // Should we modify it?
                    let message_meta = MessageMetadata::from(&message);
                    if meta != &message_meta {
                        blocks_to_replace.insert(*block_id, render_block(message_meta.clone()));
                        *meta = message_meta;
                    }
                } else {
                    // This is a new message.
                    new_blocks.push(create_block_properties(&message));
                    block_index_to_message.push((message.id, MessageMetadata::from(&message)));
                }
            }
            editor.replace_blocks(blocks_to_replace, None, cx);
            editor.remove_blocks(blocks_to_remove.into_values().collect(), None, cx);

            let ids = editor.insert_blocks(new_blocks, None, cx);
            old_blocks.extend(ids.into_iter().zip(block_index_to_message).map(
                |(block_id, (message_id, message_meta))| (message_id, (message_meta, block_id)),
            ));
            self.blocks = old_blocks;
        });
    }

    /// Returns either the selected text, or the content of the Markdown code
    /// block surrounding the cursor.
    fn get_selection_or_code_block(
        context_editor_view: &View<ContextEditor>,
        cx: &mut ViewContext<Workspace>,
    ) -> Option<(String, bool)> {
        const CODE_FENCE_DELIMITER: &'static str = "```";

        let context_editor = context_editor_view.read(cx).editor.read(cx);

        if context_editor.selections.newest::<Point>(cx).is_empty() {
            let snapshot = context_editor.buffer().read(cx).snapshot(cx);
            let head = context_editor.selections.newest::<Point>(cx).head();
            let offset = snapshot.point_to_offset(head);

            let surrounding_code_block_range = find_surrounding_code_block(snapshot, offset)?;
            let mut text = snapshot
                .text_for_range(surrounding_code_block_range)
                .collect::<String>();

            // If there is no newline trailing the closing three-backticks, then
            // tree-sitter-md extends the range of the content node to include
            // the backticks.
            if text.ends_with(CODE_FENCE_DELIMITER) {
                text.drain((text.len() - CODE_FENCE_DELIMITER.len())..);
            }

            (!text.is_empty()).then_some((text, true))
        } else {
            let anchor = context_editor.selections.newest_anchor();
            let text = context_editor
                .buffer()
                .read(cx)
                .read(cx)
                .text_for_range(anchor.range())
                .collect::<String>();

            (!text.is_empty()).then_some((text, false))
        }
    }

    fn insert_selection(
        workspace: &mut Workspace,
        _: &InsertIntoEditor,
        cx: &mut ViewContext<Workspace>,
    ) {
        let Some(panel) = workspace.panel::<AssistantPanel>(cx) else {
            return;
        };
        let Some(context_editor_view) = panel.read(cx).active_context_editor(cx) else {
            return;
        };
        let Some(active_editor_view) = workspace
            .active_item(cx)
            .and_then(|item| item.act_as::<Editor>(cx))
        else {
            return;
        };

        if let Some((text, _)) = Self::get_selection_or_code_block(&context_editor_view, cx) {
            active_editor_view.update(cx, |editor, cx| {
                editor.insert(&text, cx);
                editor.focus(cx);
            })
        }
    }

    fn copy_code(workspace: &mut Workspace, _: &CopyCode, cx: &mut ViewContext<Workspace>) {
        let result = maybe!({
            let panel = workspace.panel::<AssistantPanel>(cx)?;
            let context_editor_view = panel.read(cx).active_context_editor(cx)?;
            Self::get_selection_or_code_block(&context_editor_view, cx)
        });
        let Some((text, is_code_block)) = result else {
            return;
        };

        cx.write_to_clipboard(ClipboardItem::new_string(text));

        struct CopyToClipboardToast;
        workspace.show_toast(
            Toast::new(
                NotificationId::unique::<CopyToClipboardToast>(),
                format!(
                    "{} copied to clipboard.",
                    if is_code_block {
                        "Code block"
                    } else {
                        "Selection"
                    }
                ),
            )
            .autohide(),
            cx,
        );
    }

    fn insert_dragged_files(
        workspace: &mut Workspace,
        action: &InsertDraggedFiles,
        cx: &mut ViewContext<Workspace>,
    ) {
        let Some(panel) = workspace.panel::<AssistantPanel>(cx) else {
            return;
        };
        let Some(context_editor_view) = panel.read(cx).active_context_editor(cx) else {
            return;
        };

        let project = workspace.project().clone();

        let paths = match action {
            InsertDraggedFiles::ProjectPaths(paths) => Task::ready((paths.clone(), vec![])),
            InsertDraggedFiles::ExternalFiles(paths) => {
                let tasks = paths
                    .clone()
                    .into_iter()
                    .map(|path| Workspace::project_path_for_path(project.clone(), &path, false, cx))
                    .collect::<Vec<_>>();

                cx.spawn(move |_, cx| async move {
                    let mut paths = vec![];
                    let mut worktrees = vec![];

                    let opened_paths = futures::future::join_all(tasks).await;
                    for (worktree, project_path) in opened_paths.into_iter().flatten() {
                        let Ok(worktree_root_name) =
                            worktree.read_with(&cx, |worktree, _| worktree.root_name().to_string())
                        else {
                            continue;
                        };

                        let mut full_path = PathBuf::from(worktree_root_name.clone());
                        full_path.push(&project_path.path);
                        paths.push(full_path);
                        worktrees.push(worktree);
                    }

                    (paths, worktrees)
                })
            }
        };

        cx.spawn(|_, mut cx| async move {
            let (paths, dragged_file_worktrees) = paths.await;
            let cmd_name = file_command::FileSlashCommand.name();

            context_editor_view
                .update(&mut cx, |context_editor, cx| {
                    let file_argument = paths
                        .into_iter()
                        .map(|path| path.to_string_lossy().to_string())
                        .collect::<Vec<_>>()
                        .join(" ");

                    context_editor.editor.update(cx, |editor, cx| {
                        editor.insert("\n", cx);
                        editor.insert(&format!("/{} {}", cmd_name, file_argument), cx);
                    });

                    context_editor.confirm_command(&ConfirmCommand, cx);

                    context_editor
                        .dragged_file_worktrees
                        .extend(dragged_file_worktrees);
                })
                .log_err();
        })
        .detach();
    }

    fn quote_selection(
        workspace: &mut Workspace,
        _: &QuoteSelection,
        cx: &mut ViewContext<Workspace>,
    ) {
        let Some(panel) = workspace.panel::<AssistantPanel>(cx) else {
            return;
        };
        let Some(editor) = workspace
            .active_item(cx)
            .and_then(|item| item.act_as::<Editor>(cx))
        else {
            return;
        };

        let mut creases = vec![];
        editor.update(cx, |editor, cx| {
            let selections = editor.selections.all_adjusted(cx);
            let buffer = editor.buffer().read(cx).snapshot(cx);
            for selection in selections {
                let range = editor::ToOffset::to_offset(&selection.start, &buffer)
                    ..editor::ToOffset::to_offset(&selection.end, &buffer);
                let selected_text = buffer.text_for_range(range.clone()).collect::<String>();
                if selected_text.is_empty() {
                    continue;
                }
                let start_language = buffer.language_at(range.start);
                let end_language = buffer.language_at(range.end);
                let language_name = if start_language == end_language {
                    start_language.map(|language| language.code_fence_block_name())
                } else {
                    None
                };
                let language_name = language_name.as_deref().unwrap_or("");
                let filename = buffer
                    .file_at(selection.start)
                    .map(|file| file.full_path(cx));
                let text = if language_name == "markdown" {
                    selected_text
                        .lines()
                        .map(|line| format!("> {}", line))
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    let start_symbols = buffer
                        .symbols_containing(selection.start, None)
                        .map(|(_, symbols)| symbols);
                    let end_symbols = buffer
                        .symbols_containing(selection.end, None)
                        .map(|(_, symbols)| symbols);

                    let outline_text = if let Some((start_symbols, end_symbols)) =
                        start_symbols.zip(end_symbols)
                    {
                        Some(
                            start_symbols
                                .into_iter()
                                .zip(end_symbols)
                                .take_while(|(a, b)| a == b)
                                .map(|(a, _)| a.text)
                                .collect::<Vec<_>>()
                                .join(" > "),
                        )
                    } else {
                        None
                    };

                    let line_comment_prefix = start_language
                        .and_then(|l| l.default_scope().line_comment_prefixes().first().cloned());

                    let fence = codeblock_fence_for_path(
                        filename.as_deref(),
                        Some(selection.start.row..=selection.end.row),
                    );

                    if let Some((line_comment_prefix, outline_text)) =
                        line_comment_prefix.zip(outline_text)
                    {
                        let breadcrumb =
                            format!("{line_comment_prefix}Excerpt from: {outline_text}\n");
                        format!("{fence}{breadcrumb}{selected_text}\n```")
                    } else {
                        format!("{fence}{selected_text}\n```")
                    }
                };
                let crease_title = if let Some(path) = filename {
                    let start_line = selection.start.row + 1;
                    let end_line = selection.end.row + 1;
                    if start_line == end_line {
                        format!("{}, Line {}", path.display(), start_line)
                    } else {
                        format!("{}, Lines {} to {}", path.display(), start_line, end_line)
                    }
                } else {
                    "Quoted selection".to_string()
                };
                creases.push((text, crease_title));
            }
        });
        if creases.is_empty() {
            return;
        }
        // Activate the panel
        if !panel.focus_handle(cx).contains_focused(cx) {
            workspace.toggle_panel_focus::<AssistantPanel>(cx);
        }

        panel.update(cx, |_, cx| {
            // Wait to create a new context until the workspace is no longer
            // being updated.
            cx.defer(move |panel, cx| {
                if let Some(context) = panel
                    .active_context_editor(cx)
                    .or_else(|| panel.new_context(cx))
                {
                    context.update(cx, |context, cx| {
                        context.editor.update(cx, |editor, cx| {
                            editor.insert("\n", cx);
                            for (text, crease_title) in creases {
                                let point = editor.selections.newest::<Point>(cx).head();
                                let start_row = MultiBufferRow(point.row);

                                editor.insert(&text, cx);

                                let snapshot = editor.buffer().read(cx).snapshot(cx);
                                let anchor_before = snapshot.anchor_after(point);
                                let anchor_after = editor
                                    .selections
                                    .newest_anchor()
                                    .head()
                                    .bias_left(&snapshot);

                                editor.insert("\n", cx);

                                let fold_placeholder = quote_selection_fold_placeholder(
                                    crease_title,
                                    cx.view().downgrade(),
                                );
                                let crease = Crease::new(
                                    anchor_before..anchor_after,
                                    fold_placeholder,
                                    render_quote_selection_output_toggle,
                                    |_, _, _| Empty.into_any(),
                                );
                                editor.insert_creases(vec![crease], cx);
                                editor.fold_at(
                                    &FoldAt {
                                        buffer_row: start_row,
                                    },
                                    cx,
                                );
                            }
                        })
                    });
                };
            });
        });
    }

    fn copy(&mut self, _: &editor::actions::Copy, cx: &mut ViewContext<Self>) {
        if self.editor.read(cx).selections.count() == 1 {
            let (copied_text, metadata, _) = self.get_clipboard_contents(cx);
            cx.write_to_clipboard(ClipboardItem::new_string_with_json_metadata(
                copied_text,
                metadata,
            ));
            cx.stop_propagation();
            return;
        }

        cx.propagate();
    }

    fn cut(&mut self, _: &editor::actions::Cut, cx: &mut ViewContext<Self>) {
        if self.editor.read(cx).selections.count() == 1 {
            let (copied_text, metadata, selections) = self.get_clipboard_contents(cx);

            self.editor.update(cx, |editor, cx| {
                editor.transact(cx, |this, cx| {
                    this.change_selections(Some(Autoscroll::fit()), cx, |s| {
                        s.select(selections);
                    });
                    this.insert("", cx);
                    cx.write_to_clipboard(ClipboardItem::new_string_with_json_metadata(
                        copied_text,
                        metadata,
                    ));
                });
            });

            cx.stop_propagation();
            return;
        }

        cx.propagate();
    }

    fn get_clipboard_contents(
        &mut self,
        cx: &mut ViewContext<Self>,
    ) -> (String, CopyMetadata, Vec<text::Selection<usize>>) {
        let (snapshot, selection, creases) = self.editor.update(cx, |editor, cx| {
            let mut selection = editor.selections.newest::<Point>(cx);
            let snapshot = editor.buffer().read(cx).snapshot(cx);

            let is_entire_line = selection.is_empty() || editor.selections.line_mode;
            if is_entire_line {
                selection.start = Point::new(selection.start.row, 0);
                selection.end =
                    cmp::min(snapshot.max_point(), Point::new(selection.start.row + 1, 0));
                selection.goal = SelectionGoal::None;
            }

            let selection_start = snapshot.point_to_offset(selection.start);

            (
                snapshot.clone(),
                selection.clone(),
                editor.display_map.update(cx, |display_map, cx| {
                    display_map
                        .snapshot(cx)
                        .crease_snapshot
                        .creases_in_range(
                            MultiBufferRow(selection.start.row)
                                ..MultiBufferRow(selection.end.row + 1),
                            &snapshot,
                        )
                        .filter_map(|crease| {
                            if let Some(metadata) = &crease.metadata {
                                let start = crease
                                    .range
                                    .start
                                    .to_offset(&snapshot)
                                    .saturating_sub(selection_start);
                                let end = crease
                                    .range
                                    .end
                                    .to_offset(&snapshot)
                                    .saturating_sub(selection_start);

                                let range_relative_to_selection = start..end;

                                if range_relative_to_selection.is_empty() {
                                    None
                                } else {
                                    Some(SelectedCreaseMetadata {
                                        range_relative_to_selection,
                                        crease: metadata.clone(),
                                    })
                                }
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                }),
            )
        });

        let selection = selection.map(|point| snapshot.point_to_offset(point));
        let context = self.context.read(cx);

        let mut text = String::new();
        for message in context.messages(cx) {
            if message.offset_range.start >= selection.range().end {
                break;
            } else if message.offset_range.end >= selection.range().start {
                let range = cmp::max(message.offset_range.start, selection.range().start)
                    ..cmp::min(message.offset_range.end, selection.range().end);
                if !range.is_empty() {
                    for chunk in context.buffer().read(cx).text_for_range(range) {
                        text.push_str(chunk);
                    }
                    if message.offset_range.end < selection.range().end {
                        text.push('\n');
                    }
                }
            }
        }

        (text, CopyMetadata { creases }, vec![selection])
    }

    fn paste(&mut self, action: &editor::actions::Paste, cx: &mut ViewContext<Self>) {
        cx.stop_propagation();

        let images = if let Some(item) = cx.read_from_clipboard() {
            item.into_entries()
                .filter_map(|entry| {
                    if let ClipboardEntry::Image(image) = entry {
                        Some(image)
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

        let metadata = if let Some(item) = cx.read_from_clipboard() {
            item.entries().first().and_then(|entry| {
                if let ClipboardEntry::String(text) = entry {
                    text.metadata_json::<CopyMetadata>()
                } else {
                    None
                }
            })
        } else {
            None
        };

        if images.is_empty() {
            self.editor.update(cx, |editor, cx| {
                let paste_position = editor.selections.newest::<usize>(cx).head();
                editor.paste(action, cx);

                if let Some(metadata) = metadata {
                    let buffer = editor.buffer().read(cx).snapshot(cx);

                    let mut buffer_rows_to_fold = BTreeSet::new();
                    let weak_editor = cx.view().downgrade();
                    editor.insert_creases(
                        metadata.creases.into_iter().map(|metadata| {
                            let start = buffer.anchor_after(
                                paste_position + metadata.range_relative_to_selection.start,
                            );
                            let end = buffer.anchor_before(
                                paste_position + metadata.range_relative_to_selection.end,
                            );

                            let buffer_row = MultiBufferRow(start.to_point(&buffer).row);
                            buffer_rows_to_fold.insert(buffer_row);
                            Crease::new(
                                start..end,
                                FoldPlaceholder {
                                    render: render_fold_icon_button(
                                        weak_editor.clone(),
                                        metadata.crease.icon,
                                        metadata.crease.label.clone(),
                                    ),
                                    constrain_width: false,
                                    merge_adjacent: false,
                                },
                                render_slash_command_output_toggle,
                                |_, _, _| Empty.into_any(),
                            )
                            .with_metadata(metadata.crease.clone())
                        }),
                        cx,
                    );
                    for buffer_row in buffer_rows_to_fold.into_iter().rev() {
                        editor.fold_at(&FoldAt { buffer_row }, cx);
                    }
                }
            });
        } else {
            let mut image_positions = Vec::new();
            self.editor.update(cx, |editor, cx| {
                editor.transact(cx, |editor, cx| {
                    let edits = editor
                        .selections
                        .all::<usize>(cx)
                        .into_iter()
                        .map(|selection| (selection.start..selection.end, "\n"));
                    editor.edit(edits, cx);

                    let snapshot = editor.buffer().read(cx).snapshot(cx);
                    for selection in editor.selections.all::<usize>(cx) {
                        image_positions.push(snapshot.anchor_before(selection.end));
                    }
                });
            });

            self.context.update(cx, |context, cx| {
                for image in images {
                    let Some(render_image) = image.to_image_data(cx).log_err() else {
                        continue;
                    };
                    let image_id = image.id();
                    let image_task = LanguageModelImage::from_image(image, cx).shared();

                    for image_position in image_positions.iter() {
                        context.insert_content(
                            Content::Image {
                                anchor: image_position.text_anchor,
                                image_id,
                                image: image_task.clone(),
                                render_image: render_image.clone(),
                            },
                            cx,
                        );
                    }
                }
            });
        }
    }

    fn update_image_blocks(&mut self, cx: &mut ViewContext<Self>) {
        self.editor.update(cx, |editor, cx| {
            let buffer = editor.buffer().read(cx).snapshot(cx);
            let excerpt_id = *buffer.as_singleton().unwrap().0;
            let old_blocks = std::mem::take(&mut self.blocks);
            let mut new_blocks = vec![];
            let mut block_index_to_message = vec![];

            let render_block = |message: MessageMetadata| -> RenderBlock {
                Box::new({
                    let context = self.context.clone();
                    move |cx| {
                        let message_id = MessageId(message.timestamp);
                        let show_spinner = message.role == Role::Assistant
                            && message.status == MessageStatus::Pending;

                        let label = match message.role {
                            Role::User => {
                                Label::new("You").color(Color::Default).into_any_element()
                            }
                            Role::Assistant => {
                                let label = Label::new("Assistant").color(Color::Info);
                                if show_spinner {
                                    label
                                        .with_animation(
                                            "pulsating-label",
                                            Animation::new(Duration::from_secs(2))
                                                .repeat()
                                                .with_easing(pulsating_between(0.4, 0.8)),
                                            |label, delta| label.alpha(delta),
                                        )
                                        .into_any_element()
                                } else {
                                    label.into_any_element()
                                }
                            }

                            Role::System => Label::new("System")
                                .color(Color::Warning)
                                .into_any_element(),
                        };

                        let sender = ButtonLike::new("role")
                            .style(ButtonStyle::Filled)
                            .child(label)
                            .tooltip(|cx| {
                                Tooltip::with_meta(
                                    "Toggle message role",
                                    None,
                                    "Available roles: You (User), Assistant, System",
                                    cx,
                                )
                            })
                            .on_click({
                                let context = context.clone();
                                move |_, cx| {
                                    context.update(cx, |context, cx| {
                                        context.cycle_message_roles(
                                            HashSet::from_iter(Some(message_id)),
                                            cx,
                                        )
                                    })
                                }
                            });

                        h_flex()
                            .id(("message_header", message_id.as_u64()))
                            .pl(cx.gutter_dimensions.full_width())
                            .h_11()
                            .w_full()
                            .relative()
                            .gap_1()
                            .child(sender)
                            .children(match &message.cache {
                                Some(cache) if cache.is_final_anchor => match cache.status {
                                    CacheStatus::Cached => Some(
                                        div()
                                            .id("cached")
                                            .child(
                                                Icon::new(IconName::DatabaseZap)
                                                    .size(IconSize::XSmall)
                                                    .color(Color::Hint),
                                            )
                                            .tooltip(|cx| {
                                                Tooltip::with_meta(
                                                    "Context cached",
                                                    None,
                                                    "Large messages cached to optimize performance",
                                                    cx,
                                                )
                                            })
                                            .into_any_element(),
                                    ),
                                    CacheStatus::Pending => Some(
                                        div()
                                            .child(
                                                Icon::new(IconName::Ellipsis)
                                                    .size(IconSize::XSmall)
                                                    .color(Color::Hint),
                                            )
                                            .into_any_element(),
                                    ),
                                },
                                _ => None,
                            })
                            .into_any_element()
                    }
                })
            };
            for message in self.context.read(cx).messages(cx) {
                // This is a new message.
                new_blocks.push(create_block_properties(&message));
                block_index_to_message.push((message.id, MessageMetadata::from(&message)));
            }
            let ids = editor.insert_blocks(new_blocks, None, cx);
            old_blocks.extend(ids.into_iter().zip(block_index_to_message).map(
                |(block_id, (message_id, message_meta))| (message_id, (message_meta, block_id)),
            ));
            self.blocks = old_blocks;
        });
    }

    /// Returns either the selected text, or the content of the Markdown code
    /// block surrounding the cursor.
    fn get_selection_or_code_block(
        context_editor_view: &View<ContextEditor>,
        cx: &mut ViewContext<Workspace>,
    ) -> Option<(String, bool)> {
        const CODE_FENCE_DELIMITER: &'static str = "```";

        let context_editor = context_editor_view.read(cx).editor.read(cx);

        if context_editor.selections.newest::<Point>(cx).is_empty() {
            let snapshot = context_editor.buffer().read(cx).snapshot(cx);
            let head = context_editor.selections.newest::<Point>(cx).head();
            let offset = snapshot.point_to_offset(head);

            let surrounding_code_block_range = find_surrounding_code_block(snapshot, offset)?;
            let mut text = snapshot
                .text_for_range(surrounding_code_block_range)
                .collect::<String>();

            // If there is no newline trailing the closing three-backticks, then
            // tree-sitter-md extends the range of the content node to include
            // the backticks.
            if text.ends_with(CODE_FENCE_DELIMITER) {
                text.drain((text.len() - CODE_FENCE_DELIMITER.len())..);
            }

            (!text.is_empty()).then_some((text, true))
        } else {
            let anchor = context_editor.selections.newest_anchor();
            let text = context_editor
                .buffer()
                .read(cx)
                .read(cx)
                .text_for_range(anchor.range())
                .collect::<String>();

            (!text.is_empty()).then_some((text, false))
        }
    }

    fn insert_selection(
        workspace: &mut Workspace,
        _: &InsertIntoEditor,
        cx: &mut ViewContext<Workspace>,
    ) {
        let Some(panel) = workspace.panel::<AssistantPanel>(cx) else {
            return;
        };
        let Some(context_editor_view) = panel.read(cx).active_context_editor(cx) else {
            return;
        };
        let Some(active_editor_view) = workspace
            .active_item(cx)
            .and_then(|item| item.act_as::<Editor>(cx))
        else {
            return;
        };

        if let Some((text, _)) = Self::get_selection_or_code_block(&context_editor_view, cx) {
            active_editor_view.update(cx, |editor, cx| {
                editor.insert(&text, cx);
                editor.focus(cx);
            })
        }
    }

    fn copy_code(workspace: &mut Workspace, _: &CopyCode, cx: &mut ViewContext<Workspace>) {
        let result = maybe!({
            let panel = workspace.panel::<AssistantPanel>(cx)?;
            let context_editor_view = panel.read(cx).active_context_editor(cx)?;
            Self::get_selection_or_code_block(&context_editor_view, cx)
        });
        let Some((text, is_code_block)) = result else {
            return;
        };

        cx.write_to_clipboard(ClipboardItem::new_string(text));

        struct CopyToClipboardToast;
        workspace.show_toast(
            Toast::new(
                NotificationId::unique::<CopyToClipboardToast>(),
                format!(
                    "{} copied to clipboard.",
                    if is_code_block {
                        "Code block"
                    } else {
                        "Selection"
                    }
                ),
            )
            .autohide(),
            cx,
        );
    }

    fn insert_dragged_files(
        workspace: &mut Workspace,
        action: &InsertDraggedFiles,
        cx: &mut ViewContext<Workspace>,
    ) {
        let Some(panel) = workspace.panel::<AssistantPanel>(cx) else {
            return;
        };
        let Some(context_editor_view) = panel.read(cx).active_context_editor(cx) else {
            return;
        };

        let project = workspace.project().clone();

        let paths = match action {
            InsertDraggedFiles::ProjectPaths(paths) => Task::ready((paths.clone(), vec![])),
            InsertDraggedFiles::ExternalFiles(paths) => {
                let tasks = paths
                    .clone()
                    .into_iter()
                    .map(|path| Workspace::project_path_for_path(project.clone(), &path, false, cx))
                    .collect::<Vec<_>>();

                cx.spawn(move |_, cx| async move {
                    let mut paths = vec![];
                    let mut worktrees = vec![];

                    let opened_paths = futures::future::join_all(tasks).await;
                    for (worktree, project_path) in opened_paths.into_iter().flatten() {
                        let Ok(worktree_root_name) =
                            worktree.read_with(&cx, |worktree, _| worktree.root_name().to_string())
                        else {
                            continue;
                        };

                        let mut full_path = PathBuf::from(worktree_root_name.clone());
                        full_path.push(&project_path.path);
                        paths.push(full_path);
                        worktrees.push(worktree);
                    }

                    (paths, worktrees)
                })
            }
        };

        cx.spawn(|_, mut cx| async move {
            let (paths, dragged_file_worktrees) = paths.await;
            let cmd_name = file_command::FileSlashCommand.name();

            context_editor_view
                .update(&mut cx, |context_editor, cx| {
                    let file_argument = paths
                        .into_iter()
                        .map(|path| path.to_string_lossy().to_string())
                        .collect::<Vec<_>>()
                        .join(" ");

                    context_editor.editor.update(cx, |editor, cx| {
                        editor.insert("\n", cx);
                        editor.insert(&format!("/{} {}", cmd_name, file_argument), cx);
                    });

                    context_editor.confirm_command(&ConfirmCommand, cx);

                    context_editor
                        .dragged_file_worktrees
                        .extend(dragged_file_worktrees);
                })
                .log_err();
        })
        .detach();
    }

    fn quote_selection(
        workspace: &mut Workspace,
        _: &QuoteSelection,
        cx: &mut ViewContext<Workspace>,
    ) {
        let Some(panel) = workspace.panel::<AssistantPanel>(cx) else {
            return;
        };
        let Some(editor) = workspace
            .active_item(cx)
            .and_then(|item| item.act_as::<Editor>(cx))
        else {
            return;
        };

        let mut creases = vec![];
        editor.update(cx, |editor, cx| {
            let selections = editor.selections.all_adjusted(cx);
            let buffer = editor.buffer().read(cx).snapshot(cx);
            for selection in selections {
                let range = editor::ToOffset::to_offset(&selection.start, &buffer)
                    ..editor::ToOffset::to_offset(&selection.end, &buffer);
                let selected_text = buffer.text_for_range(range.clone()).collect::<String>();
                if selected_text.is_empty() {
                    continue;
                }
                let start_language = buffer.language_at(range.start);
                let end_language = buffer.language_at(range.end);
                let language_name = if start_language == end_language {
                    start_language.map(|language| language.code_fence_block_name())
                } else {
                    None
                };
                let language_name = language_name.as_deref().unwrap_or("");
                let filename = buffer
                    .file_at(selection.start)
                    .map(|file| file.full_path(cx));
                let text = if language_name == "markdown" {
                    selected_text
                        .lines()
                        .map(|line| format!("> {}", line))
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    let start_symbols = buffer
                        .symbols_containing(selection.start, None)
                        .map(|(_, symbols)| symbols);
                    let end_symbols = buffer
                        .symbols_containing(selection.end, None)
                        .map(|(_, symbols)| symbols);

                    let outline_text = if let Some((start_symbols, end_symbols)) =
                        start_symbols.zip(end_symbols)
                    {
                        Some(
                            start_symbols
                                .into_iter()
                                .zip(end_symbols)
                                .take_while(|(a, b)| a == b)
                                .map(|(a, _)| a.text)
                                .collect::<Vec<_>>()
                                .join(" > "),
                        )
                    } else {
                        None
                    };

                    let line_comment_prefix = start_language
                        .and_then(|l| l.default_scope().line_comment_prefixes().first().cloned());

                    let fence = codeblock_fence_for_path(
                        filename.as_deref(),
                        Some(selection.start.row..=selection.end.row),
                    );

                    if let Some((line_comment_prefix, outline_text)) =
                        line_comment_prefix.zip(outline_text)
                    {
                        let breadcrumb =
                            format!("{line_comment_prefix}Excerpt from: {outline_text}\n");
                        format!("{fence}{breadcrumb}{selected_text}\n```")
                    } else {
                        format!("{fence}{selected_text}\n```")
                    }
                };
                let crease_title = if let Some(path) = filename {
                    let start_line = selection.start.row + 1;
                    let end_line = selection.end.row + 1;
                    if start_line == end_line {
                        format!("{}, Line {}", path.display(), start_line)
                    } else {
                        format!("{}, Lines {} to {}", path.display(), start_line, end_line)
                    }
                } else {
                    "Quoted selection".to_string()
                };
                creases.push((text, crease_title));
            }
        });
        if creases.is_empty() {
            return;
        }
        // Activate the panel
        if !panel.focus_handle(cx).contains_focused(cx) {
            workspace.toggle_panel_focus::<AssistantPanel>(cx);
        }

        panel.update(cx, |_, cx| {
            // Wait to create a new context until the workspace is no longer
            // being updated.
            cx.defer(move |panel, cx| {
                if let Some(context) = panel
                    .active_context_editor(cx)
                    .or_else(|| panel.new_context(cx))
                {
                    context.update(cx, |context, cx| {
                        context.editor.update(cx, |editor, cx| {
                            editor.insert("\n", cx);
                            for (text, crease_title) in creases {
                                let point = editor.selections.newest::<Point>(cx).head();
                                let start_row = MultiBufferRow(point.row);

                                editor.insert(&text, cx);

                                let snapshot = editor.buffer().read(cx).snapshot(cx);
                                let anchor_before = snapshot.anchor_after(point);
                                let anchor_after = editor
                                    .selections
                                    .newest_anchor()
                                    .head()
                                    .bias_left(&snapshot);

                                editor.insert("\n", cx);

                                let fold_placeholder = quote_selection_fold_placeholder(
                                    crease_title,
                                    cx.view().downgrade(),
                                );
                                let crease = Crease::new(
                                    anchor_before..anchor_after,
                                    fold_placeholder,
                                    render_quote_selection_output_toggle,
                                    |_, _, _| Empty.into_any(),
                                );
                                editor.insert_creases(vec![crease], cx);
                                editor.fold_at(
                                    &FoldAt {
                                        buffer_row: start_row,
                                    },
                                    cx,
                                );
                            }
                        })
                    });
                };
            });
        });
    }

    fn copy(&mut self, _: &editor::actions::Copy, cx: &mut ViewContext<Self>) {
        if self.editor.read(cx).selections.count() == 1 {
            let (copied_text, metadata, _) = self.get_clipboard_contents(cx);
            cx.write_to_clipboard(ClipboardItem::new_string_with_json_metadata(
                copied_text,
                metadata,
            ));
            cx.stop_propagation();
            return;
        }

        cx.propagate();
    }

    fn cut(&mut self, _: &editor::actions::Cut, cx: &mut ViewContext<Self>) {
        if self.editor.read(cx).selections.count() == 1 {
            let (copied_text, metadata, selections) = self.get_clipboard_contents(cx);

            self.editor.update(cx, |editor, cx| {
                editor.transact(cx, |this, cx| {
                    this.change_selections(Some(Autoscroll::fit()), cx, |s| {
                        s.select(selections);
                    });
                    this.insert("", cx);
                    cx.write_to_clipboard(ClipboardItem::new_string_with_json_metadata(
                        copied_text,
                        metadata,
                    ));
                });
            });

            cx.stop_propagation();
            return;
        }

        cx.propagate();
    }

    fn get_clipboard_contents(
        &mut self,
        cx: &mut ViewContext<Self>,
    ) -> (String, CopyMetadata, Vec<text::Selection<usize>>) {
        let (snapshot, selection, creases) = self.editor.update(cx, |editor, cx| {
            let mut selection = editor.selections.newest::<Point>(cx);
            let snapshot = editor.buffer().read(cx).snapshot(cx);

            let is_entire_line = selection.is_empty() || editor.selections.line_mode;
            if is_entire_line {
                selection.start = Point::new(selection.start.row, 0);
                selection.end =
                    cmp::min(snapshot.max_point(), Point::new(selection.start.row + 1, 0));
                selection.goal = SelectionGoal::None;
            }

            let selection_start = snapshot.point_to_offset(selection.start);

            (
                snapshot.clone(),
                selection.clone(),
                editor.display_map.update(cx, |display_map, cx| {
                    display_map
                        .snapshot(cx)
                        .crease_snapshot
                        .creases_in_range(
                            MultiBufferRow(selection.start.row)
                                ..MultiBufferRow(selection.end.row + 1),
                            &snapshot,
                        )
                        .filter_map(|crease| {
                            if let Some(metadata) = &crease.metadata {
                                let start = crease
                                    .range
                                    .start
                                    .to_offset(&snapshot)
                                    .saturating_sub(selection_start);
                                let end = crease
                                    .range
                                    .end
                                    .to_offset(&snapshot)
                                    .saturating_sub(selection_start);

                                let range_relative_to_selection = start..end;

                                if range_relative_to_selection.is_empty() {
                                    None
                                } else {
                                    Some(SelectedCreaseMetadata {
                                        range_relative_to_selection,
                                        crease: metadata.clone(),
                                    })
                                }
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                }),
            )
        });

        let selection = selection.map(|point| snapshot.point_to_offset(point));
        let context = self.context.read(cx);

        let mut text = String::new();
        for message in context.messages(cx) {
            if message.offset_range.start >= selection.range().end {
                break;
            } else if message.offset_range.end >= selection.range().start {
                let range = cmp::max(message.offset_range.start, selection.range().start)
                    ..cmp::min(message.offset_range.end, selection.range().end);
                if !range.is_empty() {
                    for chunk in context.buffer().read(cx).text_for_range(range) {
                        text.push_str(chunk);
                    }
                    if message.offset_range.end < selection.range().end {
                        text.push('\n');
                    }
                }
            }
        }

        (text, CopyMetadata { creases }, vec![selection])
    }

    fn paste(&mut self, action: &editor::actions::Paste, cx: &mut ViewContext<Self>) {
        cx.stop_propagation();

        let images = if let Some(item) = cx.read_from_clipboard() {
            item.into_entries()
                .filter_map(|entry| {
                    if let ClipboardEntry::Image(image) = entry {
                        Some(image)
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

        let metadata = if let Some(item) = cx.read_from_clipboard() {
            item.entries().first().and_then(|entry| {
                if let ClipboardEntry::String(text) = entry {
                    text.metadata_json::<CopyMetadata>()
                } else {
                    None
                }
            })
        } else {
            None
        };

        if images.is_empty() {
            self.editor.update(cx, |editor, cx| {
                let paste_position = editor.selections.newest::<usize>(cx).head();
                editor.paste(action, cx);

                if let Some(metadata) = metadata {
                    let buffer = editor.buffer().read(cx).snapshot(cx);

                    let mut buffer_rows_to_fold = BTreeSet::new();
                    let weak_editor = cx.view().downgrade();
                    editor.insert_creases(
                        metadata.creases.into_iter().map(|metadata| {
                            let start = buffer.anchor_after(
                                paste_position + metadata.range_relative_to_selection.start,
                            );
                            let end = buffer.anchor_before(
                                paste_position + metadata.range_relative_to_selection.end,
                            );

                            let buffer_row = MultiBufferRow(start.to_point(&buffer).row);
                            buffer_rows_to_fold.insert(buffer_row);
                            Crease::new(
                                start..end,
                                FoldPlaceholder {
                                    render: render_fold_icon_button(
                                        weak_editor.clone(),
                                        metadata.crease.icon,
                                        metadata.crease.label.clone(),
                                    ),
                                    constrain_width: false,
                                    merge_adjacent: false,
                                },
                                render_slash_command_output_toggle,
                                |_, _, _| Empty.into_any(),
                            )
                            .with_metadata(metadata.crease.clone())
                        }),
                        cx,
                    );
                    for buffer_row in buffer_rows_to_fold.into_iter().rev() {
                        editor.fold_at(&FoldAt { buffer_row }, cx);
                    }
                }
            });
        } else {
            let mut image_positions = Vec::new();
            self.editor.update(cx, |editor, cx| {
                editor.transact(cx, |editor, cx| {
                    let edits = editor
                        .selections
                        .all::<usize>(cx)
                        .into_iter()
                        .map(|selection| (selection.start..selection.end, "\n"));
                    editor.edit(edits, cx);

                    let snapshot = editor.buffer().read(cx).snapshot(cx);
                    for selection in editor.selections.all::<usize>(cx) {
                        image_positions.push(snapshot.anchor_before(selection.end));
                    }
                });
            });

            self.context.update(cx, |context, cx| {
                for image in images {
                    let Some(render_image) = image.to_image_data(cx).log_err() else {
                        continue;
                    };
                    let image_id = image.id();
                    let image_task = LanguageModelImage::from_image(image, cx).shared();

                    for image_position in image_positions.iter() {
                        context.insert_content(
                            Content::Image {
                                anchor: image_position.text_anchor,
                                image_id,
                                image: image_task.clone(),
                                render_image: render_image.clone(),
                            },
                            cx,
                        );
                    }
                }
            });
        }
    }

    fn update_image_blocks(&mut self, cx: &mut ViewContext<Self>) {
        self.editor.update(cx, |editor, cx| {
            let buffer = editor.buffer().read(cx).snapshot(cx);
            let excerpt_id = *buffer.as_singleton().unwrap().0;
            let old_blocks = std::mem::take(&mut self.blocks);
            let mut new_blocks = vec![];
            let mut block_index_to_message = vec![];

            let render_block = |message: MessageMetadata| -> RenderBlock {
                Box::new({
                    let context = self.context.clone();
                    move |cx| {
                        let message_id = MessageId(message.timestamp);
                        let show_spinner = message.role == Role::Assistant
                            && message.status == MessageStatus::Pending;

                        let label = match message.role {
                            Role::User => {
                                Label::new("You").color(Color::Default).into_any_element()
                            }
                            Role::Assistant => {
                                let label = Label::new("Assistant").color(Color::Info);
                                if show_spinner {
                                    label
                                        .with_animation(
                                            "pulsating-label",
                                            Animation::new(Duration::from_secs(2))
                                                .repeat()
                                                .with_easing(pulsating_between(0.4, 0.8)),
                                            |label, delta| label.alpha(delta),
                                        )
                                        .into_any_element()
                                } else {
                                    label.into_any_element()
                                }
                            }

                            Role::System => Label::new("System")
                                .color(Color::Warning)
                                .into_any_element(),
                        };

                        let sender = ButtonLike::new("role")
                            .style(ButtonStyle::Filled)
                            .child(label)
                            .tooltip(|cx| {
                                Tooltip::with_meta(
                                    "Toggle message role",
                                    None,
                                    "Available roles: You (User), Assistant, System",
                                    cx,
                                )
                            })
                            .on_click({
                                let context = context.clone();
                                move |_, cx| {
                                    context.update(cx, |context, cx| {
                                        context.cycle_message_roles(
                                            HashSet::from_iter(Some(message_id)),
                                            cx,
                                        )
                                    })
                                }
                            });

                        h_flex()
                            .id(("message_header", message_id.as_u64()))
                            .pl(cx.gutter_dimensions.full_width())
                            .h_11()
                            .w_full()
                            .relative()
                            .gap_1()
                            .child(sender)
                            .children(match &message.cache {
                                Some(cache) if cache.is_final_anchor => match cache.status {
                                    CacheStatus::Cached => Some(
                                        div()
                                            .id("cached")
                                            .child(
                                                Icon::new(IconName::DatabaseZap)
                                                    .size(IconSize::XSmall)
                                                    .color(Color::Hint),
                                            )
                                            .tooltip(|cx| {
                                                Tooltip::with_meta(
                                                    "Context cached",
                                                    None,
                                                    "Large messages cached to optimize performance",
                                                    cx,
                                                )
                                            })
                                            .into_any_element(),
                                    ),
                                    CacheStatus::Pending => Some(
                                        div()
                                            .child(
                                                Icon::new(IconName::Ellipsis)
                                                    .size(IconSize::XSmall)
                                                    .color(Color::Hint),
                                            )
                                            .into_any_element(),
                                    ),
                                },
                                _ => None,
                            })
                            .into_any_element()
                    }
                })
            };
            for message in self.context.read(cx).messages(cx) {
                // This is a new message.
                new_blocks.push(create_block_properties(&message));
                block_index_to_message.push((message.id, MessageMetadata::from(&message)));
            }
            let ids = editor.insert_blocks(new_blocks, None, cx);
            old_blocks.extend(ids.into_iter().zip(block_index_to_message).map(
                |(block_id, (message_id, message_meta))| (message_id, (message_meta, block_id)),
            ));
            self.blocks = old_blocks;
        });
    }

    /// Returns either the selected text, or the content of the Markdown code
    /// block surrounding the cursor.
    fn get_selection_or_code_block(
        context_editor_view: &View<ContextEditor>,
        cx: &mut ViewContext<Workspace>,
    ) -> Option<(String, bool)> {
        const CODE_FENCE_DELIMITER: &'static str = "```";

        let context_editor = context_editor_view.read(cx).editor.read(cx);

        if context_editor.selections.newest::<Point>(cx).is_empty() {
            let snapshot = context_editor.buffer().read(cx).snapshot(cx);
            let head = context_editor.selections.newest::<Point>(cx).head();
            let offset = snapshot.point_to_offset(head);

            let surrounding_code_block_range = find_surrounding_code_block(snapshot, offset)?;
            let mut text = snapshot
                .text_for_range(surrounding_code_block_range)
                .collect::<String>();

            // If there is no newline trailing the closing three-backticks, then
            // tree-sitter-md extends the range of the content node to include
            // the backticks.
            if text.ends_with(CODE_FENCE_DELIMITER) {
                text.drain((text.len() - CODE_FENCE_DELIMITER.len())..);
            }

            (!text.is_empty()).then_some((text, true))
        } else {
            let anchor = context_editor.selections.newest_anchor();
            let text = context_editor
                .buffer()
                .read(cx)
                .read(cx)
                .text_for_range(anchor.range())
                .collect::<String>();

            (!text.is_empty()).then_some((text, false))
        }
    }

    fn insert_selection(
        workspace: &mut Workspace,
        _: &InsertIntoEditor,
        cx: &mut ViewContext<Workspace>,
    ) {
        let Some(panel) = workspace.panel::<AssistantPanel>(cx) else {
            return;
        };
        let Some(context_editor_view) = panel.read(cx).active_context_editor(cx) else {
            return;
        };
        let Some(active_editor_view) = workspace
            .active_item(cx)
            .and_then(|item| item.act_as::<Editor>(cx))
        else {
            return;
        };

        if let Some((text, _)) = Self::get_selection_or_code_block(&context_editor_view, cx) {
            active_editor_view.update(cx, |editor, cx| {
                editor.insert(&text, cx);
                editor.focus(cx);
            })
        }
    }

    fn copy_code(workspace: &mut Workspace, _: &CopyCode, cx: &mut ViewContext<Workspace>) {
        let result = maybe!({
            let panel = workspace.panel::<AssistantPanel>(cx)?;
            let context_editor_view = panel.read(cx).active_context_editor(cx)?;
            Self::get_selection_or_code_block(&context_editor_view, cx)
        });
        let Some((text, is_code_block)) = result else {
            return;
        };

        cx.write_to_clipboard(ClipboardItem::new_string(text));

        struct CopyToClipboardToast;
        workspace.show_toast(
            Toast::new(
                NotificationId::unique::<CopyToClipboardToast>(),
                format!(
                    "{} copied to clipboard.",
                    if is_code_block {
                        "Code block"
                    } else {
                        "Selection"
                    }
                ),
            )
            .autohide(),
            cx,
        );
    }

    fn insert_dragged_files(
        workspace: &mut Workspace,
        action: &InsertDraggedFiles,
        cx: &mut ViewContext<Workspace>,
    ) {
        let Some(panel) = workspace.panel::<AssistantPanel>(cx) else {
            return;
        };
        let Some(context_editor_view) = panel.read(cx).active_context_editor(cx) else {
            return;
        };

        let project = workspace.project().clone();

        let paths = match action {
            InsertDraggedFiles::ProjectPaths(paths) => Task::ready((paths.clone(), vec![])),
            InsertDraggedFiles::ExternalFiles(paths) => {
                let tasks = paths
                    .clone()
                    .into_iter()
                    .map(|path| Workspace::project_path_for_path(project.clone(), &path, false, cx))
                    .collect::<Vec<_>>();

                cx.spawn(move |_, cx| async move {
                    let mut paths = vec![];
                    let mut worktrees = vec![];

                    let opened_paths = futures::future::join_all(tasks).await;
                    for (worktree, project_path) in opened_paths.into_iter().flatten() {
                        let Ok(worktree_root_name) =
                            worktree.read_with(&cx, |worktree, _| worktree.root_name().to_string())
                        else {
                            continue;
                        };

                        let mut full_path = PathBuf::from(worktree_root_name.clone());
                        full_path.push(&project_path.path);
                        paths.push(full_path);
                        worktrees.push(worktree);
                    }

                    (paths, worktrees)
                })
            }
        };

        cx.spawn(|_, mut cx| async move {
            let (paths, dragged_file_worktrees) = paths.await;
            let cmd_name = file_command::FileSlashCommand.name();

            context_editor_view
                .update(&mut cx, |context_editor, cx| {
                    let file_argument = paths
                        .into_iter()
                        .map(|path| path.to_string_lossy().to_string())
                        .collect::<Vec<_>>()
                        .join(" ");

                    context_editor.editor.update(cx, |editor, cx| {
                        editor.insert("\n", cx);
                        editor.insert(&format!("/{} {}", cmd_name, file_argument), cx);
                    });

                    context_editor.confirm_command(&ConfirmCommand, cx);

                    context_editor
                        .dragged_file_worktrees
                        .extend(dragged_file_worktrees);
                })
                .log_err();
        })
        .detach();
    }

    fn quote_selection(
        workspace: &mut Workspace,
        _: &QuoteSelection,
        cx: &mut ViewContext<Workspace>,
    ) {
        let Some(panel) = workspace.panel::<AssistantPanel>(cx) else {
            return;
        };
        let Some(editor) = workspace
            .active_item(cx)
            .and_then(|item| item.act_as::<Editor>(cx))
        else {
            return;
        };

        let mut creases = vec![];
        editor.update(cx, |editor, cx| {
            let selections = editor.selections.all_adjusted(cx);
            let buffer = editor.buffer().read(cx).snapshot(cx);
            for selection in selections {
                let range = editor::ToOffset::to_offset(&selection.start, &buffer)
                    ..editor::ToOffset::to_offset(&selection.end, &buffer);
                let selected_text = buffer.text_for_range(range.clone()).collect::<String>();
                if selected_text.is_empty() {
                    continue;
                }
                let start_language = buffer.language_at(range.start);
                let end_language = buffer.language_at(range.end);
                let language_name = if start_language == end_language {
                    start_language.map(|language| language.code_fence_block_name())
                } else {
                    None
                };
                let language_name = language_name.as_deref().unwrap_or("");
                let filename = buffer
                    .file_at(selection.start)
                    .map(|file| file.full_path(cx));
                let text = if language_name == "markdown" {
                    selected_text
                        .lines()
                        .map(|line| format!("> {}", line))
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    let start_symbols = buffer
                        .symbols_containing(selection.start, None)
                        .map(|(_, symbols)| symbols);
                    let end_symbols = buffer
                        .symbols_containing(selection.end, None)
                        .map(|(_, symbols)| symbols);

                    let outline_text = if let Some((start_symbols, end_symbols)) =
                        start_symbols.zip(end_symbols)
                    {
                        Some(
                            start_symbols
                                .into_iter()
                                .zip(end_symbols)
                                .take_while(|(a, b)| a == b)
                                .map(|(a, _)| a.text)
                                .collect::<Vec<_>>()
                                .join(" > "),
                        )
                    } else {
                        None
                    };

                    let line_comment_prefix = start_language
                        .and_then(|l| l.default_scope().line_comment_prefixes().first().cloned());

                    let fence = codeblock_fence_for_path(
                        filename.as_deref(),
                        Some(selection.start.row..=selection.end.row),
                    );

                    if let Some((line_comment_prefix, outline_text)) =
                        line_comment_prefix.zip(outline_text)
                    {
                        let breadcrumb =
                            format!("{line_comment_prefix}Excerpt from: {outline_text}\n");
                        format!("{fence}{breadcrumb}{selected_text}\n```")
                    } else {
                        format!("{fence}{selected_text}\n```")
                    }
                };
                let crease_title = if let Some(path) = filename {
                    let start_line = selection.start.row + 1;
                    let end_line = selection.end.row + 1;
                    if start_line == end_line {
                        format!("{}, Line {}", path.display(), start_line)
                    } else {
                        format!("{}, Lines {} to {}", path.display(), start_line, end_line)
                    }
                } else {
                    "Quoted selection".to_string()
                };
                creases.push((text, crease_title));
            }
        });
        if creases.is_empty() {
            return;
        }
        // Activate the panel
        if !panel.focus_handle(cx).contains_focused(cx) {
            workspace.toggle_panel_focus::<AssistantPanel>(cx);
        }

        panel.update(cx, |_, cx| {
            // Wait to create a new context until the workspace is no longer
            // being updated.
            cx.defer(move |panel, cx| {
                if let Some(context) = panel
                    .active_context_editor(cx)
                    .or_else(|| panel.new_context(cx))
                {
                    context.update(cx, |context, cx| {
                        context.editor.update(cx, |editor, cx| {
                            editor.insert("\n",cx);
                            for (text, crease_title) in creases {
                                let point = editor.selections.newest::<Point>(cx).head();
                                let start_row = MultiBufferRow(point.row);

                                editor.insert(&text, cx);

                                let snapshot = editor.buffer().read(cx).snapshot(cx);
                                let anchor_before = snapshot.anchor_after(point);
                                let anchor_after = editor
                                    .selections
                                    .newest_anchor()
                                    .head()
                                    .bias_left(&snapshot);

                                editor.insert("\n", cx);

                                let fold_placeholder = quote_selection_fold_placeholder(
                                    crease_title,
                                    cx.view().downgrade(),
                                );
                                let crease = Crease::new(
                                    anchor_before..anchor_after,
                                    fold_placeholder,
                                    render_quote_selection_output_toggle,
                                    |_, _, _| Empty.into_any(),
                                );
                                editor.insert_creases(vec![crease], cx);
                                editor.fold_at(
                                    &FoldAt {
                                        buffer_row: start_row,
                                    },
                                    cx,
                                );
                            }
                        })
                    });
                };
            });
        });
    }

    fn copy(&mut self, _: &editor::actions::Copy, cx: &mut ViewContext<Self>) {
        if self.editor.read(cx).selections.count() == 1 {
            let (copied_text, metadata, _) = self.get_clipboard_contents(cx);
            cx.write_to_clipboard(ClipboardItem::new_string_with_json_metadata(
                copied_text,
                metadata,
            ));
            cx.stop_propagation();
            return;
        }

        cx.propagate();
    }

    fn cut(&mut self, _: &editor::actions::Cut, cx: &mut ViewContext<Self>) {
        if self.editor.read(cx).selections.count() == 1 {
            let (copied_text, metadata, selections) = self.get_clipboard_contents(cx);

            self.editor.update(cx, |editor, cx| {
                editor.transact(cx, |this, cx| {
                    this.change_selections(Some(Autoscroll::fit()), cx, |s| {
                        s.select(selections);
                    });
                    this.insert("", cx);
                    cx.write_to_clipboard(ClipboardItem::new_string_with_json_metadata(
                        copied_text,
                        metadata,
                    ));
                });
            });

            cx.stop_propagation();
            return;
        }

        cx.propagate();
    }

    fn get_clipboard_contents(
        &mut self,
        cx: &mut ViewContext<Self>,
    ) -> (String, CopyMetadata, Vec<text::Selection<usize>>) {
        let (snapshot, selection, creases) = self.editor.update(cx, |editor, cx| {
            let mut selection = editor.selections.newest::<Point>(cx);
            let snapshot = editor.buffer().read(cx).snapshot(cx);

            let is_entire_line = selection.is_empty() || editor.selections.line_mode;
            if is_entire_line {
                selection.start = Point::new(selection.start.row, 0);
                selection.end =
                    cmp::min(snapshot.max_point(), Point::new(selection.start.row + 1, 0));
                selection.goal = SelectionGoal::None;
            }

            let selection_start = snapshot.point_to_offset(selection.start);

            (
                snapshot.clone(),
                selection.clone(),
                editor.display_map.update(cx, |display_map, cx| {
                    display_map
                        .snapshot(cx)
                        .crease_snapshot
                        .creases_in_range(
                            MultiBufferRow(selection.start.row)
                                ..MultiBufferRow(selection.end.row + 1),
                            &snapshot,
                        )
                        .filter_map(|crease| {
                            if let Some(metadata) = &crease.metadata {
                                let start = crease
                                    .range
                                    .start
                                    .to_offset(&snapshot)
                                    .saturating_sub(selection_start);
                                let end = crease
                                    .range
                                    .end
                                    .to_offset(&snapshot)
                                    .saturating_sub(selection_start);

                                let range_relative_to_selection = start..end;

                                if range_relative_to_selection.is_empty() {
                                    None
                                } else {
                                    Some(SelectedCreaseMetadata {
                                        range_relative_to_selection,
                                        crease: metadata.clone(),
                                    })
                                }
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                }),
            )
        });

        let selection = selection.map(|point| snapshot.point_to_offset(point));
        let context = self.context.read(cx);

        let mut text = String::new();
        for message in context.messages(cx) {
            if message.offset_range.start >= selection.range().end {
                break;
            } else if message.offset_range.end >= selection.range().start {
                let range = cmp::max(message.offset_range.start, selection.range().start)
                    ..cmp::min(message.offset_range.end, selection.range().end);
                if !range.is_empty() {
                    for chunk in context.buffer().read(cx).text_for_range(range) {
                        text.push_str(chunk);
                    }
                    if message.offset_range.end < selection.range().end {
                        text.push('\n');
                    }
                }
            }
        }

        (text, CopyMetadata { creases }, vec![selection])
    }

    fn paste(&mut self, action: &editor::actions::Paste, cx: &mut ViewContext<Self>) {
        cx.stop_propagation();

        let images = if let Some(item) = cx.read_from_clipboard() {
            item.into_entries()
                .filter_map(|entry| {
                    if let ClipboardEntry::Image(image) = entry {
                        Some(image)
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

        let metadata = if let Some(item) = cx.read_from_clipboard() {
            item.entries().first().and_then(|entry| {
                if let ClipboardEntry::String(text) = entry {
                    text.metadata_json::<CopyMetadata>()
                } else {
                    None
                }
            })
        } else {
            None
        };

        if images.is_empty() {
            self.editor.update(cx, |editor, cx| {
                let paste_position = editor.selections.newest::<usize>(cx).head();
                editor.paste(action, cx);

                if let Some(metadata) = metadata {
                    let buffer = editor.buffer().read(cx).snapshot(cx);

                    let mut buffer_rows_to_fold = BTreeSet::new();
                    let weak_editor = cx.view().downgrade();
                    editor.insert_creases(
                        metadata.creases.into_iter().map(|metadata| {
                            let start = buffer.anchor_after(
                                paste_position + metadata.range_relative_to_selection.start,
                            );
                            let end = buffer.anchor_before(
                                paste_position + metadata.range_relative_to_selection.end,
                            );

                            let buffer_row = MultiBufferRow(start.to_point(&buffer).row);
                            buffer_rows_to_fold.insert(buffer_row);
                            Crease::new(
                                start..end,
                                FoldPlaceholder {
                                    render: render_fold_icon_button(
                                        weak_editor.clone(),
                                        metadata.crease.icon,
                                        metadata.crease.label.clone(),
                                    ),
                                    constrain_width: false,
                                    merge_adjacent: false,
                                },
                                render_slash_command_output_toggle,
                                |_, _, _| Empty.into_any(),
                            )
                            .with_metadata(metadata.crease.clone())
                        }),
                        cx,
                    );
                    for buffer_row in buffer_rows_to_fold.into_iter().rev() {
                        editor.fold_at(&FoldAt { buffer_row }, cx);
                    }
                }
            });
        } else {
            let mut image_positions = Vec::new();
            self.editor.update(cx, |editor, cx| {
                editor.transact(cx, |editor, cx| {
                    let edits = editor
                        .selections
                        .all::<usize>(cx)
                        .into_iter()
                        .map(|selection| (selection.start..selection.end, "\n"));
                    editor.edit(edits, cx);

                    let snapshot = editor.buffer().read(cx).snapshot(cx);
                    for selection in editor.selections.all::<usize>(cx) {
                        image_positions.push(snapshot.anchor_before(selection.end));
                    }
                });
            });

            self.context.update(cx, |context, cx| {
                for image in images {
                    let Some(render_image) = image.to_image_data(cx).log_err() else {
                        continue;
                    };
                    let image_id = image.id();
                    let image_task = LanguageModelImage::from_image(image, cx).shared();

                    for image_position in image_positions.iter() {
                        context.insert_content(
                            Content::Image {
                                anchor: image_position.text_anchor,
                                image_id,
                                image: image_task.clone(),
                                render_image: render_image.clone(),
                            },
                            cx,
                        );
                    }
                }
            });
        }
    }

    fn update_image_blocks(&mut self, cx: &mut ViewContext<Self>) {
        self.editor.update(cx, |editor, cx| {
            let buffer = editor.buffer().read(cx).snapshot(cx);
            let excerpt_id = *buffer.as_singleton().unwrap().0;
            let old_blocks = std::mem::take(&mut self.blocks);
            let mut new_blocks = vec![];
            let mut block_index_to_message = vec![];

            let render_block = |message: MessageMetadata| -> RenderBlock {
                Box::new({
                    let context = self.context.clone();
                    move |cx| {
                        let message_id = MessageId(message.timestamp);
                        let show_spinner = message.role == Role::Assistant
                            && message.status == MessageStatus::Pending;

                        let label = match message.role {
                            Role::User => {
                                Label::new("You").color(Color::Default).into_any_element()
                            }
                            Role::Assistant => {
                                let label = Label::new("Assistant").color(Color::Info);
                                if show_spinner {
                                    label
                                        .with_animation(
                                            "pulsating-label",
                                            Animation::new(Duration::from_secs(2))
                                                .repeat()
                                                .with_easing(pulsating_between(0.4, 0.8)),
                                            |label, delta| label.alpha(delta),
                                        )
                                        .into_any_element()
                                } else {
                                    label.into_any_element()
                                }
                            }

                            Role::System => Label::new("System")
                                .color(Color::Warning)
                                .into_any_element(),
                        };

                        let sender = ButtonLike::new("role")
                            .style(ButtonStyle::Filled)
                            .child(label)
                            .tooltip(|cx| {
                                Tooltip::with_meta(
                                    "Toggle message role",
                                    None,
                                    "Available roles: You (User), Assistant, System",
                                    cx,
                                )
                            })
                            .on_click({
                                let context = context.clone();
                                move |_, cx| {
                                    context.update(cx, |context, cx| {
                                        context.cycle_message_roles(
                                            HashSet::from_iter(Some(message_id)),
                                            cx,
                                        )
                                    })
                                }
                            });

                        h_flex()
                            .id(("message_header", message_id.as_u64()))
                            .pl(cx.gutter_dimensions.full_width())
                            .h_11()
                            .w_full()
                            .relative()
                            .gap_1()
                            .child(sender)
                            .children(match &message.cache {
                                Some(cache) if cache.is_final_anchor => match cache.status {
                                    CacheStatus::Cached => Some(
                                        div()
                                            .id("cached")
                                            .child(
                                                Icon::new(IconName::DatabaseZap)
                                                    .size(IconSize::XSmall)
                                                    .color(Color::Hint),
                                            )
                                            .tooltip(|cx| {
                                                Tooltip::with_meta(
                                                    "Context cached",
                                                    None,
                                                    "Large messages cached to optimize performance",
                                                    cx,
                                                )
                                            })
                                            .into_any_element(),
                                    ),
                                    CacheStatus::Pending => Some(
                                        div()
                                            .child(
                                                Icon::new(IconName::Ellipsis)
                                                    .size(IconSize::XSmall)
                                                    .color(Color::Hint),
                                            )
                                            .into_any_element(),
                                    ),
                                },
                                _ => None,
                            })
                            .into_any_element()
                    }
                })
            };
            for message in self.context.read(cx).messages(cx) {
                // This is a new message.
                new_blocks.push(create_block_properties(&message));
                block_index_to_message.push((message.id, MessageMetadata::from(&message)));
            }
            let ids = editor.insert_blocks(new_blocks, None, cx);
            old_blocks.extend(ids.into_iter().zip(block_index_to_message).map(
                |(block_id, (message_id, message_meta))| (message_id, (message_meta, block_id)),
            ));
            self.blocks = old_blocks;
        });
    }

    /// Returns either the selected text, or the content of the Markdown code
    /// block surrounding the cursor.
    fn get_selection_or_code_block(
        context_editor_view: &View<ContextEditor>,
        cx: &mut ViewContext<Workspace>,
    ) -> Option<(String, bool)> {
        const CODE_FENCE_DELIMITER: &'static str = "```";

        let context_editor = context_editor_view.read(cx).editor.read(cx);

        if context_editor.selections.newest::<Point>(cx).is_empty() {
            let snapshot = context_editor.buffer().read(cx).snapshot(cx);
            let head = context_editor.selections.newest::<Point>(cx).head();
            let offset = snapshot.point_to_offset(head);

            let surrounding_code_block_range = find_surrounding_code_block(snapshot, offset)?;
            let mut text = snapshot
                .text_for_range(surrounding_code_block_range)
                .collect::<String>();

            // If there is no newline trailing the closing three-backticks, then
            // tree-sitter-md extends the range of the content node to include
            // the backticks.
            if text.ends_with(CODE_FENCE_DELIMITER) {
                text.drain((text.len() - CODE_FENCE_DELIMITER.len())..);
            }

            (!text.is_empty()).then_some((text, true))
        } else {
            let anchor = context_editor.selections.newest_anchor();
            let text = context_editor
                .buffer()
                .read(cx)
                .read(cx)
                .text_for_range(anchor.range())
                .collect::<String>();

            (!text.is_empty()).then_some((text, false))
        }
    }

    fn insert_selection(
        workspace: &mut Workspace,
        _: &InsertIntoEditor,
        cx: &mut ViewContext<Workspace>,
    ) {
        let Some(panel) = workspace.panel::<AssistantPanel>(cx) else {
            return;
        };
        let Some(context_editor_view) = panel.read(cx).active_context_editor(cx) else {
            return;
        };
        let Some(active_editor_view) = workspace
            .active_item(cx)
            .and_then(|item| item.act_as::<Editor>(cx))
        else {
            return;
        };

        if let Some((text, _)) = Self::get_selection_or_code_block(&context_editor_view, cx) {
            active_editor_view.update(cx, |editor, cx| {
                editor.insert(&text, cx);
                editor.focus(cx);
            })
        }
    }

    fn copy_code(workspace: &mut Workspace, _: &CopyCode, cx: &mut ViewContext<Workspace>) {
        let result = maybe!({
            let panel = workspace.panel::<AssistantPanel>(cx)?;
            let context_editor_view = panel.read(cx).active_context_editor(cx)?;
            Self::get_selection_or_code_block(&context_editor_view, cx)
        });
        let Some((text, is_code_block)) = result else {
            return;
        };

        cx.write_to_clipboard(ClipboardItem::new_string(text));

        struct CopyToClipboardToast;
        workspace.show_toast(
            Toast::new(
                NotificationId::unique::<CopyToClipboardToast>(),
                format!(
                    "{} copied to clipboard.",
                    if is_code_block {
                        "Code block"
                    } else {
                        "Selection"
                    }
                ),
            )
            .autohide(),
            cx,
        );
    }

    fn insert_dragged_files(
        workspace: &mut Workspace,
        action: &InsertDraggedFiles,
        cx: &mut ViewContext<Workspace>,
    ) {
        let Some(panel) = workspace.panel::<AssistantPanel>(cx) else {
            return;
        };
        let Some(context_editor_view) = panel.read(cx).active_context_editor(cx) else {
            return;
        };

        let project = workspace.project().clone();

        let paths = match action {
            InsertDraggedFiles::ProjectPaths(paths) => Task::ready((paths.clone(), vec![])),
            InsertDraggedFiles::ExternalFiles(paths) => {
                let tasks = paths
                    .clone()
                    .into_iter()
                    .map(|path| Workspace::project_path_for_path(project.clone(), &path, false, cx))
                    .collect::<Vec<_>>();

                cx.spawn(move |_, cx| async move {
                    let mut paths = vec![];
                    let mut worktrees = vec![];

                    let opened_paths = futures::future::join_all(tasks).await;
                    for (worktree, project_path) in opened_paths.into_iter().flatten() {
                        let Ok(worktree_root_name) =
                            worktree.read_with(&cx, |worktree, _| worktree.root_name().to_string())
                        else {
                            continue;
                        };

                        let mut full_path = PathBuf::from(worktree_root_name.clone());
                        full_path.push(&project_path.path);
                        paths.push(full_path);
                        worktrees.push(worktree);
                    }

                    (paths, worktrees)
                })
            }
        };

        cx.spawn(|_, mut cx| async move {
            let (paths, dragged_file_worktrees) = paths.await;
            let cmd_name = file_command::FileSlashCommand.name();

            context_editor_view
                .update(&mut cx, |context_editor, cx| {
                    let file_argument = paths
                        .into_iter()
                        .map(|path| path.to_string_lossy().to_string())
                        .collect::<Vec<_>>()
                        .join(" ");

                    context_editor.editor.update(cx, |editor, cx| {
                        editor.insert("\n", cx);
                        editor.insert(&format!("/{} {}", cmd_name, file_argument), cx);
                    });

                    context_editor.confirm_command(&ConfirmCommand, cx);

                    context_editor
                        .dragged_file_worktrees
                        .extend(dragged_file_worktrees);
                })
                .log_err();
        })
        .detach();
    }

    fn quote_selection(
        workspace: &mut Workspace,
        _: &QuoteSelection,
        cx: &mut ViewContext<Workspace>,
    ) {
        let Some(panel) = workspace.panel::<AssistantPanel>(cx) else {
            return;
        };
        let Some(editor) = workspace
            .active_item(cx)
            .and_then(|item| item.act_as::<Editor>(cx))
        else {
            return;
        };

        let mut creases = vec![];
        editor.update(cx, |editor, cx| {
            let selections = editor.selections.all_adjusted(cx);
            let buffer = editor.buffer().read(cx).snapshot(cx);
            for selection in selections {
                let range = editor::ToOffset::to_offset(&selection.start, &buffer)
                    ..editor::ToOffset::to_offset(&selection.end, &buffer);
                let selected_text = buffer.text_for_range(range.clone()).collect::<String>();
                if selected_text.is_empty() {
                    continue;
                }
                let start_language = buffer.language_at(range.start);
                let end_language = buffer.language_at(range.end);
                let language_name = if start_language == end_language {
                    start_language.map(|language| language.code_fence_block_name())
                } else {
                    None
                };
                let language_name = language_name.as_deref().unwrap_or("");
                let filename = buffer
                    .file_at(selection.start)
                    .map(|file| file.full_path(cx));
                let text = if language_name == "markdown" {
                    selected_text
                        .lines()
                        .map(|line| format!("> {}", line))
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    let start_symbols = buffer
                        .symbols_containing(selection.start, None)
                        .map(|(_, symbols)| symbols);
                    let end_symbols = buffer
                        .symbols_containing(selection.end, None)
                        .map(|(_, symbols)| symbols);

                    let outline_text = if let Some((start_symbols, end_symbols)) =
                        start_symbols.zip(end_symbols)
                    {
                        Some(
                            start_symbols
                                .into_iter()
                                .zip(end_symbols)
                                .take_while(|(a, b)| a == b)
                                .map(|(a, _)| a.text)
                                .collect::<Vec<_>>()
                                .join(" > "),
                        )
                    } else {
                        None
                    };

                    let line_comment_prefix = start_language
                        .and_then(|l| l.default_scope().line_comment_prefixes().first().cloned());

                    let fence = codeblock_fence_for_path(
                        filename.as_deref(),
                        Some(selection.start.row..=selection.end.row),
                    );

                    if let Some((line_comment_prefix, outline_text)) =
                        line_comment_prefix.zip(outline_text)
                    {
                        let breadcrumb =
                            format!("{line_comment_prefix}Excerpt from: {outline_text}\n");
                        format!("{fence}{breadcrumb}{selected_text}\n```")
                    } else {
                        format!("{fence}{selected_text}\n```")
                    }
                };
                let crease_title = if let Some(path) = filename {
                    let start_line = selection.start.row + 1;
                    let end_line = selection.end.row + 1;
                    if start_line == end_line {
                        format!("{}, Line {}", path.display(), start_line)
                    } else {
                        format!("{}, Lines {} to {}", path.display(), start_line, end_line)
                    }
                } else {
                    "Quoted selection".to_string()
                };
                creases.push((text, crease_title));
            }
        });
        if creases.is_empty() {
            return;
        }
        // Activate the panel
        if !panel.focus_handle(cx).contains_focused(cx) {
            workspace.toggle_panel_focus::<AssistantPanel>(cx);
        }

        panel.update(cx, |_, cx| {
            // Wait to create a new context until the workspace is no longer
            // being updated.
            cx.defer(move |panel, cx| {
                if let Some(context) = panel
                    .active_context_editor(cx)
                    .or_else(|| panel.new_context(cx))
                {
                    context.update(cx, |context, cx| {
                        context.editor.update(cx, |editor, cx| {
                            editor.insert("\n", cx);
                            for (text, crease_title) in creases {
                                let point = editor.selections.newest::<Point>(cx).head();
                                let start_row = MultiBufferRow(point.row);

                                editor.insert(&text, cx);

                                let snapshot = editor.buffer().read(cx).snapshot(cx);
                                let anchor_before = snapshot.anchor_after(point);
                                let anchor_after = editor
                                    .selections
                                    .newest_anchor()
                                    .head()
                                    .bias_left(&snapshot);

                                editor.insert("\n", cx);

                                let fold_placeholder = quote_selection_fold_placeholder(
                                    crease_title,
                                    cx.view().downgrade(),
                                );
                                let crease = Crease::new(
                                    anchor_before..anchor_after,
                                    fold_placeholder,
                                    render_quote_selection_output_toggle,
                                    |_, _, _| Empty.into_any(),
                                );
                                editor.insert_creases(vec![crease], cx);
                                editor.fold_at(
                                    &FoldAt {
                                        buffer_row: start_row,
                                    },
                                    cx,
                                );
                            }
                        })
                    });
                };
            });
        });
    }

    fn copy(&mut self, _: &editor::actions::Copy, cx: &mut ViewContext<Self>) {
        if self.editor.read(cx).selections.count() == 1 {
            let (copied_text, metadata, _) = self.get_clipboard_contents(cx);
            cx.write_to_clipboard(ClipboardItem::new_string_with_json_metadata(
                copied_text,
                metadata,
            ));
            cx.stop_propagation();
            return;
        }

        cx.propagate();
    }

    fn cut(&mut self, _: &editor::actions::Cut, cx: &mut ViewContext<Self>) {
        if self.editor.read(cx).selections.count() == 1 {
            let (copied_text, metadata, selections) = self.get_clipboard_contents(cx);

            self.editor.update(cx, |editor, cx| {
                editor.transact(cx, |this, cx| {
                    this.change_selections(Some(Autoscroll::fit()), cx, |s| {
                        s.select(selections);
                    });
                    this.insert("", cx);
                    cx.write_to_clipboard(ClipboardItem::new_string_with_json_metadata(
                        copied_text,
                        metadata,
                    ));
                });
            });

            cx.stop_propagation();
            return;
        }

        cx.propagate();
    }

    fn get_clipboard_contents(
        &mut self,
        cx: &mut ViewContext<Self>,
    ) -> (String, CopyMetadata, Vec<text::Selection<usize>>) {
        let (snapshot, selection, creases) = self.editor.update(cx, |editor, cx| {
            let mut selection = editor.selections.newest::<Point>(cx);
            let snapshot = editor.buffer().read(cx).snapshot(cx);

            let is_entire_line = selection.is_empty() || editor.selections.line_mode;
            if is_entire_line {
                selection.start = Point::new(selection.start.row, 0);
                selection.end =
                    cmp::min(snapshot.max_point(), Point::new(selection.start.row + 1, 0));
                selection.goal = SelectionGoal::None;
            }

            let selection_start = snapshot.point_to_offset(selection.start);

            (
                snapshot.clone(),
                selection.clone(),
                editor.display_map.update(cx, |display_map, cx| {
                    display_map
                        .snapshot(cx)
                        .crease_snapshot
                        .creases_in_range(
                            MultiBufferRow(selection.start.row)
                                ..MultiBufferRow(selection.end.row + 1),
                            &snapshot,
                        )
                        .filter_map(|crease| {
                            if let Some(metadata) = &crease.metadata {
                                let start = crease
                                    .range
                                    .start
                                    .to_offset(&snapshot)
                                    .saturating_sub(selection_start);
                                let end = crease
                                    .range
                                    .end
                                    .to_offset(&snapshot)
                                    .saturating_sub(selection_start);

                                let range_relative_to_selection = start..end;

                                if range_relative_to_selection.is_empty() {
                                    None
                                } else {
                                    Some(SelectedCreaseMetadata {
                                        range_relative_to_selection,
                                        crease: metadata.clone(),
                                    })
                                }
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                }),
            )
        });

        let selection = selection.map(|point| snapshot.point_to_offset(point));
        let context = self.context.read(cx);

        let mut text = String::new();
        for message in context.messages(cx) {
            if message.offset_range.start >= selection.range().end {
                break;
            } else if message.offset_range.end >= selection.range().start {
                let range = cmp::max(message.offset_range.start, selection.range().start)
                    ..cmp::min(message.offset_range.end, selection.range().end);
                if !range.is_empty() {
                    for chunk in context.buffer().read(cx).text_for_range(range) {
                        text.push_str(chunk);
                    }
                    if message.offset_range.end < selection.range().end {
                        text.push('\n');
                    }
                }
            }
        }

        (text, CopyMetadata { creases }, vec![selection])
    }

    fn paste(&mut self, action: &editor::actions::Paste, cx: &mut ViewContext<Self>) {
        cx.stop_propagation();

        let images = if let Some(item) = cx.read_from_clipboard() {
            item.into_entries()
                .filter_map(|entry| {
                    if let ClipboardEntry::Image(image) = entry {
                        Some(image)
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

        let metadata = if let Some(item) = cx.read_from_clipboard() {
            item.entries().first().and_then(|entry| {
                if let ClipboardEntry::String(text) = entry {
                    text.metadata_json::<CopyMetadata>()
                } else {
                    None
                }
            })
        } else {
            None
        };

        if images.is_empty() {
            self.editor.update(cx, |editor, cx| {
                let paste_position = editor.selections.newest::<usize>(cx).head();
                editor.paste(action, cx);

                if let Some(metadata) = metadata {
                    let buffer = editor.buffer().read(cx).snapshot(cx);

                    let mut buffer_rows_to_fold = BTreeSet::new();
                    let weak_editor = cx.view().downgrade();
                    editor.insert_creases(
                        metadata.creases.into_iter().map(|metadata| {
                            let start = buffer.anchor_after(
                                paste_position + metadata.range_relative_to_selection.start,
                            );
                            let end = buffer.anchor_before(
                                paste_position + metadata.range_relative_to_selection.end,
                            );

                            let buffer_row = MultiBufferRow(start.to_point(&buffer).row);
                            buffer_rows_to_fold.insert(buffer_row);
                            Crease::new(
                                start..end,
                                FoldPlaceholder {
                                    render: render_fold_icon_button(
                                        weak_editor.clone(),
                                        metadata.crease.icon,
                                        metadata.crease.label.clone(),
                                    ),
                                    constrain_width: false,
                                    merge_adjacent: false,
                                },
                                render_slash_command_output_toggle,
                                |_, _, _| Empty.into_any(),
                            )
                            .with_metadata(metadata.crease.clone())
                        }),
                        cx,
                    );
                    for buffer_row in buffer_rows_to_fold.into_iter().rev() {
                        editor.fold_at(&FoldAt { buffer_row }, cx);
                    }
                }
            });
        } else {
            let mut image_positions = Vec::new();
            self.editor.update(cx, |editor, cx| {
                editor.transact(cx, |editor, cx| {
                    let edits = editor
                        .selections
                        .all::<usize>(cx)
                        .into_iter()
                        .map(|selection| (selection.start..selection.end, "\n"));
                    editor.edit(edits, cx);

                    let snapshot = editor.buffer().read(cx).snapshot(cx);
                    for selection in editor.selections.all::<usize>(cx) {
                        image_positions.push(snapshot.anchor_before(selection.end));
                    }
                });
            });

            self.context.update(cx, |context, cx| {
                for image in images {
                    let Some(render_image) = image.to_image_data(cx).log_err() else {
                        continue;
                    };
                    let image_id = image.id();
                    let image_task = LanguageModelImage::from_image(image, cx).shared();

                    for image_position in image_positions.iter() {
                        context.insert_content(
                            Content::Image {
                                anchor: image_position.text_anchor,
                                image_id,
                                image: image_task.clone(),
                                render_image: render_image.clone(),
                            },
                            cx,
                        );
                    }
                }
            });
        }
    }

    fn update_image_blocks(&mut self, cx: &mut ViewContext<Self>) {
        self.editor.update(cx, |editor, cx| {
            let buffer = editor.buffer().read(cx).snapshot(cx);
            let excerpt_id = *buffer.as_singleton().unwrap().0;
            let old_blocks = std::mem::take(&mut self.blocks);
            let mut new_blocks = vec![];
            let mut block_index_to_message = vec![];

            let render_block = |message: MessageMetadata| -> RenderBlock {
                Box::new({
                    let context = self.context.clone();
                    move |cx| {
                        let message_id = MessageId(message.timestamp);
                        let show_spinner = message.role == Role::Assistant
                            && message.status == MessageStatus::Pending;

                        let label = match message.role {
                            Role::User => {
                                Label::new("You").color(Color::Default).into_any_element()
                            }
                            Role::Assistant => {
                                let label = Label::new("Assistant").color(Color::Info);
                                if show_spinner {
                                    label
                                        .with_animation(
                                            "pulsating-label",
                                            Animation::new(Duration::from_secs(2))
                                                .repeat()
                                                .with_easing(pulsating_between(0.4, 0.8)),
                                            |label, delta| label.alpha(delta),
                                        )
                                        .into_any_element()
                                } else {
                                    label.into_any_element()
                                }
                            }

                            Role::System => Label::new("System")
                                .color(Color::Warning)
                                .into_any_element(),
                        };

                        let sender = ButtonLike::new("role")
                            .style(ButtonStyle::Filled)
                            .child(label)
                            .tooltip(|cx| {
                                Tooltip::with_meta(
                                    "Toggle message role",
                                    None,
                                    "Available roles: You (User), Assistant, System",
                                    cx,
                                )
                            })
                            .on_click({
                                let context = context.clone();
                                move |_, cx| {
                                    context.update(cx, |context, cx| {
                                        context.cycle_message_roles(
                                            HashSet::from_iter(Some(message_id)),
                                            cx,
                                        )
                                    })
                                }
                            });

                        h_flex()
                            .id(("message_header", message_id.as_u64()))
                            .pl(cx.gutter_dimensions.full_width())
                            .h_11()
                            .w_full()
                            .relative()
                            .gap_1()
                            .child(sender)
                            .children(match &message.cache {
                                Some(cache) if cache.is_final_anchor => match cache.status {
                                    CacheStatus::Cached => Some(
                                        div()
                                            .id("cached")
                                            .child(
                                                Icon::new(IconName::DatabaseZap)
                                                    .size(IconSize::XSmall)
                                                    .color(Color::Hint),
                                            )
                                            .tooltip(|cx| {
                                                Tooltip::with_meta(
                                                    "Context cached",
                                                    None,
                                                    "Large messages cached to optimize performance",
                                                    cx,
                                                )
                                            })
                                            .into_any_element(),
                                    ),
                                    CacheStatus::Pending => Some(
                                        div()
                                            .child(
                                                Icon::new(IconName::Ellipsis)
                                                    .size(IconSize::XSmall)
                                                    .color(Color::Hint),
                                            )
                                            .into_any_element(),
                                    ),
                                },
                                _ => None,
                            })
                            .into_any_element()
                    }
                })
            };
            for message in self.context.read(cx).messages(cx) {
                // This is a new message.
                new_blocks.push(create_block_properties(&message));
                block_index_to_message.push((message.id, MessageMetadata::from(&message)));
            }
            let ids = editor.insert_blocks(new_blocks, None, cx);
            old_blocks.extend(ids.into_iter().zip(block_index_to_message).map(
                |(block_id, (message_id, message_meta))| (message_id, (message_meta, block_id)),
            ));
            self.blocks = old_blocks;
        });
    }

    /// Returns either the selected text, or the content of the Markdown code
    /// block surrounding the cursor.
    fn get_selection_or_code_block(
        context_editor_view: &View<ContextEditor>,
        cx: &mut ViewContext<Workspace>,
    ) -> Option<(String, bool)> {
        const CODE_FENCE_DELIMITER: &'static str = "```";

        let context_editor = context_editor_view.read(cx).editor.read(cx);

        if context_editor.selections.newest::<Point>(cx).is_empty() {
            let snapshot = context_editor.buffer().read(cx).snapshot(cx);
            let head = context_editor.selections.newest::<Point>(cx).head();
            let offset = snapshot.point_to_offset(head);

            let surrounding_code_block_range = find_surrounding_code_block(snapshot, offset)?;
            let mut text = snapshot
                .text_for_range(surrounding_code_block_range)
                .collect::<String>();

            // If there is no newline trailing the closing three-backticks, then
            // tree-sitter-md extends the range of the content node to include
            // the backticks.
            if text.ends_with(CODE_FENCE_DELIMITER) {
                text.drain((text.len() - CODE_FENCE_DELIMITER.len())..);
            }

            (!text.is_empty()).then_some((text, true))
        } else {
            let anchor = context_editor.selections.newest_anchor();
            let text = context_editor
                .buffer()
                .read(cx)
                .read(cx)
                .text_for_range(anchor.range())
                .collect::<String>();

            (!text.is_empty()).then_some((text, false))
        }
    }

    fn insert_selection(
        workspace: &mut Workspace,
        _: &InsertIntoEditor,
        cx: &mut ViewContext<Workspace>,
    ) {
        let Some(panel) = workspace.panel::<AssistantPanel>(cx) else {
            return;
        };
        let Some(context_editor_view) = panel.read(cx).active_context_editor(cx) else {
            return;
        };
        let Some(active_editor_view) = workspace
            .active_item(cx)
            .and_then(|item| item.act_as::<Editor>(cx))
        else {
            return;
        };

        if let Some((text, _)) = Self::get_selection_or_code_block(&context_editor_view, cx) {
            active_editor_view.update(cx, |editor, cx| {
                editor.insert(&text, cx);
                editor.focus(cx);
            })
        }
    }

    fn copy_code(workspace: &mut Workspace, _: &CopyCode, cx: &mut ViewContext<Workspace>) {
        let result = maybe!({
            let panel = workspace.panel::<AssistantPanel>(cx)?;
            let context_editor_view = panel.read(cx).active_context_editor(cx)?;
            Self::get_selection_or_code_block(&context_editor_view, cx)
        });
        let Some((text, is_code_block)) = result else {
            return;
        };

        cx.write_to_clipboard(ClipboardItem::new_string(text));

        struct CopyToClipboardToast;
        workspace.show_toast(
            Toast::new(
                NotificationId::unique::<CopyToClipboardToast>(),
                format!(
                    "{} copied to clipboard.",
                    if is_code_block {
                        "Code block"
                    } else {
                        "Selection"
                    }
                ),
            )
            .autohide(),
            cx,
        );
    }

    fn insert_dragged_files(
        workspace: &mut Workspace,
        action: &InsertDraggedFiles,
        cx: &mut ViewContext<Workspace>,
    ) {
        let Some(panel) = workspace.panel::<AssistantPanel>(cx) else {
            return;
        };
        let Some(context_editor_view) = panel.read(cx).active_context_editor(cx) else {
            return;
        };

        let project = workspace.project().clone();

        let paths = match action {
            InsertDraggedFiles::ProjectPaths(paths) => Task::ready((paths.clone(), vec![])),
            InsertDraggedFiles::ExternalFiles(paths) => {
                let tasks = paths
                    .clone()
                    .into_iter()
                    .map(|path| Workspace::project_path_for_path(project.clone(), &path, false, cx))
                    .collect::<Vec<_>>();

                cx.spawn(move |_, cx| async move {
                    let mut paths = vec![];
                    let mut worktrees = vec![];

                    let opened_paths = futures::future::join_all(tasks).await;
                    for (worktree, project_path) in opened_paths.into_iter().flatten() {
                        let Ok(worktree_root_name) =
                            worktree.read_with(&cx, |worktree, _| worktree.root_name().to_string())
                        else {
                            continue;
                        };

                        let mut full_path = PathBuf::from(worktree_root_name.clone());
                        full_path.push(&project_path.path);
                        paths.push(full_path);
                        worktrees.push(worktree);
                    }

                    (paths, worktrees)
                })
            }
        };

        cx.spawn(|_, mut cx| async move {
            let (paths, dragged_file_worktrees) = paths.await;
            let cmd_name = file_command::FileSlashCommand.name();

            context_editor_view
                .update(&mut cx, |context_editor, cx| {
                    let file_argument = paths
                        .into_iter()
                        .map(|path| path.to_string_lossy().to_string())
                        .collect::<Vec<_>>()
                        .join(" ");

                    context_editor.editor.update(cx, |editor, cx| {
                        editor.insert("\n", cx);
                        editor.insert(&format!("/{} {}", cmd_name, file_argument), cx);
                    });

                    context_editor.confirm_command(&ConfirmCommand, cx);

                    context_editor
                        .dragged_file_worktrees
                        .extend(dragged_file_worktrees);
                })
                .log_err();
        })
        .detach();
    }

    fn quote_selection(
        workspace: &mut Workspace,
        _: &QuoteSelection,
        cx: &mut ViewContext<Workspace>,
    ) {
        let Some(panel) = workspace.panel::<AssistantPanel>(cx) else {
            return;
        };
        let Some(editor) = workspace
            .active_item(cx)
            .and_then(|item| item.act_as::<Editor>(cx))
        else {
            return;
        };

        let mut creases = vec![];
        editor.update(cx, |editor, cx| {
            let selections = editor.selections.all_adjusted(cx);
            let buffer = editor.buffer().read(cx).snapshot(cx);
            for selection in selections {
                let range = editor::ToOffset::to_offset(&selection.start, &buffer)
                    ..editor::ToOffset::to_offset(&selection.end, &buffer);
                let selected_text = buffer.text_for_range(range.clone()).collect::<String>();
                if selected_text.is_empty() {
                    continue;
                }
                let start_language = buffer.language_at(range.start);
                let end_language = buffer.language_at(range.end);
                let language_name = if start_language == end_language {
                    start_language.map(|language| language.code_fence_block_name())
                } else {
                    None
                };
                let language_name = language_name.as_deref().unwrap_or("");
                let filename = buffer
                    .file_at(selection.start)
                    .map(|file| file.full_path(cx));
                let text = if language_name == "markdown" {
                    selected_text
                        .lines()
                        .map(|line| format!("> {}", line))
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    let start_symbols = buffer
                        .symbols_containing(selection.start, None)
                        .map(|(_, symbols)| symbols);
                    let end_symbols = buffer
                        .symbols_containing(selection.end, None)
                        .map(|(_, symbols)| symbols);

                    let outline_text = if let Some((start_symbols, end_symbols)) =
                        start_symbols.zip(end_symbols)
                    {
                        Some(
                            start_symbols
                                .into_iter()
                                .zip(end_symbols)
                                .take_while(|(a, b)| a == b)
                                .map(|(a, _)| a.text)
                                .collect::<Vec<_>>()
                                .join(" > "),
                        )
                    } else {
                        None
                    };

                    let line_comment_prefix = start_language
                        .and_then(|l| l.default_scope().line_comment_prefixes().first().cloned());

                    let fence = codeblock_fence_for_path(
                        filename.as_deref(),
                        Some(selection.start.row..=selection.end.row),
                    );

                    if let Some((line_comment_prefix, outline_text)) =
                        line_comment_prefix.zip(outline_text)
                    {
                        let breadcrumb =
                            format!("{line_comment_prefix}Excerpt from: {outline_text}\n");
                        format!("{fence}{breadcrumb}{selected_text}\n```")
                    } else {
                        format!("{fence}{selected_text}\n```")
                    }
                };
                let crease_title = if let Some(path) = filename {
                    let start_line = selection.start.row + 1;
                    let end_line = selection.end.row + 1;
                    if start_line == end_line {
                        format!("{}, Line {}", path.display(), start_line)
                    } else {
                        format!("{}, Lines {} to {}", path.display(), start_line, end_line)
                    }
                } else {
                    "Quoted selection".to_string()
                };
                creases.push((text, crease_title));
            }
        });
        if creases.is_empty() {
            return;
        }
        // Activate the panel
        if !panel.focus_handle(cx).contains_focused(cx) {
            workspace.toggle_panel_focus::<AssistantPanel>(cx);
        }

        panel.update(cx, |_, cx| {
            // Wait to create a new context until the workspace is no longer
            // being updated.
            cx.defer(move |panel, cx| {
                if let Some(context) = panel
                    .active_context_editor(cx)
                    .or_else(|| panel.new_context(cx))
                {
                    context.update(cx, |context, cx| {
                        context.editor.update(cx, |editor, cx| {
                            editor.insert("\n", cx);
                            for (text, crease_title) in creases {
                                let point = editor.selections.newest::<Point>(cx).head();
                                let start_row = MultiBufferRow(point.row);

                                editor.insert(&text, cx);

                                let snapshot = editor.buffer().read(cx).snapshot(cx);
                                let anchor_before = snapshot.anchor_after(point);
                                let anchor_after = editor
                                    .selections
                                    .newest_anchor()
                                    .head()
                                    .bias_left(&snapshot);

                                editor.insert("\n", cx);

                                let fold_placeholder = quote_selection_fold_placeholder(
                                    crease_title,
                                    cx.view().downgrade(),
                                );
                                let crease = Crease::new(
                                    anchor_before..anchor_after,
                                    fold_placeholder,
                                    render_quote_selection_output_toggle,
                                    |_, _, _| Empty.into_any(),
                                );
                                editor.insert_creases(vec![crease], cx);
                                editor.fold_at(
                                    &FoldAt {
                                        buffer_row: start_row,
                                    },
                                    cx,
                                );
                            }
                        })
                    });
                };
            });
        });
    }

    fn copy(&mut self, _: &editor::actions::Copy, cx: &mut ViewContext<Self>) {
        if self.editor.read(cx).selections.count() == 1 {
            let (copied_text, metadata, _) = self.get_clipboard_contents(cx);
            cx.write_to_clipboard(ClipboardItem::new_string_with_json_metadata(
                copied_text,
                metadata,
            ));
            cx.stop_propagation();
            return;
        }

        cx.propagate();
    }

    fn cut(&mut self, _: &editor::actions::Cut, cx: &mut ViewContext<Self>) {
        if self.editor.read(cx).selections.count() == 1 {
            let (copied_text, metadata, selections) = self.get_clipboard_contents(cx);

            self.editor.update(cx, |editor, cx| {
                editor.transact(cx, |this, cx| {
                    this.change_selections(Some(Autoscroll::fit()), cx, |s| {
                        s.select(selections);
                    });
                    this.insert("", cx);
                    cx.write_to_clipboard(ClipboardItem::new_string_with_json_metadata(
                        copied_text,
                        metadata,
                    ));
                });
            });

            cx.stop_propagation();
            return;
        }

        cx.propagate();
    }

    fn get_clipboard_contents(
        &mut self,
        cx: &mut ViewContext<Self>,
    ) -> (String, CopyMetadata, Vec<text::Selection<usize>>) {
        let (snapshot, selection, creases) = self.editor.update(cx, |editor, cx| {
            let mut selection = editor.selections.newest::<Point>(cx);
            let snapshot = editor.buffer().read(cx).snapshot(cx);

            let is_entire_line = selection.is_empty() || editor.selections.line_mode;
            if is_entire_line {
                selection.start = Point::new(selection.start.row, 0);
                selection.end =
                    cmp::min(snapshot.max_point(), Point::new(selection.start.row + 1, 0));
                selection.goal = SelectionGoal::None;
            }

            let selection_start = snapshot.point_to_offset(selection.start);

            (
                snapshot.clone(),
                selection.clone(),
                editor.display_map.update(cx, |display_map, cx| {
                    display_map
                        .snapshot(cx)
                        .crease_snapshot
                        .creases_in_range(
                            MultiBufferRow(selection.start.row)
                                ..MultiBufferRow(selection.end.row + 1),
                            &snapshot,
                        )
                        .filter_map(|crease| {
                            if let Some(metadata) = &crease.metadata {
                                let start = crease
                                    .range
                                    .start
                                    .to_offset(&snapshot)
                                    .saturating_sub(selection_start);
                                let end = crease
                                    .range
                                    .end
                                    .to_offset(&snapshot)
                                    .saturating_sub(selection_start);

                                let range_relative_to_selection = start..end;

                                if range_relative_to_selection.is_empty() {
                                    None
                                } else {
                                    Some(SelectedCreaseMetadata {
                                        range_relative_to_selection,
                                        crease: metadata.clone(),
                                    })
                                }
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                }),
            )
        });

        let selection = selection.map(|point| snapshot.point_to_offset(point));
        let context = self.context.read(cx);

        let mut text = String::new();
        for message in context.messages(cx) {
            if message.offset_range.start >= selection.range().end {
                break;
            } else if message.offset_range.end >= selection.range().start {
                let range = cmp::max(message.offset_range.start, selection.range().start)
                    ..cmp::min(message.offset_range.end, selection.range().end);
                if !range.is_empty() {
                    for chunk in context.buffer().read(cx).text_for_range(range) {
                        text.push_str(chunk);
                    }
                    if message.offset_range.end < selection.range().end {
                        text.push('\n');
                    }
                }
            }
        }

        (text, CopyMetadata { creases }, vec![selection])
    }

    fn paste(&mut self, action: &editor::actions::Paste, cx: &mut ViewContext<Self>) {
        cx.stop_propagation();

        let images = if let Some(item) = cx.read_from_clipboard() {
            item.into_entries()
                .filter_map(|entry| {
                    if let ClipboardEntry::Image(image) = entry {
                        Some(image)
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

        let metadata = if let Some(item) = cx.read_from_clipboard() {
            item.entries().first().and_then(|entry| {
                if let ClipboardEntry::String(text) = entry {
                    text.metadata_json::<CopyMetadata>()
                } else {
                    None
                }
            })
        } else {
            None
        };

        if images.is_empty() {
            self.editor.update(cx, |editor, cx| {
                let paste_position = editor.selections.newest::<usize>(cx).head();
                editor.paste(action, cx);

                if let Some(metadata) = metadata {
                    let buffer = editor.buffer().read(cx).snapshot(cx);

                    let mut buffer_rows_to_fold = BTreeSet::new();
                    let weak_editor = cx.view().downgrade();
                    editor.insert_creases(
                        metadata.creases.into_iter().map(|metadata| {
                            let start = buffer.anchor_after(
                                paste_position + metadata.range_relative_to_selection.start,
                            );
                            let end = buffer.anchor_before(
                                paste_position + metadata.range_relative_to_selection.end,
                            );

                            let buffer_row = MultiBufferRow(start.to_point(&buffer).row);
                            buffer_rows_to_fold.insert(buffer_row);
                            Crease::new(
                                start..end,
                                FoldPlaceholder {
                                    render: render_fold_icon_button(
                                        weak_editor.clone(),
                                        metadata.crease.icon,
                                        metadata.crease.label.clone(),
                                    ),
                                    constrain_width: false,
                                    merge_adjacent: false,
                                },
                                render_slash_command_output_toggle,
                                |_, _, _| Empty.into_any(),
                            )
                            .with_metadata(metadata.crease.clone())
                        }),
                        cx,
                    );
                    for buffer_row in buffer_rows_to_fold.into_iter().rev() {
                        editor.fold_at(&FoldAt { buffer_row }, cx);
                    }
                }
            });
        } else {
            let mut image_positions = Vec::new();
            self.editor.update(cx, |editor, cx| {
                editor.transact(cx, |editor, cx| {
                    let edits = editor
                        .selections
                        .all::<usize>(cx)
                        .into_iter()
                        .map(|selection| (selection.start..selection.end, "\n"));
                    editor.edit(edits, cx);

                    let snapshot = editor.buffer().read(cx).snapshot(cx);
                    for selection in editor.selections.all::<usize>(cx) {
                        image_positions.push(snapshot.anchor_before(selection.end));
                    }
                });
            });

            self.context.update(cx, |context, cx| {
                for image in images {
                    let Some(render_image) = image.to_image_data(cx).log_err() else {
                        continue;
                    };
                    let image_id = image.id();
                    let image_task = LanguageModelImage::from_image(image, cx).shared();

                    for image_position in image_positions.iter() {
                        context.insert_content(
                            Content::Image {
                                anchor: image_position.text_anchor,
                                image_id,
                                image: image_task.clone(),
                                render_image: render_image.clone(),
                            },
                            cx,
                        );
                    }
                }
            });
        }
    }

    fn update_image_blocks(&mut self, cx: &mut ViewContext<Self>) {
        self.editor.update(cx, |editor, cx| {
            let buffer = editor.buffer().read(cx).snapshot(cx);
            let excerpt_id = *buffer.as_singleton().unwrap().0;
            let old_blocks = std::mem::take(&mut self.blocks);
            let mut new_blocks = vec![];
            let mut block_index_to_message = vec![];

            let render_block = |message: MessageMetadata| -> RenderBlock {
                Box::new({
                    let context = self.context.clone();
                    move |cx| {
                        let message_id = MessageId(message.timestamp);
                        let show_spinner = message.role == Role::Assistant
                            && message.status == MessageStatus::Pending;

                        let label = match message.role {
                            Role::User => {
                                Label::new("You").color(Color::Default).into_any_element()
                            }
                            Role::Assistant => {
                                let label = Label::new("Assistant").color(Color::Info);
                                if show_spinner {
                                    label
                                        .with_animation(
                                            "pulsating-label",
                                            Animation::new(Duration::from_secs(2))
                                                .repeat()
                                                .with_easing(pulsating_between(0.4, 0.8)),
                                            |label, delta| label.alpha(delta),
                                        )
                                        .into_any_element()
                                } else {
                                    label.into_any_element()
                                }
                            }

                            Role::System => Label::new("System")
                                .color(Color::Warning)
                                .into_any_element(),
                        };

                        let sender = ButtonLike::new("role")
                            .style(ButtonStyle::Filled)
                            .child(label)
                            .tooltip(|cx| {
                                Tooltip::with_meta(
                                    "Toggle message role",
                                    None,
                                    "Available roles: You (User), Assistant, System",
                                    cx,
                                )
                            })
                            .on_click({
                                let context = context.clone();
                                move |_, cx| {
                                    context.update(cx, |context, cx| {
                                        context.cycle_message_roles(
                                            HashSet::from_iter(Some(message_id)),
                                            cx,
                                        )
                                    })
                                }
                            });

                        h_flex()
                            .id(("message_header", message_id.as_u64()))
                            .pl(cx.gutter_dimensions.full_width())
                            .h_11()
                            .w_full()
                            .relative()
                            .gap_1()
                            .child(sender)
                            .children(match &message.cache {
                                Some(cache) if cache.is_final_anchor => match cache.status {
                                    CacheStatus::Cached => Some(
                                        div()
                                            .id("cached")
                                            .child(
                                                Icon::new(IconName::DatabaseZap)
                                                    .size(IconSize::XSmall)
                                                    .color(Color::Hint),
                                            )
                                            .tooltip(|cx| {
                                                Tooltip::with_meta(
                                                    "Context cached",
                                                    None,
                                                    "Large messages cached to optimize performance",
                                                    cx,
                                                )
                                            })
                                            .into_any_element(),
                                    ),
                                    CacheStatus::Pending => Some(
                                        div()
                                            .child(
                                                Icon::new(IconName::Ellipsis)
                                                    .size(IconSize::XSmall)
                                                    .color(Color::Hint),
                                            )
                                            .into_any_element(),
                                    ),
                                },
                                _ => None,
                            })
                            .into_any_element()
                    }
                })
            };
            for message in self.context.read(cx).messages(cx) {
                // This is a new message.
                new_blocks.push(create_block_properties(&message));
                block_index_to_message.push((message.id, MessageMetadata::from(&message)));
            }
            let ids = editor.insert_blocks(new_blocks, None, cx);
            old_blocks.extend(ids.into_iter().zip(block_index_to_message).map(
                |(block_id, (message_id, message_meta))| (message_id, (message_meta, block_id)),
            ));
            self.blocks = old_blocks;
        });
    }

    /// Returns either the selected text, or the content of the Markdown code
    /// block surrounding the cursor.
    fn get_selection_or_code_block(
        context_editor_view: &View<ContextEditor>,
        cx: &mut ViewContext<Workspace>,
    ) -> Option<(String, bool)> {
        const CODE_FENCE_DELIMITER: &'static str = "```";

        let context_editor = context_editor_view.read(cx).editor.read(cx);

        if context_editor.selections.newest::<Point>(cx).is_empty() {
            let snapshot = context_editor.buffer().read(cx).snapshot(cx);
            let head = context_editor.selections.newest::<Point>(cx).head();
            let offset = snapshot.point_to_offset(head);

            let surrounding_code_block_range = find_surrounding_code_block(snapshot, offset)?;
            let mut text = snapshot
                .text_for_range(surrounding_code_block_range)
                .collect::<String>();

            // If there is no newline trailing the closing three-backticks, then
            // tree-sitter-md extends the range of the content node to include
            // the backticks.
            if text.ends_with(CODE_FENCE_DELIMITER) {
                text.drain((text.len() - CODE_FENCE_DELIMITER.len())..);
            }

            (!text.is_empty()).then_some((text, true))
        } else {
            let anchor = context_editor.selections.newest_anchor();
            let text = context_editor
                .buffer()
                .read(cx)
                .read(cx)
                .text_for_range(anchor.range())
                .collect::<String>();

            (!text.is_empty()).then_some((text, false))
        }
    }

    fn insert_selection(
        workspace: &mut Workspace,
        _: &InsertIntoEditor,
        cx: &mut ViewContext<Workspace>,
    ) {
        let Some(panel) = workspace.panel::<AssistantPanel>(cx) else {
            return;
        };
        let Some(context_editor_view) = panel.read(cx).active_context_editor(cx) else {
            return;
        };
        let Some(active_editor_view) = workspace
            .active_item(cx)
            .and_then(|item| item.act_as::<Editor>(cx))
        else {
            return;
        };

        if let Some((text, _)) = Self::get_selection_or_code_block(&context_editor_view, cx) {
            active_editor_view.update(cx, |editor, cx| {
                editor.insert(&text, cx);
                editor.focus(cx);
            })
        }
    }

    fn copy_code(workspace: &mut Workspace, _: &CopyCode, cx: &mut ViewContext<Workspace>) {
        let result = maybe!({
            let panel = workspace.panel::<AssistantPanel>(cx)?;
            let context_editor_view = panel.read(cx).active_context_editor(cx)?;
            Self::get_selection_or_code_block(&context_editor_view, cx)
        });
        let Some((text, is_code_block)) = result else {
            return;
        };

        cx.write_to_clipboard(ClipboardItem::new_string(text));

        struct CopyToClipboardToast;
        workspace.show_toast(
            Toast::new(
                NotificationId::unique::<CopyToClipboardToast>(),
                format!(
                    "{} copied to clipboard.",
                    if is_code_block {
                        "Code block"
                    } else {
                        "Selection"
                    }
                ),
            )
            .autohide(),
            cx,
        );
    }

    fn insert_dragged_files(
        workspace: &mut Workspace,
        action: &InsertDraggedFiles,
        cx: &mut ViewContext<Workspace>,
    ) {
        let Some(panel) = workspace.panel::<AssistantPanel>(cx) else {
            return;
        };
        let Some(context_editor_view) = panel.read(cx).active_context_editor(cx) else {
            return;
        };

        let project = workspace.project().clone();

        let paths = match action {
            InsertDraggedFiles::ProjectPaths(paths) => Task::ready((paths.clone(), vec![])),
            InsertDraggedFiles::ExternalFiles(paths) => {
                let tasks = paths
                    .clone()
                    .into_iter()
                    .map(|path| Workspace::project_path_for_path(project.clone(), &path, false, cx))
                    .collect::<Vec<_>>();

                cx.spawn(move |_, cx| async move {
                    let mut paths = vec![];
                    let mut worktrees = vec![];

                    let opened_paths = futures::future::join_all(tasks).await;
                    for (worktree, project_path) in opened_paths.into_iter().flatten() {
                        let Ok(worktree_root_name) =
                            worktree.read_with(&cx, |worktree, _| worktree.root_name().to_string())
                        else {
                            continue;
                        };

                        let mut full_path = PathBuf::from(worktree_root_name.clone());
                        full_path.push(&project_path.path);
                        paths.push(full_path);
                        worktrees.push(worktree);
                    }

                    (paths, worktrees)
                })
            }
        };

        cx.spawn(|_, mut cx| async move {
            let (paths, dragged_file_worktrees) = paths.await;
            let cmd_name = file_command::FileSlashCommand.name();

            context_editor_view
                .update(&mut cx, |context_editor, cx| {
                    let file_argument = paths
                        .into_iter()
                        .map(|path| path.to_string_lossy().to_string())
                        .collect::<Vec<_>>()
                        .join(" ");

                    context_editor.editor.update(cx, |editor, cx| {
                        editor.insert("\n", cx);
                        editor.insert(&format!("/{} {}", cmd_name, file_argument), cx);
                    });

                    context_editor.confirm_command(&ConfirmCommand, cx);

                    context_editor
                        .dragged_file_worktrees
                        .extend(dragged_file_worktrees);
                })
                .log_err();
        })
        .detach();
    }

    fn quote_selection(
        workspace: &mut Workspace,
        _: &QuoteSelection,
        cx: &mut ViewContext<Workspace>,
    ) {
        let Some(panel) = workspace.panel::<AssistantPanel>(cx) else {
            return;
        };
        let Some(editor) = workspace
            .active_item(cx)
            .and_then(|item| item.act_as::<Editor>(cx))
        else {
            return;
        };

        let mut creases = vec![];
        editor.update(cx, |editor, cx| {
            let selections = editor.selections.all_adjusted(cx);
            let buffer = editor.buffer().read(cx).snapshot(cx);
            for selection in selections {
                let range = editor::ToOffset::to_offset(&selection.start, &buffer)
                    ..editor::ToOffset::to_offset(&selection.end, &buffer);
                let selected_text = buffer.text_for_range(range.clone()).collect::<String>();
                if selected_text.is_empty() {
                    continue;
                }
                let start_language = buffer.language_at(range.start);
                let end_language = buffer.language_at(range.end);
                let language_name = if start_language == end_language {
                    start_language.map(|language| language.code_fence_block_name())
                } else {
                    None
                };
                let language_name = language_name.as_deref().unwrap_or("");
                let filename = buffer
                    .file_at(selection.start)
                    .map(|file| file.full_path(cx));
                let text = if language_name == "markdown" {
                    selected_text
                        .lines()
                        .map(|line| format!("> {}", line))
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    let start_symbols = buffer
                        .symbols_containing(selection.start, None)
                        .map(|(_, symbols)| symbols);
                    let end_symbols = buffer
                        .symbols_containing(selection.end, None)
                        .map(|(_, symbols)| symbols);

                    let outline_text = if let Some((start_symbols, end_symbols)) =
                        start_symbols.zip(end_symbols)
                    {
                        Some(
                            start_symbols
                                .into_iter()
                                .zip(end_symbols)
                                .take_while(|(a, b)| a == b)
                                .map(|(a, _)| a.text)
                                .collect::<Vec<_>>()
                                .join(" > "),
                        )
                    } else {
                        None
                    };

                    let line_comment_prefix = start_language
                        .and_then(|l| l.default_scope().line_comment_prefixes().first().cloned());

                    let fence = codeblock_fence_for_path(
                        filename.as_deref(),
                        Some(selection.start.row..=selection.end.row),
                    );

                    if let Some((line_comment_prefix, outline_text)) =
                        line_comment_prefix.zip(outline_text)
                    {
                        let breadcrumb =
                            format!("{line_comment_prefix}Excerpt from: {outline_text}\n");
                        format!("{fence}{breadcrumb}{selected_text}\n```")
                    } else {
                        format!("{fence}{selected_text}\n```")
                    }
                };
                let crease_title = if let Some(path) = filename {
                    let start_line = selection.start.row + 1;
                    let end_line = selection.end.row + 1;
                    if start_line == end_line {
                        format!("{}, Line {}", path.display(), start_line)
                    } else {
                        format!("{}, Lines {} to {}", path.display(), start_line, end_line)
                    }
                } else {
                    "Quoted selection".to_string()
                };
                creases.push((text, crease_title));
            }
        });
        if creases.is_empty() {
            return;
        }
        // Activate the panel
        if !panel.focus_handle(cx).contains_focused(cx) {
            workspace.toggle_panel_focus::<AssistantPanel>(cx);
        }

        panel.update(cx, |_, cx| {
            // Wait to create a new context until the workspace is no longer
            // being updated.
            cx.defer(move |panel, cx| {
                if let Some(context) = panel
                    .active_context_editor(cx)
                    .or_else(|| panel.new_context(cx))
                {
                    context.update(cx, |context, cx| {
                        context.editor.update(cx, |editor, cx| {
                            editor.insert("\n", cx);
                            for (text, crease_title) in creases {
                                let point = editor.selections.newest::<Point>(cx).head();
                                let start_row = MultiBufferRow(point.row);

                                editor.insert(&text, cx);

                                let snapshot = editor.buffer().read(cx).snapshot(cx);
                                let anchor_before = snapshot.anchor_after(point);
                                let anchor_after = editor
                                    .selections
                                    .newest_anchor()
                                    .head()
                                    .bias_left(&snapshot);

                                editor.insert("\n", cx);

                                let fold_placeholder = quote_selection_fold_placeholder(
                                    crease_title,
                                    cx.view().downgrade(),
                                );
                                let crease = Crease::new(
                                    anchor_before..anchor_after,
                                    fold_placeholder,
                                    render_quote_selection_output_toggle,
                                    |_, _, _| Empty.into_any(),
                                );
                                editor.insert_creases(vec![crease], cx);
                                editor.fold_at(
                                    &FoldAt {
                                        buffer_row: start_row,
                                    },
                                    cx,
                                );
                            }
                        })
                    });
                };
            });
        });
    }

    fn copy(&mut self, _: &editor::actions::Copy, cx: &mut ViewContext<Self>) {
        if self.editor.read(cx).selections.count() == 1 {
            let (copied_text, metadata, _) = self.get_clipboard_contents(cx);
            cx.write_to_clipboard(ClipboardItem::new_string_with_json_metadata(
                copied_text,
                metadata,
            ));
            cx.stop_propagation();
            return;
        }

        cx.propagate();
    }

    fn cut(&mut self, _: &editor::actions::Cut, cx: &mut ViewContext<Self>) {
        if self.editor.read(cx).selections.count() == 1 {
            let (copied_text, metadata, selections) = self.get_clipboard_contents(cx);

            self.editor.update(cx, |editor, cx| {
                editor.transact(cx, |this, cx| {
                    this.change_selections(Some(Autoscroll::fit()), cx, |s| {
                        s.select(selections);
                    });
                    this.insert("", cx);
                    cx.write_to_clipboard(ClipboardItem::new_string_with_json_metadata(
                        copied_text,
                        metadata,
                    ));
                });
            });

            cx.stop_propagation();
            return;
        }

        cx.propagate();
    }

    fn get_clipboard_contents(
        &mut self,
        cx: &mut ViewContext<Self>,
    ) -> (String, CopyMetadata, Vec<text::Selection<usize>>) {
        let (snapshot, selection, creases) = self.editor.update(cx, |editor, cx| {
            let mut selection = editor.selections.newest::<Point>(cx);
            let snapshot = editor.buffer().read(cx).snapshot(cx);

            let is_entire_line = selection.is_empty() || editor.selections.line_mode;
            if is_entire_line {
                selection.start = Point::new(selection.start.row, 0);
                selection.end =
                    cmp::min(snapshot.max_point(), Point::new(selection.start.row + 1, 0));
                selection.goal = SelectionGoal::None;
            }

            let selection_start = snapshot.point_to_offset(selection.start);

            (
                snapshot.clone(),
                selection.clone(),
                editor.display_map.update(cx, |display_map, cx| {
                    display_map
                        .snapshot(cx)
                        .crease_snapshot
                        .creases_in_range(
                            MultiBufferRow(selection.start.row)
                                ..MultiBufferRow(selection.end.row + 1),
                            &snapshot,
                        )
                        .filter_map(|crease| {
                            if let Some(metadata) = &crease.metadata {
                                let start = crease
                                    .range
                                    .start
                                    .to_offset(&snapshot)
                                    .saturating_sub(selection_start);
                                let end = crease
                                    .range
                                    .end
                                    .to_offset(&snapshot)
                                    .saturating_sub(selection_start);

                                let range_relative_to_selection = start..end;

                                if range_relative_to_selection.is_empty() {
                                    None
                                } else {
                                    Some(SelectedCreaseMetadata {
                                        range_relative_to_selection,
                                        crease: metadata.clone(),
                                    })
                                }
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                }),
            )
        });

        let selection = selection.map(|point| snapshot.point_to_offset(point));
        let context = self.context.read(cx);

        let mut text = String::new();
        for message in context.messages(cx) {
            if message.offset_range.start >= selection.range().end {
                break;
            } else if message.offset_range.end >= selection.range().start {
                let range = cmp::max(message.offset_range.start, selection.range().start)
                    ..cmp::min(message.offset_range.end, selection.range().end);
                if !range.is_empty() {
                    for chunk in context.buffer().read(cx).text_for_range(range) {
                        text.push_str(chunk);
                    }
                    if message.offset_range.end < selection.range().end {
                        text.push('\n');
                    }
                }
            }
        }

        (text, CopyMetadata { creases }, vec![selection])
    }

    fn paste(&mut self, action: &editor::actions::Paste, cx: &mut ViewContext<Self>) {
        cx.stop_propagation();

        let images = if let Some(item) = cx.read_from_clipboard() {
            item.into_entries()
                .filter_map(|entry| {
                    if let ClipboardEntry::Image(image) = entry {
                        Some(image)
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

        let metadata = if let Some(item) = cx.read_from_clipboard() {
            item.entries().first().and_then(|entry| {
                if let ClipboardEntry::String(text) = entry {
                    text.metadata_json::<CopyMetadata>()
                } else {
                    None
                }
            })
        } else {
            None
        };

        if images.is_empty() {
            self.editor.update(cx, |editor, cx| {
                let paste_position = editor.selections.newest::<usize>(cx).head();
                editor.paste(action, cx);

                if let Some(metadata) = metadata {
                    let buffer = editor.buffer().read(cx).snapshot(cx);

                    let mut buffer_rows_to_fold = BTreeSet::new();
                    let weak_editor = cx.view().downgrade();
                    editor.insert_creases(
                        metadata.creases.into_iter().map(|metadata| {
                            let start = buffer.anchor_after(
                                paste_position + metadata.range_relative_to_selection.start,
                            );
                            let end = buffer.anchor_before(
                                paste_position + metadata.range_relative_to_selection.end,
                            );

                            let buffer_row = MultiBufferRow(start.to_point(&buffer).row);
                            buffer_rows_to_fold.insert(buffer_row);
                            Crease::new(
                                start..end,
                                FoldPlaceholder {
                                    render: render_fold_icon_button(
                                        weak_editor.clone(),
                                        metadata.crease.icon,
                                        metadata.crease.label.clone(),
                                    ),
                                    constrain_width: false,
                                    merge_adjacent: false,
                                },
                                render_slash_command_output_toggle,
                                |_, _, _| Empty.into_any(),
                            )
                            .with_metadata(metadata.crease.clone())
                        }),
                        cx,
                    );
                    for buffer_row in buffer_rows_to_fold.into_iter().rev() {
                        editor.fold_at(&FoldAt { buffer_row }, cx);
                    }
                }
            });
        } else {
            let mut image_positions = Vec::new();
            self.editor.update(cx, |editor, cx| {
                editor.transact(cx, |editor, cx| {
                    let edits = editor
                        .selections
                        .all::<usize>(cx)
                        .into_iter()
                        .map(|selection| (selection.start..selection.end, "\n"));
                    editor.edit(edits, cx);

                    let snapshot = editor.buffer().read(cx).snapshot(cx);
                    for selection in editor.selections.all::<usize>(cx) {
                        image_positions.push(snapshot.anchor_before(selection.end));
                    }
                });
            });

            self.context.update(cx, |context, cx| {
                for image in images {
                    let Some(render_image) = image.to_image_data(cx).log_err() else {
                        continue;
                    };
                    let image_id = image.id();
                    let image_task = LanguageModelImage::from_image(image, cx).shared();

                    for image_position in image_positions.iter() {
                        context.insert_content(
                            Content::Image {
                                anchor: image_position.text_anchor,
                                image_id,
                                image: image_task.clone(),
                                render_image: render_image.clone(),
                            },
                            cx,
                        );
                    }
                }
            });
        }
    }

    fn update_image_blocks(&mut self, cx: &mut ViewContext<Self>) {
        self.editor.update(cx, |editor, cx| {
            let buffer = editor.buffer().read(cx).snapshot(cx);
            let excerpt_id = *buffer.as_singleton().unwrap().0;
            let old_blocks = std::mem::take(&mut self.blocks);
            let mut new_blocks = vec![];
            let mut block_index_to_message = vec![];

            let render_block = |message: MessageMetadata| -> RenderBlock {
                Box::new({
                    let context = self.context.clone();
                    move |cx| {
                        let message_id = MessageId(message.timestamp);
                        let show_spinner = message.role == Role::Assistant
                            && message.status == MessageStatus::Pending;

                        let label = match message.role {
                            Role::User => {
                                Label::new("You").color(Color::Default).into_any_element()
                            }
                            Role::Assistant => {
                                let label = Label::new("Assistant").color(Color::Info);
                                if show_spinner {
                                    label
                                        .with_animation(
                                            "pulsating-label",
                                            Animation::new(Duration::from_secs(2))
                                                .repeat()
                                                .with_easing(pulsating_between(0.4, 0.8)),
                                            |label, delta| label.alpha(delta),
                                        )
                                        .into_any_element()
                                } else {
                                    label.into_any_element()
                                }
                            }

                            Role::System => Label::new("System")
                                .color(Color::Warning)
                                .into_any_element(),
                        };

                        let sender = ButtonLike::new("role")
                            .style(ButtonStyle::Filled)
                            .child(label)
                            .tooltip(|cx| {
                                Tooltip::with_meta(
                                    "Toggle message role",
                                    None,
                                    "Available roles: You (User), Assistant, System",
                                    cx,
                                )
                            })
                            .on_click({
                                let context = context.clone();
                                move |_, cx| {
                                    context.update(cx, |context, cx| {
                                        context.cycle_message_roles(
                                            HashSet::from_iter(Some(message_id)),
                                            cx,
                                        )