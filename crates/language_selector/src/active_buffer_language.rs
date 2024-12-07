use editor::Editor;
use gpui::{div, IntoElement, ModelContext, ParentElement, Render, Subscription, View, WeakView};
use language::LanguageName;
use ui::{Button, ButtonCommon, Clickable, FluentBuilder, LabelSize, Tooltip};
use workspace::{item::ItemHandle, StatusItemView, Workspace};

use crate::LanguageSelector;

pub struct ActiveBufferLanguage {
    active_language: Option<Option<LanguageName>>,
    workspace: WeakModel<Workspace>,
    _observe_active_editor: Option<Subscription>,
}

impl ActiveBufferLanguage {
    pub fn new(workspace: &Workspace) -> Self {
        Self {
            active_language: None,
            workspace: workspace.weak_handle(),
            _observe_active_editor: None,
        }
    }

    fn update_language(&mut self, editor: Model<Editor>, cx: &mut ModelContext<Self>) {
        self.active_language = Some(None);

        let editor = editor.read(cx);
        if let Some((_, buffer, _)) = editor.active_excerpt(cx) {
            if let Some(language) = buffer.read(cx).language() {
                self.active_language = Some(Some(language.name()));
            }
        }

        cx.notify();
    }
}

impl Render for ActiveBufferLanguage {
    fn render(&mut self, cx: &mut ModelContext<Self>) -> impl IntoElement {
        div().when_some(self.active_language.as_ref(), |el, active_language| {
            let active_language_text = if let Some(active_language_text) = active_language {
                active_language_text.to_string()
            } else {
                "Unknown".to_string()
            };

            el.child(
                Button::new("change-language", active_language_text)
                    .label_size(LabelSize::Small)
                    .on_click(cx.listener(|this, _, cx| {
                        if let Some(workspace) = this.workspace.upgrade() {
                            workspace.update(cx, |workspace, cx| {
                                LanguageSelector::toggle(workspace, cx)
                            });
                        }
                    }))
                    .tooltip(|cx| Tooltip::text("Select Language", cx)),
            )
        })
    }
}

impl StatusItemView for ActiveBufferLanguage {
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn ItemHandle>,
        cx: &mut ModelContext<Self>,
    ) {
        if let Some(editor) = active_pane_item.and_then(|item| item.downcast::<Editor>()) {
            self._observe_active_editor = Some(cx.observe(&editor, Self::update_language));
            self.update_language(editor, cx);
        } else {
            self.active_language = None;
            self._observe_active_editor = None;
        }

        cx.notify();
    }
}
