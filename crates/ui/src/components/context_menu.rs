#![allow(missing_docs)]
use crate::{
    h_flex, prelude::*, utils::WithRemSize, v_flex, Icon, IconName, IconSize, KeyBinding, Label,
    List, ListItem, ListSeparator, ListSubHeader,
};
use gpui::{
    px, Action, AnyElement, AppContext, DismissEvent, EventEmitter, FocusHandle, FocusableView,
    IntoElement, Render, Subscription,
};
use menu::{SelectFirst, SelectLast, SelectNext, SelectPrev};
use std::{rc::Rc, time::Duration};
use theme::ThemeSettings;

enum ContextMenuItem {
    Separator,
    Header(SharedString),
    Label(SharedString),
    Entry {
        toggle: Option<(IconPosition, bool)>,
        label: SharedString,
        icon: Option<IconName>,
        icon_size: IconSize,
        handler: Rc<dyn Fn(Option<&FocusHandle>, &mut gpui::Window, &mut gpui::AppContext)>,
        action: Option<Box<dyn Action>>,
        disabled: bool,
    },
    CustomEntry {
        entry_render: Box<dyn Fn(&mut gpui::Window, &mut gpui::AppContext) -> AnyElement>,
        handler: Rc<dyn Fn(Option<&FocusHandle>, &mut gpui::Window, &mut gpui::AppContext)>,
        selectable: bool,
    },
}

pub struct ContextMenu {
    items: Vec<ContextMenuItem>,
    focus_handle: FocusHandle,
    action_context: Option<FocusHandle>,
    selected_index: Option<usize>,
    delayed: bool,
    clicked: bool,
    _on_blur_subscription: Subscription,
}

impl FocusableView for ContextMenu {
    fn focus_handle(&self, _cx: &AppContext) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for ContextMenu {}

impl FluentBuilder for ContextMenu {}

impl ContextMenu {
    pub fn build(
        window: &mut gpui::Window,
        cx: &mut gpui::AppContext,
        f: impl FnOnce(Self, &mut gpui::Window, &mut gpui::ModelContext<Self>) -> Self,
    ) -> gpui::Model<Self> {
        cx.new_model(|cx| {
            let focus_handle = window.focus_handle();
            let this = cx.handle();
            let _on_blur_subscription = window.on_blur(&focus_handle, cx, move |window, cx| {
                this.update(cx, |this: &mut Self, cx| {
                    this.cancel(&menu::Cancel, window, cx)
                })
            });
            cx.refresh();
            f(
                Self {
                    items: Default::default(),
                    focus_handle,
                    action_context: None,
                    selected_index: None,
                    delayed: false,
                    clicked: false,
                    _on_blur_subscription,
                },
                window,
                cx,
            )
        })
    }

    pub fn context(mut self, focus: FocusHandle) -> Self {
        self.action_context = Some(focus);
        self
    }

    pub fn header(mut self, title: impl Into<SharedString>) -> Self {
        self.items.push(ContextMenuItem::Header(title.into()));
        self
    }

    pub fn separator(mut self) -> Self {
        self.items.push(ContextMenuItem::Separator);
        self
    }

    pub fn entry(
        mut self,
        label: impl Into<SharedString>,
        action: Option<Box<dyn Action>>,
        handler: impl Fn(&mut gpui::Window, &mut gpui::AppContext) + 'static,
    ) -> Self {
        self.items.push(ContextMenuItem::Entry {
            toggle: None,
            label: label.into(),
            handler: Rc::new(move |_, window, cx| handler(window, cx)),
            icon: None,
            icon_size: IconSize::Small,
            action,
            disabled: false,
        });
        self
    }

    pub fn toggleable_entry(
        mut self,
        label: impl Into<SharedString>,
        toggled: bool,
        position: IconPosition,
        action: Option<Box<dyn Action>>,
        handler: impl Fn(&mut gpui::Window, &mut gpui::AppContext) + 'static,
    ) -> Self {
        self.items.push(ContextMenuItem::Entry {
            toggle: Some((position, toggled)),
            label: label.into(),
            handler: Rc::new(move |_, window, cx| handler(window, cx)),
            icon: None,
            icon_size: IconSize::Small,
            action,
            disabled: false,
        });
        self
    }

