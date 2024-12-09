use gpui::{Render, View};
use story::{Story, StoryItem, StorySection};

use ui::prelude::*;

use crate::application_menu::ApplicationMenu;

pub struct ApplicationMenuStory {
    menu: View<ApplicationMenu>,
}

impl ApplicationMenuStory {
    pub fn new(window: &mut gpui::Window, cx: &mut gpui::AppContext) -> Self {
        Self {
            menu: cx.new_model(ApplicationMenu::new),
        }
    }
}

impl Render for ApplicationMenuStory {
    fn render(&mut self, model: &Model<>Self, _cx: &mut AppContext) -> impl IntoElement {
        Story::container()
            .child(Story::title_for::<ApplicationMenu>())
            .child(StorySection::new().child(StoryItem::new(
                "Application Menu",
                h_flex().child(self.menu.clone()),
            )))
    }
}
