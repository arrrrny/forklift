use gpui::{
    AnyModel, AnyView, DismissEvent, FocusHandle, ManagedView, Model, ModelContext, Subscription,
    Window,
};
use ui::prelude::*;

pub enum DismissDecision {
    Dismiss(bool),
    Pending,
}

pub trait ModalView: ManagedView + Render {
    fn on_before_dismiss(&mut self, _: &mut Window, _: &mut ModelContext<Self>) -> DismissDecision {
        DismissDecision::Dismiss(true)
    }

    fn fade_out_background(&self) -> bool {
        false
    }
}

trait ModalViewHandle {
    fn on_before_dismiss(&mut self, window: &mut Window, cx: &mut AppContext) -> DismissDecision;
    fn model(&self) -> AnyModel;
    fn view(&self) -> AnyView;
    fn fade_out_background(&self, cx: &mut AppContext) -> bool;
}

impl<V: ModalView> ModalViewHandle for Model<V> {
    fn on_before_dismiss(&mut self, window: &mut Window, cx: &mut AppContext) -> DismissDecision {
        self.update(cx, |this, cx| this.on_before_dismiss(window, cx))
    }

    fn model(&self) -> AnyModel {
        self.clone().into()
    }

    fn view(&self) -> AnyView {
        self.clone().into()
    }

    fn fade_out_background(&self, cx: &mut AppContext) -> bool {
        self.read(cx).fade_out_background()
    }
}

pub struct ActiveModal {
    modal: Box<dyn ModalViewHandle>,
    _subscriptions: [Subscription; 2],
    previous_focus_handle: Option<FocusHandle>,
    focus_handle: FocusHandle,
}

pub struct ModalLayer {
    active_modal: Option<ActiveModal>,
    dismiss_on_focus_lost: bool,
}

impl Default for ModalLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl ModalLayer {
    pub fn new() -> Self {
        Self {
            active_modal: None,
            dismiss_on_focus_lost: false,
        }
    }

    pub fn toggle_modal<V, B>(
        &mut self,
        window: &mut Window,
        cx: &mut ModelContext<Self>,
        build_view: B,
    ) where
        V: ModalView,
        B: FnOnce(&mut Window, &mut ModelContext<V>) -> V,
    {
        if let Some(active_modal) = &self.active_modal {
            let is_close = active_modal.modal.model().downcast::<V>().is_ok();
            let did_close = self.hide_modal(window, cx);
            if is_close || !did_close {
                return;
            }
        }
        let new_modal = cx.new_model(|cx| build_view(window, cx));
        self.show_modal(new_modal, window, cx);
    }

    fn show_modal<V>(
        &mut self,
        new_modal: Model<V>,
        window: &mut Window,
        cx: &mut ModelContext<Self>,
    ) where
        V: ModalView,
    {
        let focus_handle = window.focus_handle();
        self.active_modal = Some(ActiveModal {
            modal: Box::new(new_modal.clone()),
            _subscriptions: [
                cx.subscribe(&new_modal, |this, _, _: &DismissEvent, cx| {
                    this.hide_modal(window, cx);
                }),
                cx.on_focus_out(&focus_handle, |this, _event, cx| {
                    if this.dismiss_on_focus_lost {
                        this.hide_modal(cx);
                    }
                }),
            ],
            previous_focus_handle: cx.focused(),
            focus_handle,
        });
        cx.defer(move |_, cx| {
            window.focus_view(&new_modal, cx);
        });
        cx.notify();
    }

    fn hide_modal(&mut self, window: &mut Window, cx: &mut ModelContext<Self>) -> bool {
        let Some(active_modal) = self.active_modal.as_mut() else {
            self.dismiss_on_focus_lost = false;
            return false;
        };

        match active_modal.modal.on_before_dismiss(window, cx) {
            DismissDecision::Dismiss(dismiss) => {
                self.dismiss_on_focus_lost = !dismiss;
                if !dismiss {
                    return false;
                }
            }
            DismissDecision::Pending => {
                self.dismiss_on_focus_lost = false;
                return false;
            }
        }

        if let Some(active_modal) = self.active_modal.take() {
            if let Some(previous_focus) = active_modal.previous_focus_handle {
                if active_modal.focus_handle.contains_focused(window) {
                    previous_focus.focus(window);
                }
            }
            cx.notify();
        }
        true
    }

    pub fn active_modal<V>(&self) -> Option<Model<V>>
    where
        V: 'static,
    {
        let active_modal = self.active_modal.as_ref()?;
        active_modal.modal.model().downcast::<V>().ok()
    }

    pub fn has_active_modal(&self) -> bool {
        self.active_modal.is_some()
    }
}

impl Render for ModalLayer {
    fn render(&mut self, window: &mut Window, cx: &mut ModelContext<Self>) -> impl IntoElement {
        let Some(active_modal) = &self.active_modal else {
            return div();
        };

        div()
            .absolute()
            .size_full()
            .top_0()
            .left_0()
            .when(active_modal.modal.fade_out_background(cx), |el| {
                let mut background = cx.theme().colors().elevated_surface_background;
                background.fade_out(0.2);
                el.bg(background)
                    .occlude()
                    .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                        this.hide_modal(window, cx);
                    }))
            })
            .child(
                v_flex()
                    .h(px(0.0))
                    .top_20()
                    .flex()
                    .flex_col()
                    .items_center()
                    .track_focus(&active_modal.focus_handle)
                    .child(h_flex().occlude().child(active_modal.modal.view())),
            )
    }
}