    pub fn custom_row(
        mut self,
        entry_render: impl Fn(&mut gpui::Window, &mut gpui::AppContext) -> AnyElement + 'static,
    ) -> Self {
        self.items.push(ContextMenuItem::CustomEntry {
            entry_render: Box::new(entry_render),
            handler: Rc::new(|_, _, _| {}),
            selectable: false,
        });
        self
    }

    pub fn custom_entry(
        mut self,
        entry_render: impl Fn(&mut gpui::Window, &mut gpui::AppContext) -> AnyElement + 'static,
        handler: impl Fn(&mut gpui::Window, &mut gpui::AppContext) + 'static,
    ) -> Self {
        self.items.push(ContextMenuItem::CustomEntry {
            entry_render: Box::new(entry_render),
            handler: Rc::new(move |_, window, cx| handler(window, cx)),
            selectable: true,
        });
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.items.push(ContextMenuItem::Label(label.into()));
        self
    }

    pub fn action(mut self, label: impl Into<SharedString>, action: Box<dyn Action>) -> Self {
        self.items.push(ContextMenuItem::Entry {
            toggle: None,
            label: label.into(),
            action: Some(action.boxed_clone()),

            handler: Rc::new(move |context, window, cx| {
                if let Some(context) = &context {
                    window.focus(context);
                }
                window.dispatch_action(action.boxed_clone(), cx);
            }),
            icon: None,
            icon_size: IconSize::Small,
            disabled: false,
        });
        self
    }

    pub fn disabled_action(
        mut self,
        label: impl Into<SharedString>,
        action: Box<dyn Action>,
    ) -> Self {
        self.items.push(ContextMenuItem::Entry {
            toggle: None,
            label: label.into(),
            action: Some(action.boxed_clone()),

            handler: Rc::new(move |context, window, cx| {
                if let Some(context) = &context {
                    window.focus(context);
                }
                window.dispatch_action(action.boxed_clone(), cx);
            }),
            icon: None,
            icon_size: IconSize::Small,
            disabled: true,
        });
        self
    }

    pub fn link(mut self, label: impl Into<SharedString>, action: Box<dyn Action>) -> Self {
        self.items.push(ContextMenuItem::Entry {
            toggle: None,
            label: label.into(),

            action: Some(action.boxed_clone()),
            handler: Rc::new(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx)),
            icon: Some(IconName::ArrowUpRight),
            icon_size: IconSize::XSmall,
            disabled: false,
        });
        self
    }

    pub fn confirm(
        &mut self,
        _: &menu::Confirm,
        window: &mut gpui::Window,
        cx: &mut gpui::ModelContext<Self>,
    ) {
        let context = self.action_context.as_ref();
        if let Some(
            ContextMenuItem::Entry {
                handler,
                disabled: false,
                ..
            }
            | ContextMenuItem::CustomEntry { handler, .. },
        ) = self.selected_index.and_then(|ix| self.items.get(ix))
        {
            (handler)(context, window, cx)
        }

        cx.emit(DismissEvent);
    }

    pub fn cancel(
        &mut self,
        _: &menu::Cancel,
        _window: &mut Window,
        cx: &mut gpui::ModelContext<Self>,
    ) {
        cx.emit(DismissEvent);
        cx.emit(DismissEvent);
    }

    fn select_first(
        &mut self,
        _: &SelectFirst,
        _window: &mut gpui::Window,
        cx: &mut gpui::ModelContext<Self>,
    ) {
        self.selected_index = self.items.iter().position(|item| item.is_selectable());
        cx.notify();
    }

    pub fn select_last(&mut self) -> Option<usize> {
        for (ix, item) in self.items.iter().enumerate().rev() {
            if item.is_selectable() {
                self.selected_index = Some(ix);
                return Some(ix);
            }
        }
        None
    }

    fn handle_select_last(
        &mut self,
        _: &SelectLast,
        _window: &mut gpui::Window,
        cx: &mut gpui::ModelContext<Self>,
    ) {
        if self.select_last().is_some() {
            cx.notify();
        }
    }

