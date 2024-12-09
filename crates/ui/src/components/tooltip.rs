#![allow(missing_docs)]

use std::rc::Rc;

use gpui::{Action, AnyElement, AnyView, FocusHandle, IntoElement, Render};
use settings::Settings;
use theme::ThemeSettings;

use crate::prelude::*;
use crate::{h_flex, v_flex, Color, KeyBinding, Label, LabelSize, StyledExt};

pub struct Tooltip {
    title: SharedString,
    meta: Option<SharedString>,
    key_binding: Option<KeyBinding>,
}

impl Tooltip {
    pub fn text(title: impl Into<SharedString>, cx: &mut gpui::AppContext) -> AnyView {
        cx.new_model(|_cx| Self {
            title: title.into(),
            meta: None,
            key_binding: None,
        })
        .into()
    }

    pub fn for_action(
        title: impl Into<SharedString>,
        action: &dyn Action,
        window: &mut gpui::Window,
        cx: &mut gpui::AppContext,
    ) -> AnyView {
        cx.new_model(|model, cx| Self {
            title: title.into(),
            meta: None,
            key_binding: KeyBinding::for_action(action, window, cx),
        })
        .into()
    }

    pub fn for_action_in(
        title: impl Into<SharedString>,
        action: &dyn Action,
        focus_handle: &FocusHandle,
        window: &mut gpui::Window,
        cx: &mut gpui::AppContext,
    ) -> AnyView {
        cx.new_model(|model, cx| Self {
            title: title.into(),
            meta: None,
            key_binding: KeyBinding::for_action_in(action, focus_handle, window, cx),
        })
        .into()
    }

    pub fn with_meta(
        title: impl Into<SharedString>,
        action: Option<&dyn Action>,
        meta: impl Into<SharedString>,
        window: &mut gpui::Window,
        cx: &mut gpui::AppContext,
    ) -> AnyView {
        cx.new_model(|model, cx| Self {
            title: title.into(),
            meta: Some(meta.into()),
            key_binding: action.and_then(|action| KeyBinding::for_action(action, window, cx)),
        })
        .into()
    }

    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            meta: None,
            key_binding: None,
        }
    }

    pub fn meta(mut self, meta: impl Into<SharedString>) -> Self {
        self.meta = Some(meta.into());
        self
    }

    pub fn key_binding(mut self, key_binding: impl Into<Option<KeyBinding>>) -> Self {
        self.key_binding = key_binding.into();
        self
    }
}

impl Render for Tooltip {
    fn render(
        &mut self,
        window: &mut gpui::Window,
        model: &Model<Self>,
        cx: &mut AppContext,
    ) -> impl IntoElement {
        tooltip_container(window, cx, |el, _| {
            el.child(
                h_flex()
                    .gap_4()
                    .child(self.title.clone())
                    .when_some(self.key_binding.clone(), |this, key_binding| {
                        this.justify_between().child(key_binding)
                    }),
            )
            .when_some(self.meta.clone(), |this, meta| {
                this.child(Label::new(meta).size(LabelSize::Small).color(Color::Muted))
            })
        })
    }
}

pub fn tooltip_container<V>(
    window: &mut Window,
    model: &Model<V>,
    cx: &mut AppContext,
    f: impl FnOnce(Div, &Model<V>, &mut AppContext) -> Div,
) -> impl IntoElement {
    let ui_font = ThemeSettings::get_global(cx).ui_font.clone();

    // padding to avoid tooltip appearing right below the mouse cursor
    div().pl_2().pt_2p5().child(
        v_flex()
            .elevation_2(window, cx)
            .font(ui_font)
            .text_ui(window, cx)
            .text_color(cx.theme().colors().text)
            .py_1()
            .px_2()
            .map(|el| f(el, cx)),
    )
}

pub struct LinkPreview {
    link: SharedString,
}

impl LinkPreview {
    pub fn new(url: &str, cx: &mut gpui::AppContext) -> AnyView {
        let mut wrapped_url = String::new();
        for (i, ch) in url.chars().enumerate() {
            if i == 500 {
                wrapped_url.push('…');
                break;
            }
            if i % 100 == 0 && i != 0 {
                wrapped_url.push('\n');
            }
            wrapped_url.push(ch);
        }
        cx.new_model(|_cx| LinkPreview {
            link: wrapped_url.into(),
        })
        .into()
    }
}

impl Render for LinkPreview {
    fn render(
        &mut self,
        window: &mut gpui::Window,
        model: &Model<Self>,
        cx: &mut AppContext,
    ) -> impl IntoElement {
        tooltip_container(window, cx, |el, _| {
            el.child(
                Label::new(self.link.clone())
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            )
        })
    }
}