    fn select_next(
        &mut self,
        _: &SelectNext,
        window: &mut gpui::Window,
        cx: &mut gpui::ModelContext<Self>,
    ) {
        if let Some(ix) = self.selected_index {
            let next_index = ix + 1;
            if self.items.len() <= next_index {
                self.select_first(&SelectFirst, window, cx);
            } else {
                for (ix, item) in self.items.iter().enumerate().skip(next_index) {
                    if item.is_selectable() {
                        self.selected_index = Some(ix);
                        cx.notify();
                        break;
                    }
                }
            }
        } else {
            self.select_first(&SelectFirst, window, cx);
        }
    }

    pub fn select_prev(
        &mut self,
        _: &SelectPrev,
        window: &mut gpui::Window,
        cx: &mut gpui::ModelContext<Self>,
    ) {
        if let Some(ix) = self.selected_index {
            if ix == 0 {
                self.handle_select_last(&SelectLast, window, cx);
            } else {
                for (ix, item) in self.items.iter().enumerate().take(ix).rev() {
                    if item.is_selectable() {
                        self.selected_index = Some(ix);
                        cx.notify();
                        break;
                    }
                }
            }
        } else {
            self.handle_select_last(&SelectLast, window, cx);
        }
    }

    pub fn on_action_dispatch(
        &mut self,
        dispatched: &dyn Action,
        window: &mut gpui::Window,
        cx: &mut gpui::ModelContext<Self>,
    ) {
        if self.clicked {
            cx.propagate();
            return;
        }

        if let Some(ix) = self.items.iter().position(|item| {
            if let ContextMenuItem::Entry {
                action: Some(action),
                disabled: false,
                ..
            } = item
            {
                action.partial_eq(dispatched)
            } else {
                false
            }
        }) {
            self.selected_index = Some(ix);
            self.delayed = true;
            cx.notify();
            let action = dispatched.boxed_clone();
            let window = window.handle();
            cx.spawn(|this, mut cx| async move {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                window.update(&mut cx, |window, cx| {
                    this.update(cx, |this, cx| {
                        this.cancel(&menu::Cancel, window, cx);
                        window.dispatch_action(action, cx);
                    })
                })
            })
            .detach_and_log_err(cx);
        } else {
            cx.propagate()
        }
    }

    pub fn on_blur_subscription(mut self, new_subscription: Subscription) -> Self {
        self._on_blur_subscription = new_subscription;
        self
    }
}

impl ContextMenuItem {
    fn is_selectable(&self) -> bool {
        match self {
            ContextMenuItem::Header(_)
            | ContextMenuItem::Separator
            | ContextMenuItem::Label { .. } => false,
            ContextMenuItem::Entry { disabled, .. } => !disabled,
            ContextMenuItem::CustomEntry { selectable, .. } => *selectable,
        }
    }
}

impl Render for ContextMenu {
    fn render(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::ModelContext<Self>,
    ) -> impl IntoElement {
        use settings::Settings;
        let ui_font_size = ThemeSettings::get_global(cx).ui_font_size;

        div().occlude().elevation_2(cx).flex().flex_row().child(
            WithRemSize::new(ui_font_size).flex().child(
                v_flex()
                    .id("context-menu")
                    .min_w(px(200.))
                    .max_h(vh(0.75, window))
                    .overflow_y_scroll()
                    .track_focus(&self.focus_handle(cx))
                    .on_mouse_down_out(cx.listener(|this, _event, window, cx| {
                        this.cancel(&menu::Cancel, window, cx)
                    }))
                    .key_context("menu")
                    .on_action(cx.listener(ContextMenu::select_first))
                    .on_action(cx.listener(ContextMenu::handle_select_last))
                    .on_action(cx.listener(ContextMenu::select_next))
                    .on_action(cx.listener(ContextMenu::select_prev))
                    .on_action(cx.listener(ContextMenu::confirm))
                    .on_action(cx.listener(ContextMenu::cancel))
                    .when(!self.delayed, |mut el| {
                        for item in self.items.iter() {
                            if let ContextMenuItem::Entry {
                                action: Some(action),
                                disabled: false,
                                ..
                            } = item
                            {
                                el = el.on_boxed_action(
                                    &**action,
                                    cx.listener(ContextMenu::on_action_dispatch),
                                );
                            }
                        }
                        el
                    })
                    .flex_none()
                    .child(List::new().children(self.items.iter_mut().enumerate().map(
                        |(ix, item)| {
                            match item {
                                ContextMenuItem::Separator => ListSeparator.into_any_element(),
                                ContextMenuItem::Header(header) => {
                                    ListSubHeader::new(header.clone())
                                        .inset(true)
                                        .into_any_element()
                                }
                                ContextMenuItem::Label(label) => ListItem::new(ix)
                                    .inset(true)
                                    .disabled(true)
                                    .child(Label::new(label.clone()))
                                    .into_any_element(),
                                ContextMenuItem::Entry {
                                    toggle,
                                    label,
                                    handler,
                                    icon,
                                    icon_size,
                                    action,
                                    disabled,
                                } => {
                                    let handler = handler.clone();
                                    let menu = cx.handle().downgrade();
                                    let color = if *disabled {
                                        Color::Muted
                                    } else {
                                        Color::Default
                                    };
                                    let label_element = if let Some(icon_name) = icon {
                                        h_flex()
                                            .gap_1()
                                            .child(Label::new(label.clone()).color(color))
                                            .child(
                                                Icon::new(*icon_name).size(*icon_size).color(color),
                                            )
                                            .into_any_element()
                                    } else {
                                        Label::new(label.clone()).color(color).into_any_element()
                                    };

                                    ListItem::new(ix)
                                        .inset(true)
                                        .disabled(*disabled)
                                        .selected(Some(ix) == self.selected_index)
                                        .when_some(*toggle, |list_item, (position, toggled)| {
                                            let contents = if toggled {
                                                v_flex().flex_none().child(
                                                    Icon::new(IconName::Check).color(Color::Accent),
                                                )
                                            } else {
                                                v_flex()
                                                    .flex_none()
                                                    .size(IconSize::default().rems())
                                            };
                                            match position {
                                                IconPosition::Start => {
                                                    list_item.start_slot(contents)
                                                }
                                                IconPosition::End => list_item.end_slot(contents),
                                            }
                                        })
                                        .child(
                                            h_flex()
                                                .w_full()
                                                .justify_between()
                                                .child(label_element)
                                                .debug_selector(|| format!("MENU_ITEM-{}", label))
                                                .children(action.as_ref().and_then(|action| {
                                                    self.action_context
                                                        .as_ref()
                                                        .map(|focus| {
                                                            KeyBinding::for_action_in(
                                                                &**action, focus, window,
                                                            )
                                                        })
                                                        .unwrap_or_else(|| {
                                                            KeyBinding::for_action(
                                                                &**action, window, cx,
                                                            )
                                                        })
                                                        .map(|binding| div().ml_4().child(binding))
                                                })),
                                        )
                                        .on_click({
                                            let context = self.action_context.clone();
                                            move |_, window, cx| {
                                                handler(context.as_ref(), window, cx);
                                                menu.update(cx, |menu, cx| {
                                                    menu.clicked = true;
                                                    cx.emit(DismissEvent);
                                                })
                                                .ok();
                                            }
                                        })
                                        .into_any_element()
                                }
                                ContextMenuItem::CustomEntry {
                                    entry_render,
                                    handler,
                                    selectable,
                                } => {
                                    let handler = handler.clone();
                                    let menu = cx.handle().downgrade();
                                    let selectable = *selectable;
                                    ListItem::new(ix)
                                        .inset(true)
                                        .selected(if selectable {
                                            Some(ix) == self.selected_index
                                        } else {
                                            false
                                        })
                                        .selectable(selectable)
                                        .when(selectable, |item| {
                                            item.on_click({
                                                let context = self.action_context.clone();
                                                move |_, window, cx| {
                                                    handler(context.as_ref(), window, cx);
                                                    menu.update(cx, |menu, cx| {
                                                        menu.clicked = true;
                                                        cx.emit(DismissEvent);
                                                    })
                                                    .ok();
                                                }
                                            })
                                        })
                                        .child(entry_render(window, cx))
                                        .into_any_element()
                                }
                            }
                        },
                    ))),
            ),
        )
    }
}
